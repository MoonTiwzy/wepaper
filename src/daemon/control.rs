//! The control tree: what a client can read, change and do.

use std::path::Path;

use wepaper_engine::Fill;
use wepaper_ipc::{PROTOCOL, Request, error, ok, split_path};
use wepaper_json::{Map, Value, obj};

use super::plugins::{Happening, Kind};
use super::settings::{KEYS, Remembered, Settings, typed_flag, typed_number};
use super::{Daemon, Shown};

/// The time of day, 0 (midnight) to 1, in the local time zone.
pub(super) fn daytime() -> f32 {
    // SAFETY: `time` and `localtime_r` are given valid pointers to locals.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut local: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut local);
        (local.tm_hour * 3600 + local.tm_min * 60 + local.tm_sec) as f32 / 86400.0
    }
}

/// Reads the config file into `settings`. No file is no settings; a broken one is said so.
pub(super) fn read_config(settings: &mut Settings, file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    match wepaper_json::parse(&text) {
        Ok(doc) => settings.apply(&doc, &file.display().to_string()),
        Err(error) => log::error!("{}: {error}; the defaults are used", file.display()),
    }
}

/// The node at `path` in `tree`.
fn descend<'a>(tree: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(tree, |node, part| match node {
        Value::Object(map) => map.get(part),
        Value::Array(items) => items.get(part.parse::<usize>().ok()?),
        _ => None,
    })
}

/// The allocator's own count of the process's memory: what is in use, what it holds
/// free for the next request, and what it took from the system in blocks of their own.
/// Tells a leak (in use grows) from a heap that is only not given back (free grows).
#[cfg(target_env = "gnu")]
fn heap() -> Value {
    // SAFETY: reads the allocator's statistics; no preconditions.
    let info = unsafe { libc::mallinfo2() };
    obj! { "used" => info.uordblks as u64, "free" => info.fordblks as u64, "blocks" => info.hblkhd as u64 }
}

#[cfg(not(target_env = "gnu"))]
fn heap() -> Value {
    Value::Null
}

