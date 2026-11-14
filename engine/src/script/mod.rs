//! SceneScript: the JavaScript scenes attach to their values, run against a host.
//!
//! A scene's values can each carry a script: an ES module that exports `update(value)`
//! to compute the value every frame, `init(value)`, and handlers for the cursor, user
//! properties and the media player. A [`Runtime`] holds one JavaScript context for a
//! scene and every script attached in it; the scene is the [`Host`] the scripts read
//! layers from and write them to.
//!
//! - `runtime`: the context, attaching scripts and the calls a scene makes.
//! - `session`: one such call from the inside: time budgets, failures, the prelude's entry points.
//! - `host`: the [`Host`] trait and the values that cross it.
//! - `bridge`: the native functions scripts reach the host through.
//! - `convert`: values between Rust and JavaScript.
//! - `modules`: where `WEMath`, `WEColor`, `WEVector` and the vector classes come from.
//! - `event`: the events scripts handle.
//! - `error`: what goes wrong with a script.
//! - `js/`: the prelude that builds `engine`, `input`, `thisScene`, `thisLayer`,
//!   `console`, `localStorage` and script properties on top of the native functions.
//!
//! A frame, as the scene drives it:
//!
//! ```no_run
//! # use wepaper_engine::script::{Binding, Datum, Field, Host, LayerId, Runtime};
//! # fn frame(host: &mut dyn Host) -> Result<(), wepaper_engine::script::ScriptError> {
//! let mut runtime = Runtime::new()?;
//! let source = "export function update(value) { return value.add(engine.frametime); }";
//! let binding = Binding { layer: LayerId(0), field: Field::Origin, value: Datum::Vec3([0.0; 3]), properties: &Default::default() };
//! let script = runtime.attach(source, binding, host)?;
//! for (script, value) in runtime.init(host) {
//!     // The value `init` returned is the script's value now.
//! }
//! runtime.begin_frame(host);
//! if runtime.has_update(script) {
//!     let origin = runtime.update(script, Datum::Vec3([0.0; 3]), host);
//! }
//! # Ok(()) }
//! ```

mod bridge;
mod convert;
mod error;
mod event;
mod host;
mod modules;
mod runtime;
mod session;

pub use error::{ErrorKind, ScriptError};
pub use event::{CursorEvent, Handler, MediaEvent};
pub use host::{Datum, Environment, Field, Host, LayerId};
pub use runtime::{Binding, Limits, Runtime, ScriptId};
