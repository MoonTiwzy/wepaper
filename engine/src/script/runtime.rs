//! The runtime: one JavaScript context for a scene and the scripts attached in it.

use std::rc::Rc;
use std::time::Duration;

use rquickjs::{Context, Ctx, Function, Object, Persistent, Value as Js};
use wepaper_json::Value;

use super::bridge::{Bridge, natives};
use super::error::{ErrorKind, ScriptError};
use super::event::{CursorEvent, Handler, MediaEvent};
use super::host::{Datum, Field, Host, LayerId};
use super::modules::{BUILTIN_CLASSES, Names, PRELUDE, STOCK_CLASSES, Sources};
use super::session::{Session, describe, media_payload};

/// A script attached to a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptId(usize);

/// What a script may use up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How long one call into a script may run before it is stopped and the script
    /// disabled.
    pub call_budget: Duration,
    /// How many bytes the scripts of a scene may hold together.
    pub memory: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { call_budget: Duration::from_millis(50), memory: 256 << 20 }
    }
}

/// What a script is attached to.
#[derive(Debug, Clone)]
pub struct Binding<'a> {
    /// The layer: the script's `thisLayer`.
    pub layer: LayerId,
    /// The value the script drives. Its part of the layer is the script's `thisObject`:
    /// the layer itself, one of its effects, its particle instance.
    pub field: Field<'a>,
    /// The value as the scene has it: what `init` is given.
    pub value: Datum,
    /// The script's `scriptproperties` from the scene: an object of values, each plain or
    /// `{"user": <user property>, "value": <without one>}`. `Null` when it has none.
    pub properties: &'a Value,
}

pub(super) struct Script {
    pub(super) layer: LayerId,
    /// The layer's name and the field, for messages.
    pub(super) label: String,
    /// A bit for every [`Handler`] the script exports.
    pub(super) handlers: u32,
    pub(super) initial: Datum,
    /// Why the script is disabled, when it is.
    pub(super) failure: Option<ScriptError>,
    /// Whether a failing timer of this script was reported already.
    pub(super) timer_reported: bool,
}

/// The entry points of the prelude.
struct Api {
    object: Persistent<Object<'static>>,
    /// Kept apart from `object`: it is called for every script every frame.
    update: Persistent<Function<'static>>,
}

/// One JavaScript context for a scene. Its scripts are separate modules that see the
/// same globals, so `shared` is common to all of them.
///
/// Every method that runs scripts takes the host for the length of the call. A script
/// that throws or runs out of time is disabled on its own and reported through
/// [`Host::log`] once; the rest keep running.
pub struct Runtime {
    // Declared in the order they must be dropped: what holds JavaScript values, then
    // the context, then the engine.
    api: Option<Api>,
    private: Persistent<Object<'static>>,
    context: Context,
    _engine: rquickjs::Runtime,
    bridge: Rc<Bridge>,
    scripts: Vec<Script>,
    limits: Limits,
}

/// The prelude is the runtime's own code, and Wallpaper Engine's classes are 40 KB to
/// compile: they get more time than a script's call.
const SETUP_BUDGET: Duration = Duration::from_secs(5);
/// The fields of the prelude's `env`, in the order `environment()` in `scripts.js` takes them.
const ENVIRONMENT: [&str; 11] = [
    "time",
    "frametime",
    "timeOfDay",
    "canvasWidth",
    "canvasHeight",
    "screenWidth",
    "screenHeight",
    "cursorX",
    "cursorY",
    "pointerX",
    "pointerY",
];

/// How much of the environment a call takes from the host before it runs scripts.
#[derive(Clone, Copy, PartialEq)]
enum Refresh {
    /// Nothing: what the frame began with still holds.
    Keep,
    /// The environment as it is now.
    Read,
    /// The environment of a new frame.
    Advance,
}

impl Runtime {
    /// A runtime with the default limits.
    pub fn new() -> Result<Self, ScriptError> {
        Self::with_limits(Limits::default())
    }