impl Daemon {
    /// Everything the daemon knows, as one tree.
    fn tree(&mut self) -> Value {
        let mut outputs = Map::new();
        for head in &self.display.heads {
            let (Some(name), Some(slot)) = (&head.name, self.slots.get(&head.id)) else { continue };
            let size = head.sheet.as_ref().map(|sheet| sheet.pixels()).unwrap_or((0, 0));
            let scale = head.sheet.as_ref().map_or(1.0, |sheet| f64::from(sheet.scale_120) / 120.0);
            let wallpaper = match (&slot.wallpaper, &slot.shown.wallpaper) {
                (Some(wallpaper), Some(folder)) => {
                    let project = wallpaper.project();
                    let mut node = Map::new();
                    node.insert("path", Value::from(folder.to_string_lossy().into_owned()));
                    node.insert("kind", Value::from(project.kind.name()));
                    node.insert("title", Value::from(&project.title));
                    node.insert("state", Value::from("playing"));
                    for (key, value) in wallpaper.tree().members() {
                        node.insert(key, value.clone());
                    }
                    Value::Object(node)
                }
                (None, Some(folder)) if slot.renderer.is_some() => {
                    let plugin = slot.renderer.as_ref().map(|r| r.plugin.clone()).unwrap_or_default();
                    obj! {
                        "path" => folder.to_string_lossy().into_owned(),
                        "kind" => plugin.clone(),
                        "title" => &slot.title,
                        "state" => "playing",
                        "plugin" => plugin,
                        "properties" => Value::Object(slot.shown.properties.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                    }
                }
                _ => Value::Null,
            };
            outputs.insert(
                name.clone(),
                obj! {
                    "size" => vec![Value::from(size.0), Value::from(size.1)],
                    "scale" => scale,
                    "paused" => slot.paused,
                    "fill" => slot.shown.fill.map(Fill::name),
                    // The count of the last full second; nothing drawn for two means none.
                    "fps" => if slot.last_draw.elapsed().as_secs() >= 2 { 0 } else { slot.frames.2 },
                    "error" => slot.error.clone(),
                    "wallpaper" => wallpaper,
                },
            );
        }
        let memory = self.engine.gl().memory();
        let mut plugins = Map::new();
        for manifest in &self.plugins.manifests {
            let mut node = Map::new();
            node.insert("name", Value::from(&manifest.name));
            node.insert("kind", Value::from(manifest.kind.name()));
            node.insert("enabled", Value::from(manifest.enabled));
            match manifest.kind {
                Kind::Service => node.insert("running", Value::from(self.plugins.running(&manifest.id))),
                Kind::Shader => {
                    let params = self
                        .shaders
                        .iter()
                        .find(|(id, _)| *id == manifest.id)
                        .map(|(_, pass)| pass.params.clone())
                        .unwrap_or_default();
                    node.insert(
                        "params",
                        Value::Object(
                            params.into_iter().map(|(name, value)| (name, Value::from(value))).collect(),
                        ),
                    );
                }
                Kind::Renderer => {
                    node.insert("handles", Value::Array(manifest.handles.iter().map(Value::from).collect()))
                }
                Kind::Hook => {}
            }
            plugins.insert(manifest.id.clone(), Value::Object(node));
        }
        obj! {
            "config" => self.settings.to_value(),
            "plugins" => Value::Object(plugins),
            "outputs" => Value::Object(outputs),
            "memory" => obj! {
                "textures" => memory.textures,
                "buffers" => memory.targets,
                "video" => memory.video,
                "videos" => self.engine.videos().playing(),
                "optimising" => self.optimising,
                "listening" => self.engine.listening(),
                "peak_effect_buffers" => self.engine.peak_buffers(),
                "heap" => heap(),
            },
        }
    }

    /// The ids of the outputs a name stands for: one output, or all for `None` and `*`.
    fn targets(&self, output: Option<&str>) -> Result<Vec<u32>, String> {
        let known = || {
            self.display.heads.iter().filter_map(|head| head.name.as_deref()).collect::<Vec<_>>().join(", ")
        };
        match output.filter(|name| *name != "*") {
            None => Ok(self
                .display
                .heads
                .iter()
                .filter(|head| head.name.is_some() && self.slots.contains_key(&head.id))
                .map(|head| head.id)
                .collect()),
            Some(name) => match self.display.heads.iter().find(|head| head.name.as_deref() == Some(name)) {
                Some(head) => Ok(vec![head.id]),
                None => Err(format!("there is no output \"{name}\" (outputs: {})", known())),
            },
        }
    }

    /// Answers one request.
    pub(super) fn handle(&mut self, request: Request, client: usize) -> Value {
        let result = match request {
            Request::Version => Ok(obj! {
                "version" => env!("CARGO_PKG_VERSION"),
                "protocol" => PROTOCOL,
                "features" => vec![Value::from("scene"), Value::from("video")],
                "gpu" => &self.context.description,
            }),
            Request::Get { path } => {
                let tree = self.tree();
                descend(&tree, &split_path(&path))
                    .cloned()
                    .ok_or_else(|| format!("there is nothing at \"{path}\""))
            }
            Request::List { path } => {
                let tree = self.tree();
                match descend(&tree, &split_path(&path)) {
                    Some(Value::Object(map)) => {
                        Ok(Value::Array(map.iter().map(|(key, _)| Value::from(key)).collect()))
                    }
                    Some(Value::Array(items)) => {
                        Ok(Value::Array((0..items.len()).map(Value::from).collect()))
                    }
                    Some(_) => Err(format!("\"{path}\" is a value, not a list")),
                    None => Err(format!("there is nothing at \"{path}\"")),
                }
            }
            Request::Set { path, value } => self.set(&split_path(&path), &value).map(|()| Value::Null),
            Request::Reset { path } => self.reset(&split_path(&path)).map(|()| Value::Null),
            Request::Show { wallpaper, output, properties } => {
                self.show(&wallpaper, output.as_deref(), properties).map(|()| Value::Null)
            }
            Request::Hide { output } => self.targets(output.as_deref()).map(|ids| {
                for id in ids {
                    let Some(slot) = self.slots.get_mut(&id) else { continue };
                    slot.wallpaper = None;
                    slot.renderer = None;
                    slot.shown = Shown::default();
                    slot.error = None;
                    slot.stale = true;
                    // The surface would keep its last frame: a new one has nothing on
                    // it, and what is under the wallpaper shows.
                    let name = self.display.heads.iter_mut().find(|head| head.id == id).and_then(|head| {
                        head.sheet = None;
                        head.name.clone()
                    });
                    self.display.cover(id, self.settings.layer, self.settings.passthrough);
                    self.plugins.announce(&Happening {
                        event: "hidden",
                        output: name.unwrap_or_default(),
                        ..Happening::default()
                    });
                }
                self.state.last = Shown::default();
                self.release();
                self.save_state();
                Value::Null
            }),
            Request::Pause { output } => self.pause(output.as_deref(), true),
            Request::Resume { output } => self.pause(output.as_deref(), false),
            Request::Snapshot { output, file } => {
                self.snapshot(output.as_deref(), Path::new(&file)).map(|()| Value::Null)
            }
            Request::Watch => {
                self.server.watch(client);
                Ok(Value::Null)
            }
            Request::Reload => {
                self.state.overrides = Map::new();
                self.recompute_settings();
                self.save_state();
                Ok(Value::Null)
            }
            Request::Quit => {
                self.quit = true;
                Ok(Value::Null)
            }
        };
        match result {
            Ok(value) => ok(value),
            Err(why) => error(why),
        }
    }

    fn pause(&mut self, output: Option<&str>, paused: bool) -> Result<Value, String> {
        for id in self.targets(output)? {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.paused = paused;
                if let Some(wallpaper) = &mut slot.wallpaper {
                    wallpaper.set_paused(paused);
                }
                if let Some(renderer) = &mut slot.renderer {
                    renderer.send(&obj! { "cmd" => if paused { "pause" } else { "resume" } });
                }
            }
            let output = self
                .display
                .heads
                .iter()
                .find(|head| head.id == id)
                .and_then(|head| head.name.clone())
                .unwrap_or_default();
            self.plugins.announce(&Happening {
                event: if paused { "paused" } else { "resumed" },
                output,
                ..Happening::default()
            });
        }
        if paused {
            // A frozen output needs no effect buffers until it plays again.
            self.engine.trim();
        }
        self.notify("paused", Value::from(paused));
        Ok(Value::Null)
    }

