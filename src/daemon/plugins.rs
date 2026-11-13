//! Plugins: what extends the daemon without being part of it.
//!
//! A plugin is a folder with a `plugin.json`. There are four kinds:
//!
//! - `hook`: a command run when something happens, told what through `WEPAPER_*`
//!   variables.
//! - `service`: a process the daemon keeps running, which talks to it over the
//!   control socket like any client.
//! - `shader`: a fragment shader run over every finished frame, with parameters
//!   that can be set while it runs.
//! - `renderer`: a process that draws a kind of wallpaper the daemon does not
//!   (web wallpapers), one per output.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use wepaper_json::{Value, obj};

/// What a plugin is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hook,
    Service,
    Shader,
    Renderer,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Hook => "hook",
            Kind::Service => "service",
            Kind::Shader => "shader",
            Kind::Renderer => "renderer",
        }
    }
}

/// A plugin's `plugin.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// The name it is addressed by: its folder's name unless the manifest gives one.
    pub id: String,
    /// A name for people.
    pub name: String,
    pub kind: Kind,
    /// The folder it lives in; relative paths in the manifest start here.
    pub dir: PathBuf,
    /// The command of a hook, service or renderer.
    pub exec: String,
    /// The events a hook runs on; empty for all.
    pub events: Vec<String>,
    /// The wallpaper kinds a renderer draws.
    pub handles: Vec<String>,
    /// A shader plugin's fragment shader file.
    pub shader: String,
    /// The defaults of a shader's parameters.
    pub params: BTreeMap<String, f32>,
    pub enabled: bool,
}

impl Manifest {
    /// Reads a manifest. The error says what is wrong with it.
    pub fn parse(doc: &Value, dir: &Path) -> Result<Self, String> {
        let text = |key: &str| doc.get(key).as_str().unwrap_or("").to_owned();
        let list = |key: &str| doc.get(key).items().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>();
        let kind = match doc.get("kind").as_str() {
            Some("hook") => Kind::Hook,
            Some("service") => Kind::Service,
            Some("shader") => Kind::Shader,
            Some("renderer") => Kind::Renderer,
            Some(other) => return Err(format!("unknown kind \"{other}\" (hook, service, shader or renderer)")),
            None => return Err("no \"kind\"".to_owned()),
        };
        let folder = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let manifest = Self {
            id: Some(text("id")).filter(|id| !id.is_empty()).unwrap_or(folder),
            name: text("name"),
            kind,
            dir: dir.to_owned(),
            exec: text("exec"),
            events: list("events"),
            handles: list("handles"),
            shader: text("shader"),
            // A parameter is a number, or an object with a "default".
            params: doc
                .get("params")
                .members()
                .filter_map(|(name, value)| Some((name.to_owned(), value.as_f32().or_else(|| value.get("default").as_f32())?)))
                .collect(),
            enabled: doc.get("enabled").as_bool().unwrap_or(true),
        };
        if manifest.id.is_empty() || manifest.id.contains('/') {
            return Err("the id must be a name without slashes".to_owned());
        }
        match kind {
            Kind::Shader if manifest.shader.is_empty() => Err("a shader plugin names its \"shader\" file".to_owned()),
            Kind::Renderer if manifest.handles.is_empty() => Err("a renderer says which kinds it \"handles\"".to_owned()),
            Kind::Hook | Kind::Service | Kind::Renderer if manifest.exec.is_empty() => Err(format!("a {} plugin needs \"exec\"", kind.name())),
            _ => Ok(manifest),
        }
    }

    /// The program of `exec` with paths relative to the plugin's folder resolved.
    pub fn program(&self) -> PathBuf {
        let first = self.exec.split_whitespace().next().unwrap_or("");
        if first.starts_with("./") || first.starts_with("../") { self.dir.join(first) } else { PathBuf::from(first) }
    }
}