    /// A runtime whose scripts are held to `limits`.
    pub fn with_limits(limits: Limits) -> Result<Self, ScriptError> {
        let engine = rquickjs::Runtime::new().map_err(ScriptError::setup)?;
        engine.set_memory_limit(limits.memory);
        let bridge = Rc::new(Bridge::default());
        let clock = Rc::clone(&bridge);
        engine.set_interrupt_handler(Some(Box::new(move || clock.out_of_time())));
        engine.set_loader(Names, Sources(Rc::clone(&bridge)));
        let context = Context::full(&engine).map_err(ScriptError::setup)?;
        let private = context
            .with(|ctx| -> rquickjs::Result<_> {
                let private = Object::new(ctx.clone())?;
                let env = Object::new(ctx.clone())?;
                for name in ENVIRONMENT {
                    env.set(name, 0.0)?;
                }
                env.set("cursorDown", false)?;
                env.set("frame", 0)?;
                private.set("host", natives(&ctx, &bridge)?)?;
                private.set("env", env)?;
                private.set("current", Js::new_null(ctx.clone()))?;
                Ok(Persistent::save(&ctx, private))
            })
            .map_err(ScriptError::setup)?;
        Ok(Self { api: None, private, context, _engine: engine, bridge, scripts: Vec::new(), limits })
    }

    /// Compiles `source` and runs its top level, attached to `binding`.
    ///
    /// A script that does not compile, or throws while loading, is reported through the
    /// host and returned as the error; the scripts attached before it are not affected.
    pub fn attach(
        &mut self,
        source: &str,
        binding: Binding<'_>,
        host: &mut dyn Host,
    ) -> Result<ScriptId, ScriptError> {
        self.prepare(host)?;
        let index = self.scripts.len();
        let layer = match host.get(binding.layer, Field::Name) {
            Datum::Text(name) => name,
            _ => format!("layer {}", binding.layer.0),
        };
        self.scripts.push(Script {
            layer: binding.layer,
            label: format!("\"{layer}\" ({})", binding.field),
            handlers: 0,
            initial: binding.value.clone(),
            failure: None,
            timer_reported: false,
        });
        let loaded =
            self.session(host, Refresh::Read, |session| match session.load(index, source, &binding) {
                Ok(handlers) => {
                    session.scripts[index].handlers = handlers;
                    Ok(ScriptId(index))
                }
                Err(error) => {
                    session.fail(index, error.clone());
                    Err(error)
                }
            });
        loaded.unwrap_or_else(|| Err(ScriptError::new(ErrorKind::Setup, "the script runtime is not set up")))
    }

    /// Calls every script's `init(value)` with the value it was attached with, in the
    /// order they were attached. Returns what they returned: each is the script's value
    /// from now on.
    ///
    /// Wallpaper Engine also calls `applyUserProperties` once when a wallpaper loads:
    /// follow this with [`Runtime::apply_user_properties`] and every property.
    pub fn init(&mut self, host: &mut dyn Host) -> Vec<(ScriptId, Datum)> {
        let returned = self.session(host, Refresh::Read, |session| {
            let mut returned = Vec::new();
            for index in 0..session.scripts.len() {
                if session.handles(index, Handler::Init) {
                    let initial = session.scripts[index].initial.clone();
                    returned.extend(
                        session
                            .value(index, Handler::Init, &initial, None)
                            .map(|value| (ScriptId(index), value)),
                    );
                }
            }
            returned
        });
        returned.unwrap_or_default()
    }

    /// Starts a frame: takes the environment from the host, feeds the audio buffers,
    /// runs the timers that are due and tells scripts when the screen changed size.
    /// Call it once a frame, before the frame's [`Runtime::update`]s.
    pub fn begin_frame(&mut self, host: &mut dyn Host) {
        self.session(host, Refresh::Advance, |session| session.begin_frame());
    }

    /// Calls a script's `update(value)` with the value as the scene has it. Returns the
    /// new value; `None` when the script returned nothing, has no `update`, is disabled
    /// or failed just now.
    pub fn update(&mut self, id: ScriptId, value: Datum, host: &mut dyn Host) -> Option<Datum> {
        if !self.handles(id, Handler::Update) {
            return None;
        }
        self.session(host, Refresh::Keep, |session| session.update(id.0, &value)).flatten()
    }

