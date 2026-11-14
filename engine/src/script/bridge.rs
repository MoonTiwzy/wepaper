//! The way from a running script back to the host.
//!
//! The host is lent to the runtime for the length of one call. The native functions the
//! prelude calls live in the JavaScript context for as long as the runtime does, so they
//! cannot hold that loan in their types: it is kept here as a raw pointer that is set
//! when a call begins and cleared when it ends.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rquickjs::function::{Opt, Rest};
use rquickjs::{Array, Ctx, Function, Object, Value as Js};

use super::convert::{self, from_datum, to_datum};
use super::host::{Datum, Field, Host};

/// What the native functions share with the runtime.
#[derive(Default)]
pub(super) struct Bridge {
    host: Cell<Option<NonNull<dyn Host>>>,
    /// When the script that is running must be done.
    deadline: Cell<Option<Instant>>,
    /// Whether the interrupt handler stopped the script that ran last.
    timed_out: Cell<bool>,
    /// Members scripts used that are not implemented, with how often.
    unsupported: RefCell<BTreeMap<String, u32>>,
    /// Whether a script registered audio buffers.
    wants_audio: Cell<bool>,
}

/// The host's loan to the bridge; ends it when dropped.
pub(super) struct Lent<'a> {
    bridge: &'a Bridge,
    host: PhantomData<&'a mut dyn Host>,
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        self.bridge.host.set(None);
    }
}

impl Bridge {
    /// Makes `host` the one scripts reach until the returned guard is dropped.
    pub(super) fn lend<'a>(&'a self, host: &'a mut dyn Host) -> Lent<'a> {
        let host: *mut (dyn Host + 'a) = host;
        // SAFETY: only the lifetime bound of the trait object changes, which has no layout.
        // The pointer is dereferenced in `with_host` alone, and only while the guard
        // returned here, which holds the borrow of `host`, is alive: it clears the pointer
        // when it drops.
        let host: *mut (dyn Host + 'static) = unsafe { std::mem::transmute(host) };
        self.host.set(NonNull::new(host));
        Lent { bridge: self, host: PhantomData }
    }

    /// Runs `f` on the lent host; `None` when no call is in progress.
    pub(super) fn with_host<R>(&self, f: impl FnOnce(&mut dyn Host) -> R) -> Option<R> {
        let mut host = self.host.get()?;
        // SAFETY: the pointer is set only while a `Lent` guard holds the exclusive borrow
        // it was made from. The runtime does not touch the host by any other path while
        // the guard lives, and `f` cannot re-enter here: a host has no way to run a script,
        // so this is the only reference for the length of `f`.
        Some(f(unsafe { host.as_mut() }))
    }

    /// Starts the clock for one call into a script.
    pub(super) fn arm(&self, budget: Duration) {
        self.timed_out.set(false);
        self.deadline.set(Some(Instant::now() + budget));
    }

    /// Stops the clock; says whether the call ran out of time.
    pub(super) fn disarm(&self) -> bool {
        self.deadline.set(None);
        self.timed_out.replace(false)
    }

    /// For the engine's interrupt handler: whether the running script is out of time.
    pub(super) fn out_of_time(&self) -> bool {
        let late = self.deadline.get().is_some_and(|deadline| Instant::now() >= deadline);
        if late {
            self.timed_out.set(true);
        }
        late
    }

    pub(super) fn wants_audio(&self) -> bool {
        self.wants_audio.get()
    }

    pub(super) fn unsupported(&self) -> Vec<(String, u32)> {
        self.unsupported.borrow().iter().map(|(name, count)| (name.clone(), *count)).collect()
    }

    /// Counts a use of a member that is not implemented and reports the first.
    fn note_unsupported(&self, name: &str) {
        let first = {
            let mut unsupported = self.unsupported.borrow_mut();
            let count = unsupported.entry(name.to_owned()).or_default();
            *count += 1;
            *count == 1
        };
        if first {
            self.with_host(|host| host.log(log::Level::Debug, &format!("unsupported: {name}")));
        }
    }
}

/// The name a field's code may come with, when it is a string.
fn name_of(value: &Js<'_>) -> String {
    value.as_string().and_then(|name| name.to_string().ok()).unwrap_or_default()
}

fn index_of(index: i32) -> usize {
    usize::try_from(index).unwrap_or(usize::MAX)
}

/// The `host` object of the prelude: one native function for each thing a script can
/// ask of the host. `scene.js` and `engine.js` are the callers.
pub(super) fn natives<'js>(ctx: &Ctx<'js>, bridge: &Rc<Bridge>) -> rquickjs::Result<Object<'js>> {
    let host = Object::new(ctx.clone())?;

