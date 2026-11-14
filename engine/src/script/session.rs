//! One call on the runtime from the inside: calling the prelude's entry points within
//! the time budget and turning what fails into a disabled script.

use std::time::Duration;

use rquickjs::function::Args;
use rquickjs::{Coerced, Ctx, Error, Exception, Function, Module, Object, Value as Js};
use wepaper_json::Value;

use super::bridge::Bridge;
use super::convert::{from_json, numbers, to_datum};
use super::error::{ErrorKind, ScriptError};
use super::event::{Handler, MediaEvent};
use super::host::{Datum, Environment};
use super::runtime::{Binding, Script};

/// A frame runs this many timers and promise jobs at most: a script that queues one
/// from every callback must not hold the frame.
const JOBS_PER_FRAME: usize = 256;

/// The inside of one call on the runtime: the context is entered and the host is lent.
pub(super) struct Session<'a, 'js> {
    pub(super) ctx: Ctx<'js>,
    pub(super) api: Object<'js>,
    pub(super) update: Function<'js>,
    pub(super) scripts: &'a mut Vec<Script>,
    pub(super) bridge: &'a Bridge,
    pub(super) budget: Duration,
    /// Whether the screen changed size with this frame.
    pub(super) resized: bool,
    pub(super) environment: Option<Environment>,
}