    /// Tells scripts that user properties changed: moves the script properties that follow
    /// them, then calls `applyUserProperties(changed)` with these properties only.
    pub fn apply_user_properties(&mut self, changed: &[(String, Value)], host: &mut dyn Host) {
        self.session(host, Refresh::Read, |session| session.apply(Handler::UserProperties, changed));
    }

    /// Tells scripts that application settings changed (`language` is the only one
    /// Wallpaper Engine has): calls `applyGeneralSettings(changed)`.
    pub fn apply_general_settings(&mut self, changed: &[(String, Value)], host: &mut dyn Host) {
        self.session(host, Refresh::Read, |session| session.apply(Handler::GeneralSettings, changed));
    }

    /// Passes a cursor event to the scripts attached to `target`, the layer the scene
    /// found under the cursor; with `None`, to every script that handles the event, which
    /// is how `cursorMove` is delivered. The position scripts see is the host's
    /// environment at the time of the call.
    pub fn cursor(&mut self, event: CursorEvent, target: Option<LayerId>, host: &mut dyn Host) {
        let handler = event.handler();
        if !self.any_handles(handler) {
            return;
        }
        self.session(host, Refresh::Read, |session| {
            for index in 0..session.scripts.len() {
                if session.handles(index, handler)
                    && target.is_none_or(|layer| session.scripts[index].layer == layer)
                {
                    let nothing = Js::new_undefined(session.ctx.clone());
                    session.event(index, handler, nothing);
                }
            }
        });
    }

    /// Passes a media player event to every script that handles it.
    pub fn media(&mut self, event: MediaEvent<'_>, host: &mut dyn Host) {
        let handler = event.handler();
        if !self.any_handles(handler) {
            return;
        }
        self.session(host, Refresh::Keep, |session| {
            for index in 0..session.scripts.len() {
                if session.handles(index, handler) {
                    // A fresh object each: a script may keep or change the one it is given.
                    match media_payload(&session.ctx, &event) {
                        Ok(payload) => session.event(index, handler, payload),
                        Err(error) => session.report(&format!("script runtime: {error}")),
                    }
                }
            }
        });
    }

    /// Tells a script that the animation on its value reached a keyframe event `name` at
    /// `frame`: calls `animationEvent(event, value)`. Returns the value it returned.
    pub fn animation_event(
        &mut self,
        id: ScriptId,
        name: &str,
        frame: f64,
        value: Datum,
        host: &mut dyn Host,
    ) -> Option<Datum> {
        if !self.handles(id, Handler::AnimationEvent) {
            return None;
        }
        let returned = self.session(host, Refresh::Keep, |session| {
            let event = Object::new(session.ctx.clone()).and_then(|event| {
                event.set("name", name)?;
                event.set("frame", frame)?;
                Ok(event.into_value())
            });
            match event {
                Ok(event) => session.value(id.0, Handler::AnimationEvent, &value, Some(event)),
                Err(error) => {
                    session.report(&format!("script runtime: {error}"));
                    None
                }
            }
        });
        returned.flatten()
    }

    /// Calls every script's `destroy()`. For a scene that is about to be dropped.
    pub fn destroy(&mut self, host: &mut dyn Host) {
        self.session(host, Refresh::Keep, |session| {
            for index in 0..session.scripts.len() {
                if session.handles(index, Handler::Destroy) {
                    let nothing = Js::new_undefined(session.ctx.clone());
                    session.event(index, Handler::Destroy, nothing);
                }
            }
        });
    }

    /// Whether a script has an `update` to call every frame. Scripts that only handle
    /// events have none, and a disabled script has nothing.
    pub fn has_update(&self, id: ScriptId) -> bool {
        self.handles(id, Handler::Update)
    }

    /// Whether a script exports `handler` and is still enabled.
    pub fn handles(&self, id: ScriptId, handler: Handler) -> bool {
        self.scripts
            .get(id.0)
            .is_some_and(|script| script.failure.is_none() && script.handlers & handler.bit() != 0)
    }