    let b = Rc::clone(bridge);
    host.set(
        "get",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, layer: i32, code: i32| {
            let datum = match (convert::layer(layer), Field::from_code(code, 0, "")) {
                (Some(layer), Some(field)) => b.with_host(|host| host.get(layer, field)).unwrap_or_default(),
                _ => Datum::Null,
            };
            from_datum(&ctx, &datum)
        })?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "getPart",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, layer: i32, code: i32, index: i32, name: Js<'js>| {
                let name = name_of(&name);
                let datum = match (convert::layer(layer), Field::from_code(code, index_of(index), &name)) {
                    (Some(layer), Some(field)) => {
                        b.with_host(|host| host.get(layer, field)).unwrap_or_default()
                    }
                    _ => Datum::Null,
                };
                from_datum(&ctx, &datum)
            },
        )?,
    )?;

    let b = Rc::clone(bridge);
    let set = move |layer: i32, code: i32, index: usize, name: &str, value: Datum| {
        if let (Some(layer), Some(field), true) =
            (convert::layer(layer), Field::from_code(code, index, name), convert::is_finite(&value))
        {
            b.with_host(|host| host.set(layer, field, value));
        }
    };

    let assign = set.clone();
    host.set(
        "set",
        Function::new(
            ctx.clone(),
            move |layer: i32, code: i32, x: f64, y: Opt<f64>, z: Opt<f64>, w: Opt<f64>| {
                let value = match (y.0, z.0, w.0) {
                    (Some(y), Some(z), Some(w)) => Datum::Vec4([x, y, z, w]),
                    (Some(y), Some(z), None) => Datum::Vec3([x, y, z]),
                    (Some(y), None, _) => Datum::Vec2([x, y]),
                    _ => Datum::Number(x),
                };
                assign(layer, code, 0, "", value);
            },
        )?,
    )?;
    let assign = set.clone();
    host.set(
        "setBool",
        Function::new(ctx.clone(), move |layer: i32, code: i32, value: bool| {
            assign(layer, code, 0, "", Datum::Bool(value))
        })?,
    )?;
    let assign = set.clone();
    host.set(
        "setText",
        Function::new(ctx.clone(), move |layer: i32, code: i32, value: String| {
            assign(layer, code, 0, "", Datum::Text(value))
        })?,
    )?;
    host.set(
        "setPart",
        Function::new(
            ctx.clone(),
            move |layer: i32, code: i32, index: i32, name: Js<'js>, value: Js<'js>| {
                if let Some(value) = to_datum(&value) {
                    set(layer, code, index_of(index), &name_of(&name), value);
                }
            },
        )?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "call",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  layer: i32,
                  code: i32,
                  index: i32,
                  name: Js<'js>,
                  method: String,
                  arguments: Array<'js>| {
                let name = name_of(&name);
                let arguments: Vec<Datum> = arguments
                    .iter::<Js>()
                    .map(|argument| argument.ok().and_then(|a| to_datum(&a)).unwrap_or_default())
                    .collect();
                let result = convert::layer(layer)
                    .and_then(|layer| {
                        b.with_host(|host| {
                            host.call(
                                layer,
                                Field::from_code(code, index_of(index), &name),
                                &method,
                                &arguments,
                            )
                        })
                    })
                    .flatten();
                match result {
                    Some(datum) => from_datum(&ctx, &datum),
                    None => Ok(Js::new_undefined(ctx)),
                }
            },
        )?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "layers",
        Function::new(ctx.clone(), move || -> Vec<f64> {
            b.with_host(|host| host.layers())
                .unwrap_or_default()
                .into_iter()
                .map(|layer| f64::from(layer.0))
                .collect()
        })?,
    )?;
    let b = Rc::clone(bridge);
    host.set(
        "layerNamed",
        Function::new(ctx.clone(), move |name: String| {
            b.with_host(|host| host.layer_named(&name)).flatten().map_or(-1.0, |layer| f64::from(layer.0))
        })?,
    )?;
    let b = Rc::clone(bridge);
    host.set(
        "layerWithId",
        Function::new(ctx.clone(), move |id: f64| {
            // Scripts pass whatever they have; an id is a whole number that fits.
            let id = (id.is_finite() && id.fract() == 0.0 && id.abs() < 9.0e15).then_some(id as i64);
            id.and_then(|id| b.with_host(|host| host.layer_with_id(id)).flatten())
                .map_or(-1.0, |layer| f64::from(layer.0))
        })?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "sceneGet",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String| {
            from_datum(&ctx, &b.with_host(|host| host.scene_get(&name)).unwrap_or_default())
        })?,
    )?;
    let b = Rc::clone(bridge);
    host.set(
        "sceneSet",
        Function::new(ctx.clone(), move |name: String, value: Js<'js>| {
            if let Some(value) = to_datum(&value).filter(convert::is_finite) {
                b.with_host(|host| host.scene_set(&name, value));
            }
        })?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "userProperty",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String| {
            let value = b.with_host(|host| host.user_property(&name)).unwrap_or(wepaper_json::Value::Null);
            convert::from_json(&ctx, &value)
        })?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "log",
        Function::new(ctx.clone(), move |level: i32, message: String| {
            let level = if level == 1 { log::Level::Error } else { log::Level::Info };
            b.with_host(|host| host.log(level, &message));
        })?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "storageGet",
        Function::new(ctx.clone(), move |key: String| b.with_host(|host| host.storage_get(&key)).flatten())?,
    )?;
    let b = Rc::clone(bridge);
    host.set(
        "storageSet",
        Function::new(ctx.clone(), move |key: String, value: Opt<Option<String>>| {
            b.with_host(|host| host.storage_set(&key, value.0.flatten().as_deref()));
        })?,
    )?;

    let b = Rc::clone(bridge);
    host.set(
        "unsupported",
        Function::new(ctx.clone(), move |name: Rest<String>| {
            b.note_unsupported(name.0.first().map_or("?", String::as_str))
        })?,
    )?;
    let b = Rc::clone(bridge);
    host.set("wantAudio", Function::new(ctx.clone(), move || b.wants_audio.set(true))?)?;

    Ok(host)
}