/// The folders plugins are looked for in, later ones winning over earlier ones:
/// next to the binary, the system's data folders, the user's data folder, the
/// user's config folder, and the ones named on the command line.
pub fn search_dirs(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(Path::parent)
    {
        dirs.push(prefix.join("share/wepaper/plugins"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("wepaper/plugins")));
    dirs.push(super::settings::xdg_dir("XDG_DATA_HOME", ".local/share").join("plugins"));
    dirs.push(super::settings::xdg_dir("XDG_CONFIG_HOME", ".config").join("plugins"));
    dirs.extend(extra.iter().cloned());
    dirs
}

/// Every plugin found in `dirs`. A plugin in a later folder replaces one of the
/// same id from an earlier one; a broken manifest is reported and skipped.
pub fn discover(dirs: &[PathBuf]) -> Vec<Manifest> {
    let mut found: BTreeMap<String, Manifest> = BTreeMap::new();
    for dir in dirs {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let folder = entry.path();
            let Ok(text) = std::fs::read_to_string(folder.join("plugin.json")) else { continue };
            let manifest = wepaper_json::parse(&text).map_err(|e| e.to_string()).and_then(|doc| Manifest::parse(&doc, &folder));
            match manifest {
                Ok(manifest) => {
                    found.insert(manifest.id.clone(), manifest);
                }
                Err(why) => log::warn!("{}: {why}", folder.join("plugin.json").display()),
            }
        }
    }
    found.into_values().collect()
}

/// What happened, for hooks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Happening {
    /// `shown`, `hidden`, `paused`, `resumed`, `started`, `stopping`.
    pub event: &'static str,
    pub output: String,
    pub wallpaper: String,
    pub title: String,
    pub kind: String,
}

impl Happening {
    /// The variables a hook is run with.
    pub fn variables(&self, socket: &Path) -> Vec<(&'static str, String)> {
        vec![
            ("WEPAPER_EVENT", self.event.to_owned()),
            ("WEPAPER_OUTPUT", self.output.clone()),
            ("WEPAPER_WALLPAPER", self.wallpaper.clone()),
            ("WEPAPER_TITLE", self.title.clone()),
            ("WEPAPER_KIND", self.kind.clone()),
            ("WEPAPER_SOCKET", socket.to_string_lossy().into_owned()),
        ]
    }
}

/// A service and when it may be started again.
struct Running {
    child: Option<Child>,
    /// How long to wait before the next start; doubles each time it dies young.
    backoff: Duration,
    retry_at: Instant,
    started: Instant,
}

/// A renderer plugin's process for one output.
pub struct Renderer {
    child: Child,
    pub plugin: String,
}

impl Renderer {
    /// Sends the process one message. False when it is gone.
    pub fn send(&mut self, message: &Value) -> bool {
        let Some(pipe) = self.child.stdin.as_mut() else { return false };
        let mut line = message.to_json();
        line.push('\n');
        pipe.write_all(line.as_bytes()).is_ok()
    }

    /// Whether the process is still there.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // It leaves on `quit` or when its input closes; if it does neither, it is made to.
        let _ = self.send(&obj! { "cmd" => "quit" });
        drop(self.child.stdin.take());
        for _ in 0..20 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The plugins and what of them runs.
pub struct Plugins {
    pub manifests: Vec<Manifest>,
    socket: PathBuf,
    services: BTreeMap<String, Running>,
    /// Hooks that were started and have not been waited for yet.
    hooks: Vec<Child>,
}

impl Plugins {
    pub fn new(manifests: Vec<Manifest>, socket: PathBuf) -> Self {
        Self { manifests, socket, services: BTreeMap::new(), hooks: Vec::new() }
    }

    /// The renderer plugin that draws wallpapers of `kind`.
    pub fn renderer_for(&self, kind: &str) -> Option<&Manifest> {
        self.manifests.iter().find(|m| m.kind == Kind::Renderer && m.enabled && m.handles.iter().any(|h| h == kind))
    }