    fn show(
        &mut self,
        wallpaper: &str,
        output: Option<&str>,
        properties: Option<Vec<(String, Value)>>,
    ) -> Result<(), String> {
        let folder = wepaper_engine::locate::wallpaper(wallpaper)
            .ok_or_else(|| format!("no such wallpaper: {wallpaper}"))?;
        let ids = self.targets(output)?;
        if ids.is_empty() {
            return Err("there are no outputs to show it on".to_owned());
        }
        // Values named now are the values; otherwise the wallpaper comes back as it was left.
        let remembered = Remembered::load(&self.memory_dir, &folder);
        let named = properties.is_some();
        let properties = properties.map_or(remembered.properties, |named| named.into_iter().collect());
        let shown =
            Shown { wallpaper: Some(folder.clone()), properties, edits: remembered.edits, fill: None };
        // An output that cannot show it keeps what it had; the others are not undone
        // for it, and what they show now is what is remembered.
        let mut failure = None;
        let mut any = false;
        for id in ids {
            match self.load(id, &shown) {
                Ok(()) => {
                    any = true;
                    if let Some(slot) = self.slots.get_mut(&id) {
                        slot.shown = Shown { fill: slot.shown.fill, ..shown.clone() };
                    }
                }
                Err(why) => failure = failure.or(Some(why)),
            }
        }
        if any {
            if named {
                self.remember(&shown);
            }
            if output.is_none_or(|name| name == "*") {
                self.state.last = shown;
            }
            self.save_state();
            self.notify("shown", Value::from(folder.to_string_lossy().into_owned()));
        }
        failure.map_or(Ok(()), Err)
    }

    /// Keeps a wallpaper's values for the next time it is shown.
    fn remember(&self, shown: &Shown) {
        if let Some(folder) = &shown.wallpaper {
            Remembered { properties: shown.properties.clone(), edits: shown.edits.clone() }
                .save(&self.memory_dir, folder);
        }
    }

    /// Loads again what the outputs called `name` show: for values that only a
    /// fresh wallpaper has back.
    fn reload_outputs(&mut self, name: &str) -> Result<(), String> {
        for id in self.targets(Some(name))? {
            let Some(shown) =
                self.slots.get(&id).map(|slot| slot.shown.clone()).filter(|shown| shown.wallpaper.is_some())
            else {
                continue;
            };
            self.load(id, &shown)?;
            self.remember(&shown);
        }
        Ok(())
    }

