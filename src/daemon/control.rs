//! The control tree: what a client can read, change and do.

use std::path::Path;

use wepaper_engine::Fill;
use wepaper_engine::doc::FromDoc;
use wepaper_ipc::{PROTOCOL, Request, error, ok, split_path};
use wepaper_json::{Map, Value, obj};

use super::settings::{KEYS, Settings};
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
        obj! {
            "config" => self.settings.to_value(),
            "outputs" => Value::Object(outputs),
            "memory" => obj! {
                "textures" => memory.textures,
                "buffers" => memory.targets,
                "video" => memory.video,
                "videos" => self.engine.videos().playing(),
                "listening" => self.engine.listening(),
                "peak_effect_buffers" => self.engine.peak_buffers(),
            },
        }
    }

    /// The ids of the outputs a name stands for: one output, or all for `None` and `*`.
    fn targets(&self, output: Option<&str>) -> Result<Vec<u32>, String> {
        let known = || self.display.heads.iter().filter_map(|head| head.name.as_deref()).collect::<Vec<_>>().join(", ");
        match output.filter(|name| *name != "*") {
            None => Ok(self.display.heads.iter().filter(|head| head.name.is_some() && self.slots.contains_key(&head.id)).map(|head| head.id).collect()),
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
                descend(&tree, &split_path(&path)).cloned().ok_or_else(|| format!("there is nothing at \"{path}\""))
            }
            Request::List { path } => {
                let tree = self.tree();
                match descend(&tree, &split_path(&path)) {
                    Some(Value::Object(map)) => Ok(Value::Array(map.iter().map(|(key, _)| Value::from(key)).collect())),
                    Some(Value::Array(items)) => Ok(Value::Array((0..items.len()).map(Value::from).collect())),
                    Some(_) => Err(format!("\"{path}\" is a value, not a list")),
                    None => Err(format!("there is nothing at \"{path}\"")),
                }
            }
            Request::Set { path, value } => self.set(&split_path(&path), &value).map(|()| Value::Null),
            Request::Reset { path } => self.reset(&split_path(&path)).map(|()| Value::Null),
            Request::Show { wallpaper, output, properties } => self.show(&wallpaper, output.as_deref(), properties).map(|()| Value::Null),
            Request::Hide { output } => self.targets(output.as_deref()).map(|ids| {
                for id in ids {
                    if let Some(slot) = self.slots.get_mut(&id) {
                        slot.wallpaper = None;
                        slot.shown = Shown::default();
                        slot.error = None;
                    }
                }
                self.state.last = Shown::default();
                self.engine.trim();
                self.save_state();
                Value::Null
            }),
            Request::Pause { output } => self.pause(output.as_deref(), true),
            Request::Resume { output } => self.pause(output.as_deref(), false),
            Request::Snapshot { output, file } => self.snapshot(output.as_deref(), Path::new(&file)).map(|()| Value::Null),
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
            }
        }
        if paused {
            // A frozen output needs no effect buffers until it plays again.
            self.engine.trim();
        }
        self.notify("paused", Value::from(paused));
        Ok(Value::Null)
    }

    fn show(&mut self, wallpaper: &str, output: Option<&str>, properties: Vec<(String, Value)>) -> Result<(), String> {
        let folder = wepaper_engine::locate::wallpaper(wallpaper).ok_or_else(|| format!("no such wallpaper: {wallpaper}"))?;
        let ids = self.targets(output)?;
        if ids.is_empty() {
            return Err("there are no outputs to show it on".to_owned());
        }
        for id in ids {
            self.load(id, &folder, &properties)?;
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.shown = Shown { wallpaper: Some(folder.clone()), properties: properties.iter().cloned().collect(), fill: slot.shown.fill };
            }
        }
        if output.is_none_or(|name| name == "*") {
            self.state.last = Shown { wallpaper: Some(folder.clone()), properties: properties.into_iter().collect(), fill: None };
        }
        self.save_state();
        self.notify("shown", Value::from(folder.to_string_lossy().into_owned()));
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
                let paused = bool::from_doc(value).ok_or("paused takes true or false")?;
                self.pause(Some(name), paused).map(|_| ())
            }
            ["outputs", name, "fill"] => {
                let fill = match value {
                    Value::Null => None,
                    other => Some(other.as_str().and_then(Fill::from_name).ok_or("fill takes cover, contain or stretch (or null for the setting)")?),
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
            ["outputs", name, "wallpaper", rest @ ..] if !rest.is_empty() => {
                let ids = self.targets(Some(name))?;
                let mut changed = false;
                for id in ids {
                    let Some(slot) = self.slots.get_mut(&id) else { continue };
                    let Some(wallpaper) = &mut slot.wallpaper else { continue };
                    wallpaper.edit(rest, value)?;
                    slot.stale = true;
                    changed = true;
                    if let ["properties", property] = rest {
                        slot.shown.properties.insert((*property).to_owned(), value.clone());
                    }
                }
                if !changed {
                    return Err("no wallpaper is shown there".to_owned());
                }
                self.save_state();
                Ok(())
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
                for id in self.targets(Some(name))? {
                    let Some(slot) = self.slots.get_mut(&id) else { continue };
                    let Some(wallpaper) = &mut slot.wallpaper else { continue };
                    let defaults: Vec<(String, Value)> = wallpaper.project().properties.iter().map(|p| (p.name.clone(), p.default.clone())).collect();
                    for (property, default) in defaults.iter().filter(|(property, _)| rest.first().is_none_or(|only| only == property)) {
                        if !default.is_null() {
                            wallpaper.set_property(property, default);
                        }
                        slot.shown.properties.remove(property);
                    }
                    slot.stale = true;
                }
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
        let frame = Target::new(self.engine.gl(), size.0, size.1, TextureFormat::Rgba8).map_err(|e| e.to_string())?;
        frame.clear([0.0, 0.0, 0.0, 1.0]);
        let input = wepaper_engine::scene::FrameInput { time: slot.clock, pointer, daytime: daytime() };
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
