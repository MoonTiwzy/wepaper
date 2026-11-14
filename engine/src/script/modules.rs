//! Where the code scripts import comes from.
//!
//! Wallpaper Engine ships its script classes and modules as plain JavaScript in its
//! assets, so they are read from there through the host and run as they are. The copies
//! built in here are for a host without them.

use std::rc::Rc;

use rquickjs::loader::{Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module};

use super::bridge::Bridge;

/// `Vec2`, `Vec3`, `Vec4`, `Mat3`, `Mat4` and the rest of the classes scripts take for granted.
pub(super) const STOCK_CLASSES: &str = "scripts/jsclasses/baseclasses.js";
pub(super) const BUILTIN_CLASSES: &str = include_str!("js/classes.js");

/// The prelude, in the order it is run. Each file is one function taking the runtime's
/// private object.
pub(super) const PRELUDE: [(&str, &str); 3] = [
    ("engine.js", include_str!("js/engine.js")),
    ("scene.js", include_str!("js/scene.js")),
    ("scripts.js", include_str!("js/scripts.js")),
];

/// The modules of the SceneScript reference: the name scripts import, the file in the
/// assets, the copy built in.
const MODULES: [(&str, &str, &str); 3] = [
    ("WEMath", "scripts/jsmodules/wemath.js", include_str!("js/wemath.js")),
    ("WEColor", "scripts/jsmodules/wecolor.js", include_str!("js/wecolor.js")),
    ("WEVector", "scripts/jsmodules/wevector.js", include_str!("js/wevector.js")),
];

/// Names modules as scripts spell them: no folders to be relative to.
pub(super) struct Names;

impl Resolver for Names {
    fn resolve<'js>(&mut self, _ctx: &Ctx<'js>, _base: &str, name: &str) -> rquickjs::Result<String> {
        Ok(match MODULES.iter().find(|(module, ..)| module.eq_ignore_ascii_case(name)) {
            Some((module, ..)) => (*module).to_owned(),
            None => name.trim_start_matches("./").to_owned(),
        })
    }
}

/// Finds a module's source: the reference's modules in the assets or built in, anything
/// else among the wallpaper's own scripts.
pub(super) struct Sources(pub(super) Rc<Bridge>);

impl Loader for Sources {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Module<'js, Declared>> {
        let read = |path: &str| self.0.with_host(|host| host.read_script(path)).flatten();
        let source = match MODULES.iter().find(|(module, ..)| *module == name) {
            Some((_, path, builtin)) => read(path).unwrap_or_else(|| (*builtin).to_owned()),
            None => [
                name.to_owned(),
                format!("{name}.js"),
                format!("scripts/{name}"),
                format!("scripts/{name}.js"),
            ]
            .iter()
            .find_map(|path| read(path))
            .ok_or_else(|| Error::new_loading(name))?,
        };
        Module::declare(ctx.clone(), name, source)
    }
}