impl<'js> Session<'_, 'js> {
    pub(super) fn handles(&self, index: usize, handler: Handler) -> bool {
        self.scripts
            .get(index)
            .is_some_and(|script| script.failure.is_none() && script.handlers & handler.bit() != 0)
    }

    pub(super) fn report(&self, message: &str) {
        self.bridge.with_host(|host| host.log(log::Level::Error, message));
    }

    /// Disables a script and says why, once.
    pub(super) fn fail(&mut self, index: usize, error: ScriptError) {
        let Some(script) = self.scripts.get_mut(index) else { return };
        if script.failure.is_none() {
            let message = format!("script on {} is disabled: {error}", script.label);
            script.failure = Some(error);
            self.report(&message);
        }
    }

    pub(super) fn args(&self) -> Args<'js> {
        Args::new_unsized(self.ctx.clone())
    }

    /// Calls a function of the prelude within the time budget.
    pub(super) fn call(&self, function: &Function<'js>, args: Args<'js>) -> Result<Js<'js>, ScriptError> {
        self.bridge.arm(self.budget);
        let result = args.apply::<Js>(function);
        let timed_out = self.bridge.disarm();
        result.map_err(|error| describe(&self.ctx, error, timed_out, self.budget))
    }

    /// Calls an entry point of the prelude by name.
    pub(super) fn entry(
        &self,
        name: &str,
        fill: impl FnOnce(&mut Args<'js>) -> rquickjs::Result<()>,
    ) -> Result<Js<'js>, ScriptError> {
        let mut args = self.args();
        let function =
            self.api.get::<_, Function>(name).and_then(|function| fill(&mut args).map(|()| function));
        let function = function.map_err(|error| describe(&self.ctx, error, false, self.budget))?;
        self.call(&function, args)
    }

    /// Calls an entry point that runs none of a script's code, so cannot fail by one.
    pub(super) fn internal(
        &self,
        name: &str,
        fill: impl FnOnce(&mut Args<'js>) -> rquickjs::Result<()>,
    ) -> Option<Js<'js>> {
        self.entry(name, fill).map_err(|error| self.report(&format!("script runtime: {name}: {error}"))).ok()
    }

    /// Calls an entry point that runs script `index`; a failure disables the script.
    pub(super) fn run(
        &mut self,
        index: usize,
        name: &str,
        fill: impl FnOnce(&mut Args<'js>) -> rquickjs::Result<()>,
    ) -> Option<Js<'js>> {
        match self.entry(name, fill) {
            Ok(value) => Some(value),
            Err(error) => {
                self.fail(index, error);
                None
            }
        }
    }

    pub(super) fn take_environment(&mut self, environment: Environment, advance: bool) {
        let Environment {
            time,
            frame_time,
            time_of_day,
            canvas_size,
            screen_size,
            cursor_scene,
            cursor_screen,
            cursor_down,
            ..
        } = environment;
        let fields = [
            time,
            frame_time,
            time_of_day,
            canvas_size[0],
            canvas_size[1],
            screen_size[0],
            screen_size[1],
            cursor_scene[0],
            cursor_scene[1],
            cursor_screen[0],
            cursor_screen[1],
        ];
        let ctx = self.ctx.clone();
        let resized = self.internal("environment", |args| {
            args.push_arg(numbers(&ctx, &fields)?)?;
            args.push_arg(cursor_down)?;
            args.push_arg(advance)
        });
        self.resized = resized.and_then(|resized| resized.as_bool()).unwrap_or(false);
        self.environment = Some(environment);
    }

    pub(super) fn begin_frame(&mut self) {
        if self.bridge.wants_audio()
            && let Some(environment) = &self.environment
        {
            let samples: Vec<f64> =
                environment.spectrum.iter().flatten().map(|sample| f64::from(*sample)).collect();
            let ctx = self.ctx.clone();
            self.internal("audio", |args| args.push_arg(numbers(&ctx, &samples)?));
        }
        for _ in 0..JOBS_PER_FRAME {
            match self.entry("tick", |_| Ok(())) {
                Ok(more) if more.as_bool() == Some(true) => {}
                Ok(_) => break,
                Err(error) => self.timer_failed(error),
            }
        }
        if self.resized {
            for index in 0..self.scripts.len() {
                if self.handles(index, Handler::ResizeScreen) {
                    let nothing = Js::new_undefined(self.ctx.clone());
                    self.event(index, Handler::ResizeScreen, nothing);
                }
            }
        }
        // What scripts left to promises: `async` functions and `.then` callbacks.
        self.bridge.arm(self.budget);
        for _ in 0..JOBS_PER_FRAME {
            if !self.ctx.execute_pending_job() {
                break;
            }
        }
        self.bridge.disarm();
    }

    /// A timer's callback threw. The timer is gone; the script it belongs to keeps
    /// running, and is named the first time.
    pub(super) fn timer_failed(&mut self, error: ScriptError) {
        let owner = self
            .internal("current", |_| Ok(()))
            .and_then(|id| id.as_number())
            .and_then(|id| usize::try_from(id as i64).ok());
        match owner.and_then(|index| self.scripts.get_mut(index)) {
            Some(script) if script.timer_reported => {}
            Some(script) => {
                script.timer_reported = true;
                let message = format!("a timer of the script on {} failed: {error}", script.label);
                self.report(&message);
            }
            None => self.report(&format!("a script's timer failed: {error}")),
        }
    }

    /// Compiles a script and runs its top level; returns the handlers it exports.
    pub(super) fn load(
        &mut self,
        index: usize,
        source: &str,
        binding: &Binding<'_>,
    ) -> Result<u32, ScriptError> {
        let (code, field_index, name) = binding.field.code();
        self.entry("begin", |args| {
            args.push_arg(index)?;
            args.push_arg(binding.layer.0)?;
            args.push_arg(code)?;
            args.push_arg(field_index)?;
            args.push_arg(name)
        })?;
        // Unique, and what stack traces show: the layer and the field.
        let module = format!("{} #{index}", self.scripts[index].label);
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        self.bridge.arm(self.budget);
        let exports = Module::declare(self.ctx.clone(), module, source).and_then(|module| {
            let (module, loaded) = module.eval()?;
            loaded.finish::<()>()?;
            module.namespace()
        });
        let timed_out = self.bridge.disarm();
        let exports = exports.map_err(|error| describe(&self.ctx, error, timed_out, self.budget))?;
        let ctx = self.ctx.clone();
        let handlers = self.entry("finish", |args| {
            args.push_arg(index)?;
            args.push_arg(exports)?;
            args.push_arg(from_json(&ctx, binding.properties)?)
        })?;
        Ok(handlers.as_number().map_or(0, |mask| mask as u32))
    }

    pub(super) fn update(&mut self, index: usize, value: &Datum) -> Option<Datum> {
        let mut args = self.args();
        let filled = args.push_arg(index).and_then(|()| push_datum(&mut args, value));
        let update = self.update.clone();
        let returned = filled
            .map_err(|error| describe(&self.ctx, error, false, self.budget))
            .and_then(|()| self.call(&update, args));
        match returned {
            Ok(returned) => to_datum(&returned).filter(|value| *value != Datum::Null),
            Err(error) => {
                self.fail(index, error);
                None
            }
        }
    }

    /// Calls a handler that is given the script's value and may return a new one.
    pub(super) fn value(
        &mut self,
        index: usize,
        handler: Handler,
        value: &Datum,
        event: Option<Js<'js>>,
    ) -> Option<Datum> {
        let returned = self.run(index, "value", |args| {
            args.push_arg(index)?;
            args.push_arg(handler as u8)?;
            push_datum(args, value)?;
            args.push_arg(event)
        })?;
        to_datum(&returned).filter(|value| *value != Datum::Null)
    }

    /// Calls a handler that is given an event and returns nothing.
    pub(super) fn event(&mut self, index: usize, handler: Handler, payload: Js<'js>) {
        self.run(index, "event", |args| {
            args.push_arg(index)?;
            args.push_arg(handler as u8)?;
            args.push_arg(payload)
        });
    }

    /// `applyUserProperties` and `applyGeneralSettings`: an object of what changed.
    pub(super) fn apply(&mut self, handler: Handler, changed: &[(String, Value)]) {
        let object = Object::new(self.ctx.clone()).and_then(|object| {
            for (name, value) in changed {
                object.set(name.as_str(), from_json(&self.ctx, value)?)?;
            }
            Ok(object.into_value())
        });
        let Ok(mut object) = object.map_err(|error| self.report(&format!("script runtime: {error}"))) else {
            return;
        };
        if handler == Handler::UserProperties {
            let Some(converted) = self.internal("userProperties", |args| args.push_arg(object)) else {
                return;
            };
            object = converted;
        }
        for index in 0..self.scripts.len() {
            if self.handles(index, handler) {
                self.event(index, handler, object.clone());
            }
        }
    }
}