    fn set(&mut self, path: &[&str], value: &Value) -> Result<(), String> {
        match path {
            ["config", key] => {
                self.settings.set(key, value)?;
                self.state.overrides.insert(*key, value.clone());
                self.settings_changed(key);
                self.save_state();
                Ok(())
            }
            ["outputs", name, "paused"] => {
                let paused = typed_flag(value).ok_or("paused takes true or false")?;
                self.pause(Some(name), paused).map(|_| ())
            }
            ["outputs", name, "fill"] => {
                let fill = match value {
                    Value::Null => None,
                    other => Some(
                        other
                            .as_str()
                            .and_then(Fill::from_name)
                            .ok_or("fill takes cover, contain or stretch (or null for the setting)")?,
                    ),
                };
                for id in self.targets(Some(name))? {
                    if let Some(slot) = self.slots.get_mut(&id) {
                        slot.shown.fill = fill;
                        slot.stale = true;
                        if let Some(wallpaper) = &mut slot.wallpaper {
                            wallpaper.set_fill(fill.unwrap_or(self.settings.fill));
                        }
                    }
                }
                self.save_state();
                Ok(())
            }
            ["plugins", id, "enabled"] => {
                let enabled = typed_flag(value).ok_or("enabled takes true or false")?;
                let manifest = self
                    .plugins
                    .manifests
                    .iter_mut()
                    .find(|m| m.id == *id)
                    .ok_or_else(|| format!("there is no plugin \"{id}\""))?;
                manifest.enabled = enabled;
                if let Some((_, pass)) = self.shaders.iter_mut().find(|(shader, _)| shader == id) {
                    pass.enabled = enabled;
                }
                self.redraw_all();
                Ok(())
            }
            ["plugins", id, "params", param] => {
                let number = typed_number(value).ok_or("a parameter takes a number")? as f32;
                let (_, pass) = self
                    .shaders
                    .iter_mut()
                    .find(|(shader, _)| shader == id)
                    .ok_or_else(|| format!("there is no shader plugin \"{id}\""))?;
                if !pass.set(param, number) {
                    return Err(format!("the shader of \"{id}\" has no parameter \"{param}\""));
                }
                self.redraw_all();
                Ok(())
            }
            ["outputs", name, "wallpaper", "properties", property]
                if self
                    .targets(Some(name))?
                    .iter()
                    .any(|id| self.slots.get(id).is_some_and(|slot| slot.renderer.is_some())) =>
            {
                let mut remembered = Vec::new();
                for id in self.targets(Some(name))? {
                    let Some(slot) = self.slots.get_mut(&id) else { continue };
                    if let Some(renderer) = &mut slot.renderer {
                        let mut values = Map::new();
                        values.insert(*property, value.clone());
                        renderer.send(&obj! { "cmd" => "properties", "values" => Value::Object(values) });
                        slot.shown.properties.insert((*property).to_owned(), value.clone());
                        remembered.push(slot.shown.clone());
                    }
                }
                for shown in &remembered {
                    self.remember(shown);
                }
                self.save_state();
                Ok(())
            }
            ["outputs", name, "wallpaper", rest @ ..] if !rest.is_empty() => {
                let ids = self.targets(Some(name))?;
                let mut changed = false;
                let mut failure = None;
                let mut remembered = Vec::new();
                for id in ids {
                    let Some(slot) = self.slots.get_mut(&id) else { continue };
                    let Some(wallpaper) = &mut slot.wallpaper else { continue };
                    // With `*`, an output whose wallpaper has no such value does not
                    // undo the change on those that have.
                    if let Err(why) = wallpaper.edit(rest, value) {
                        failure = failure.or(Some(why));
                        continue;
                    }
                    slot.stale = true;
                    changed = true;
                    match rest {
                        ["properties", property] => {
                            slot.shown.properties.insert((*property).to_owned(), value.clone())
                        }
                        _ => slot.shown.edits.insert(rest.join("/"), value.clone()),
                    };
                    remembered.push(slot.shown.clone());
                }
                for shown in &remembered {
                    self.remember(shown);
                }
                if changed {
                    self.save_state();
                }
                match failure {
                    Some(why) => Err(why),
                    None if !changed => Err("no wallpaper is shown there".to_owned()),
                    None => Ok(()),
                }
            }
            _ => Err(format!("\"{}\" cannot be set", path.join("/"))),
        }
    }