    /// Runs every hook that listens for `happening`. They are not waited for here.
    pub fn announce(&mut self, happening: &Happening) {
        for manifest in self.manifests.iter().filter(|m| m.kind == Kind::Hook && m.enabled) {
            if !manifest.events.is_empty() && !manifest.events.iter().any(|event| event == happening.event) {
                continue;
            }
            let mut command = Command::new("sh");
            command.arg("-c").arg(&manifest.exec).current_dir(&manifest.dir).envs(happening.variables(&self.socket));
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            match command.spawn() {
                Ok(child) => self.hooks.push(child),
                Err(error) => log::warn!("hook {}: {error}", manifest.id),
            }
        }
    }

    /// Keeps services running and takes leave of hooks that finished. Call now and then.
    pub fn upkeep(&mut self) {
        self.hooks.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        let now = Instant::now();
        for manifest in self.manifests.iter().filter(|m| m.kind == Kind::Service) {
            let running = self.services.entry(manifest.id.clone()).or_insert(Running { child: None, backoff: Duration::from_secs(1), retry_at: now, started: now });
            if !manifest.enabled {
                if let Some(mut child) = running.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                continue;
            }
            if let Some(child) = &mut running.child {
                match child.try_wait() {
                    Ok(None) => continue,
                    _ => {
                        // One that lasted a minute starts over at once; one that keeps dying waits longer each time.
                        running.backoff = if running.started.elapsed() > Duration::from_secs(60) { Duration::from_secs(1) } else { (running.backoff * 2).min(Duration::from_secs(60)) };
                        running.retry_at = now + running.backoff;
                        running.child = None;
                        log::warn!("service {} stopped; next start in {} s", manifest.id, running.backoff.as_secs());
                    }
                }
            }
            if now < running.retry_at {
                continue;
            }
            let mut command = Command::new("sh");
            command.arg("-c").arg(&manifest.exec).current_dir(&manifest.dir).env("WEPAPER_SOCKET", &self.socket).stdin(Stdio::null());
            match command.spawn() {
                Ok(child) => {
                    running.child = Some(child);
                    running.started = now;
                }
                Err(error) => {
                    log::warn!("service {}: {error}", manifest.id);
                    running.retry_at = now + running.backoff;
                }
            }
        }
    }

    /// Whether a service's process is there.
    pub fn running(&self, id: &str) -> bool {
        self.services.get(id).is_some_and(|running| running.child.is_some())
    }