/// A value as the prelude's `fromWire` takes it: the kind, then four slots for its parts.
fn push_datum<'js>(args: &mut Args<'js>, datum: &Datum) -> rquickjs::Result<()> {
    let (kind, parts): (i32, &[f64]) = match datum {
        Datum::Null | Datum::Layer(_) => (0, &[]),
        Datum::Bool(flag) => {
            args.push_arg(1)?;
            args.push_arg(*flag)?;
            return (0..3).try_for_each(|_| args.push_arg(0));
        }
        Datum::Text(text) => {
            args.push_arg(3)?;
            args.push_arg(text.as_str())?;
            return (0..3).try_for_each(|_| args.push_arg(0));
        }
        Datum::Number(number) => (2, std::slice::from_ref(number)),
        Datum::Vec2(parts) => (4, parts),
        Datum::Vec3(parts) => (5, parts),
        Datum::Vec4(parts) => (6, parts),
    };
    args.push_arg(kind)?;
    (0..4).try_for_each(|i| args.push_arg(parts.get(i).copied().unwrap_or(0.0)))
}

pub(super) fn media_payload<'js>(ctx: &Ctx<'js>, event: &MediaEvent<'_>) -> rquickjs::Result<Js<'js>> {
    let payload = Object::new(ctx.clone())?;
    match *event {
        MediaEvent::Status { enabled } => payload.set("enabled", enabled)?,
        MediaEvent::Playback { state } => payload.set("state", state)?,
        MediaEvent::Properties {
            title,
            artist,
            sub_title,
            album_title,
            album_artist,
            genres,
            content_type,
        } => {
            payload.set("title", title)?;
            payload.set("artist", artist)?;
            payload.set("subTitle", sub_title)?;
            payload.set("albumTitle", album_title)?;
            payload.set("albumArtist", album_artist)?;
            payload.set("genres", genres)?;
            payload.set("contentType", content_type)?;
        }
        MediaEvent::Thumbnail { has_thumbnail, primary, secondary, tertiary, text, high_contrast } => {
            payload.set("hasThumbnail", has_thumbnail)?;
            // Arrays here; `event()` in `scripts.js` makes them `Vec3`.
            for (name, color) in [
                ("primaryColor", primary),
                ("secondaryColor", secondary),
                ("tertiaryColor", tertiary),
                ("textColor", text),
                ("highContrastColor", high_contrast),
            ] {
                payload.set(name, numbers(ctx, &color)?)?;
            }
        }
        MediaEvent::Timeline { position, duration } => {
            payload.set("position", position)?;
            payload.set("duration", duration)?;
        }
    }
    Ok(payload.into_value())
}

/// What an error from the engine means for a script. Takes the pending exception off
/// the context, so the next call starts clean.
pub(super) fn describe(ctx: &Ctx<'_>, error: Error, timed_out: bool, budget: Duration) -> ScriptError {
    if timed_out {
        ctx.catch();
        return ScriptError::new(
            ErrorKind::TimedOut,
            format!("ran for more than {} ms and was stopped", budget.as_millis()),
        );
    }
    match error {
        Error::Exception => {
            let thrown = ctx.catch();
            let Some(exception) = thrown.as_object().cloned().and_then(Exception::from_object) else {
                let text = thrown
                    .get::<Coerced<String>>()
                    .map_or_else(|_| "something that is not an error".to_owned(), |text| text.0);
                return ScriptError::new(ErrorKind::Threw, format!("threw {text}"));
            };
            let name = exception
                .as_object()
                .get::<_, Coerced<String>>("name")
                .map_or_else(|_| "Error".to_owned(), |name| name.0);
            let message = exception.message().unwrap_or_default();
            // A module that is not found comes back from the loader as a thrown error.
            let unloadable =
                message.starts_with("Error loading module") || message.starts_with("Error resolving module");
            let kind =
                if name == "SyntaxError" || unloadable { ErrorKind::Compile } else { ErrorKind::Threw };
            let at = exception.stack().and_then(|stack| {
                let frame = stack.lines().map(str::trim).find(|line| !line.is_empty())?;
                Some(frame.strip_prefix("at ").unwrap_or(frame).to_owned())
            });
            ScriptError { kind, message: format!("{name}: {message}"), at }
        }
        Error::Loading { .. } | Error::Resolving { .. } => {
            ScriptError::new(ErrorKind::Compile, error.to_string())
        }
        Error::WouldBlock => {
            ScriptError::new(ErrorKind::Threw, "waits at its top level for something that never comes")
        }
        other => ScriptError::new(ErrorKind::Threw, other.to_string()),
    }
}