    /// Whether any enabled script exports `handler`: a scene without `cursorEnter`
    /// handlers need not find the layer under the cursor.
    pub fn any_handles(&self, handler: Handler) -> bool {
        self.scripts.iter().any(|script| script.failure.is_none() && script.handlers & handler.bit() != 0)
    }

    /// Whether a script registered audio buffers: somebody has to listen for the
    /// spectrum in the host's environment to be more than silence.
    pub fn wants_audio(&self) -> bool {
        self.bridge.wants_audio()
    }

    /// Why a script is disabled; `None` while it runs.
    pub fn failure(&self, id: ScriptId) -> Option<&ScriptError> {
        self.scripts.get(id.0)?.failure.as_ref()
    }

    /// The members of SceneScript that scripts used and this runtime, or the host, does
    /// not implement, with how many times each was used.
    pub fn unsupported(&self) -> Vec<(String, u32)> {
        self.bridge.unsupported()
    }

    /// Loads the classes and the prelude on the first use. They wait for a host because
    /// Wallpaper Engine's own classes are read through it.
    fn prepare(&mut self, host: &mut dyn Host) -> Result<(), ScriptError> {
        if self.api.is_some() {
            return Ok(());
        }
        let stock = host.read_script(STOCK_CLASSES);
        let bridge = Rc::clone(&self.bridge);
        let lent = bridge.lend(host);
        let private = self.private.clone();
        let loaded = self.context.with(|ctx| -> rquickjs::Result<Api> {
            let defined = |ctx: &Ctx<'_>| {
                ctx.eval::<bool, _>(
                    "typeof Vec2 === 'function' && typeof Vec3 === 'function' && typeof Mat4 === 'function'",
                )
            };
            bridge.arm(SETUP_BUDGET);
            if let Some(stock) = stock
                && let Err(error) = ctx.eval::<(), _>(stock)
            {
                let error = describe(&ctx, error, false, SETUP_BUDGET);
                bridge.with_host(|host| {
                    host.log(
                        log::Level::Warn,
                        &format!("{STOCK_CLASSES} does not load, using the built-in classes: {error}"),
                    )
                });
            }
            if !defined(&ctx)? {
                ctx.eval::<(), _>(BUILTIN_CLASSES)?;
            }
            let private = private.restore(&ctx)?;
            for (_, source) in PRELUDE {
                ctx.eval::<Function, _>(source)?.call::<_, ()>((private.clone(),))?;
            }
            let object: Object = private.get("api")?;
            let update: Function = object.get("update")?;
            Ok(Api { object: Persistent::save(&ctx, object), update: Persistent::save(&ctx, update) })
        });
        let loaded = loaded
            .map_err(|error| self.context.with(|ctx| describe(&ctx, error, bridge.disarm(), SETUP_BUDGET)));
        bridge.disarm();
        drop(lent);
        match loaded {
            Ok(api) => {
                self.api = Some(api);
                Ok(())
            }
            Err(error) => {
                let error = ScriptError { kind: ErrorKind::Setup, ..error };
                host.log(log::Level::Error, &format!("script runtime: {error}"));
                Err(error)
            }
        }
    }

    /// Lends the host to the scripts and runs `f` inside the context. `None` before the
    /// first script is attached: there is nothing to run then.
    fn session<R>(
        &mut self,
        host: &mut dyn Host,
        refresh: Refresh,
        f: impl FnOnce(&mut Session<'_, '_>) -> R,
    ) -> Option<R> {
        let api = self.api.as_ref()?;
        let environment = (refresh != Refresh::Keep).then(|| host.environment());
        let bridge = Rc::clone(&self.bridge);
        let _lent = bridge.lend(host);
        let (scripts, budget) = (&mut self.scripts, self.limits.call_budget);
        self.context.with(|ctx| {
            let object = api.object.clone().restore(&ctx).ok()?;
            let update = api.update.clone().restore(&ctx).ok()?;
            let mut session = Session {
                ctx,
                api: object,
                update,
                scripts,
                bridge: &bridge,
                budget,
                resized: false,
                environment: None,
            };
            if let Some(environment) = environment {
                session.take_environment(environment, refresh == Refresh::Advance);
            }
            Some(f(&mut session))
        })
    }
}