    /// Starts a renderer plugin's process for one output.
    pub fn render(&self, manifest: &Manifest, arguments: &[String]) -> Result<Renderer, String> {
        let mut words = manifest.exec.split_whitespace().skip(1).map(str::to_owned).collect::<Vec<_>>();
        words.extend_from_slice(arguments);
        let child = Command::new(manifest.program())
            .args(words)
            .current_dir(&manifest.dir)
            .env("WEPAPER_SOCKET", &self.socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| format!("{} could not be started: {e}", manifest.program().display()))?;
        Ok(Renderer { child, plugin: manifest.id.clone() })
    }
}

impl Drop for Plugins {
    fn drop(&mut self) {
        for running in self.services.values_mut() {
            if let Some(mut child) = running.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        for mut hook in self.hooks.drain(..) {
            let _ = hook.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wepaper-test-plugins-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(json: &str, dir: &Path) -> Result<Manifest, String> {
        Manifest::parse(&wepaper_json::parse(json).unwrap(), dir)
    }

    #[test]
    fn manifest_kinds() {
        let dir = Path::new("/plugins/crt");
        let shader = manifest(r#"{"kind": "shader", "shader": "crt.frag", "params": {"strength": 0.5, "curve": {"default": 2, "min": 0, "max": 4}}}"#, dir).unwrap();
        assert_eq!((shader.id.as_str(), shader.kind, shader.enabled), ("crt", Kind::Shader, true), "the folder's name is the id");
        assert_eq!(shader.params, [("curve".to_owned(), 2.0), ("strength".to_owned(), 0.5)].into());
        let renderer = manifest(r#"{"id": "web", "kind": "renderer", "exec": "wepaper-web --fast", "handles": ["web"], "enabled": false}"#, dir).unwrap();
        assert_eq!((renderer.id.as_str(), renderer.handles.as_slice(), renderer.enabled), ("web", &["web".to_owned()][..], false));
        assert_eq!(renderer.program(), PathBuf::from("wepaper-web"), "a bare name is looked up on the PATH");
        let local = manifest(r#"{"kind": "hook", "exec": "./notify.sh now", "events": ["shown"]}"#, dir).unwrap();
        assert_eq!(local.program(), PathBuf::from("/plugins/crt/./notify.sh"), "a relative path starts at the plugin's folder");
        assert!(manifest(r#"{"kind": "dance"}"#, dir).unwrap_err().contains("unknown kind"));
        assert!(manifest(r#"{"exec": "x"}"#, dir).unwrap_err().contains("kind"));
        assert!(manifest(r#"{"kind": "shader"}"#, dir).unwrap_err().contains("shader"));
        assert!(manifest(r#"{"kind": "renderer", "exec": "x"}"#, dir).unwrap_err().contains("handles"));
        assert!(manifest(r#"{"kind": "service"}"#, dir).unwrap_err().contains("exec"));
        assert!(manifest(r#"{"kind": "hook", "exec": "x", "id": "a/b"}"#, dir).is_err());
    }

    #[test]
    fn later_folders_win_and_broken_manifests_are_skipped() {
        let (system, user) = (temp("system"), temp("user"));
        for (root, name, body) in [
            (&system, "a", r#"{"kind": "hook", "exec": "true", "name": "system a"}"#),
            (&system, "b", r#"{"kind": "hook", "exec": "true"}"#),
            (&user, "a", r#"{"kind": "hook", "exec": "true", "name": "user a"}"#),
            (&user, "broken", r#"{ not json"#),
            (&user, "nokind", r#"{"exec": "true"}"#),
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("plugin.json"), body).unwrap();
        }
        let found = discover(&[system, user, PathBuf::from("/nonexistent/plugins")]);
        assert_eq!(found.iter().map(|m| (m.id.as_str(), m.name.as_str())).collect::<Vec<_>>(), [("a", "user a"), ("b", "")]);
    }

    #[test]
    fn hook_receives_event_variables() {
        let dir = temp("hook");
        let out = dir.join("out.txt");
        let hook = manifest(&format!(r#"{{"kind": "hook", "events": ["shown"], "exec": "echo $WEPAPER_EVENT $WEPAPER_OUTPUT $WEPAPER_KIND \"$WEPAPER_TITLE\" $WEPAPER_SOCKET > {}"}}"#, out.display()), &dir).unwrap();
        let mut plugins = Plugins::new(vec![hook], PathBuf::from("/run/wepaper.sock"));
        plugins.announce(&Happening { event: "paused", output: "DP-1".into(), ..Happening::default() });
        plugins.announce(&Happening { event: "shown", output: "DP-1".into(), wallpaper: "/w/1".into(), title: "A B".into(), kind: "scene".into() });
        for _ in 0..200 {
            plugins.upkeep();
            if plugins.hooks.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "shown DP-1 scene A B /run/wepaper.sock", "only the event it listens for ran it");
    }

    #[test]
    fn service_restarts_with_backoff() {
        let dir = temp("service");
        let service = manifest(r#"{"id": "service", "kind": "service", "exec": "exit 1"}"#, &dir).unwrap();
        let mut plugins = Plugins::new(vec![service], PathBuf::from("/run/wepaper.sock"));
        plugins.upkeep();
        std::thread::sleep(Duration::from_millis(150));
        plugins.upkeep();
        let running = &plugins.services["service"];
        assert!(running.child.is_none(), "the process that exited is noticed");
        assert_eq!(running.backoff, Duration::from_secs(2), "it died young: the wait doubles");
        assert!(running.retry_at > Instant::now() + Duration::from_secs(1), "and it is not started again at once");
        plugins.upkeep();
        assert!(!plugins.running("service"));
    }
}