    fn reset(&mut self, path: &[&str]) -> Result<(), String> {
        match path {
            ["config"] => {
                self.state.overrides = Map::new();
                self.recompute_settings();
            }
            ["config", key] if KEYS.contains(key) => {
                self.state.overrides.remove(key);
                self.recompute_settings();
            }
            ["outputs", name, "wallpaper", "properties", rest @ ..] if rest.len() <= 1 => {
                let mut remembered = Vec::new();
                for id in self.targets(Some(name))? {
                    let Some(slot) = self.slots.get_mut(&id) else { continue };
                    let Some(wallpaper) = &mut slot.wallpaper else { continue };
                    let defaults: Vec<(String, Value)> = wallpaper
                        .project()
                        .properties
                        .iter()
                        .map(|p| (p.name.clone(), p.default.clone()))
                        .collect();
                    for (property, default) in defaults
                        .iter()
                        .filter(|(property, _)| rest.first().is_none_or(|only| only == property))
                    {
                        if !default.is_null() {
                            wallpaper.set_property(property, default);
                        }
                        slot.shown.properties.remove(property);
                    }
                    slot.stale = true;
                    remembered.push(slot.shown.clone());
                }
                for shown in &remembered {
                    self.remember(shown);
                }
            }
            // What was changed inside the wallpaper is undone by loading it again without.
            ["outputs", name, "wallpaper", rest @ ..] => {
                let under = rest.join("/");
                for id in self.targets(Some(name))? {
                    let Some(slot) = self.slots.get_mut(&id) else { continue };
                    if rest.is_empty() {
                        slot.shown.properties.clear();
                    }
                    slot.shown.edits.retain(|path, _| {
                        !(under.is_empty() || path == &under || path.starts_with(&format!("{under}/")))
                    });
                }
                self.reload_outputs(name)?;
            }
            _ => return Err(format!("\"{}\" cannot be reset", path.join("/"))),
        }
        self.save_state();
        Ok(())
    }

    /// Settings anew: the config file, then what the socket changed, then the command line.
    fn recompute_settings(&mut self) {
        let before = self.settings.clone();
        let mut settings = Settings::default();
        read_config(&mut settings, &self.config_file);
        settings.apply(&Value::Object(self.state.overrides.clone()), "saved settings");
        for (key, value) in &self.from_flags {
            let _ = settings.set(key, value);
        }
        self.settings = settings;
        for key in KEYS {
            if before.to_value().get(key) != self.settings.to_value().get(key) {
                self.settings_changed(key);
            }
        }
    }

    /// Makes a changed setting take effect on what is running.
    fn settings_changed(&mut self, key: &str) {
        match key {
            "volume" | "mute" => self.apply_sound(),
            "fps" => {
                for renderer in self.slots.values_mut().filter_map(|slot| slot.renderer.as_mut()) {
                    renderer.send(&obj! { "cmd" => "fps", "value" => self.settings.fps });
                }
            }
            "fill" => {
                for slot in self.slots.values_mut() {
                    slot.stale = true;
                    if let Some(wallpaper) = &mut slot.wallpaper {
                        wallpaper.set_fill(slot.shown.fill.unwrap_or(self.settings.fill));
                    }
                }
            }
            "layer" | "passthrough" => {
                // A surface's layer and input region are set when it is made: make them anew.
                let ids: Vec<u32> = self.display.heads.iter().map(|head| head.id).collect();
                for id in ids {
                    if let Some(head) = self.display.heads.iter_mut().find(|head| head.id == id) {
                        head.sheet = None;
                    }
                    self.display.cover(id, self.settings.layer, self.settings.passthrough);
                    if let Some(slot) = self.slots.get_mut(&id) {
                        slot.stale = true;
                    }
                }
            }
            _ => {}
        }
        self.notify(&format!("config/{key}"), self.settings.to_value().get(key).clone());
    }

    /// Has every output draw again, though nothing in its wallpaper changed.
    fn redraw_all(&mut self) {
        for slot in self.slots.values_mut() {
            slot.stale = true;
        }
    }

    /// Writes what an output shows right now to a PNG.
    fn snapshot(&mut self, output: Option<&str>, file: &Path) -> Result<(), String> {
        use wepaper_engine::gpu::{Target, TextureFormat};
        let id = *self.targets(output)?.first().ok_or("there are no outputs")?;
        let size = {
            let viewport = self.viewport(id);
            (viewport.width, viewport.height)
        };
        let head = self.display.heads.iter().find(|head| head.id == id).ok_or("the output is gone")?;
        let pointer = head.pointer;
        let slot = self.slots.get_mut(&id).ok_or("the output is gone")?;
        let wallpaper = slot.wallpaper.as_mut().ok_or("no wallpaper is shown there")?;
        let frame =
            Target::new(self.engine.gl(), size.0, size.1, TextureFormat::Rgba8).map_err(|e| e.to_string())?;
        frame.clear([0.0, 0.0, 0.0, 1.0]);
        let input = wepaper_engine::scene::FrameInput {
            time: slot.clock,
            pointer,
            pointer_down: false,
            daytime: daytime(),
        };
        wallpaper.draw(&mut self.engine, &input, Some(frame.framebuffer()), size, true);
        crate::cli::render::write_png(file, size.0, size.1, &frame.read_rgba())
    }

    /// Tells the watchers that something changed.
    fn notify(&mut self, what: &str, value: Value) {
        if self.server.has_watchers() {
            self.server.broadcast(&obj! { "event" => "changed", "what" => what, "value" => value });
        }
    }
}
