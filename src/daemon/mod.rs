//! `wepaper daemon`: the wallpapers on the desktop.
//!
//! One thread, one loop: it waits on the compositor, the control socket and the
//! video decoders' wake-up at once, and each time round it handles what came in and
//! draws the outputs that are due a frame.

mod control;
mod device;
mod look;
mod plugins;
mod server;
mod settings;
mod sound;
mod watch;
mod wayland;

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wayland_client::{Connection, EventQueue};
use wepaper_engine::scene::FrameInput;
use wepaper_engine::{Engine, Options, Viewport, Wallpaper, gpu};

use crate::cli::{Failure, value};
use server::Server;
use settings::{Settings, Shown, State, xdg_dir};
use wayland::Display;

pub const USAGE: &str = "\
wepaper daemon [wallpaper] [options]

Shows wallpapers as the desktop background and listens for commands.
Without a wallpaper it shows what it showed last.

  [wallpaper]          a folder or a workshop id to show on every output
      --socket PATH    the control socket (default $XDG_RUNTIME_DIR/wepaper.sock)
      --config FILE    the config file (default $XDG_CONFIG_HOME/wepaper/config.json)
      --state FILE     where what is shown is remembered (default $XDG_STATE_HOME/wepaper/state.json)
      --assets DIR     Wallpaper Engine's assets folder (default: found in Steam)
      --set KEY=VALUE  a setting for this run, e.g. --set fps=30 (repeatable)
      --plugins DIR    one more folder to look for plugins in (repeatable)

Devices (outputs that are not screens) are described in the config file and under
`devices/` in the control tree: see docs/devices.md.
";

/// Set by SIGINT and SIGTERM.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// What plays on one output.
struct Slot {
    wallpaper: Option<Wallpaper>,
    /// The title of what is shown, for a wallpaper a plugin draws.
    title: String,
    shown: Shown,
    paused: bool,
    /// The wallpaper's own time: it stands still while paused.
    clock: f64,
    last_tick: Instant,
    last_draw: Instant,
    /// When the next frame is due under the frame cap.
    due: Instant,
    /// How many more frames are presented without waiting for the GPU first.
    light_frames: u32,
    /// The surface lost its contents (it was resized, or just made): draw even without a new frame.
    stale: bool,
    /// The output's wallpaper is known but not loaded yet: its size is not.
    waiting: bool,
    /// Why the last wallpaper asked for did not show.
    error: Option<String>,
    /// The plugin process that draws this output's wallpaper, when the engine does not.
    renderer: Option<plugins::Renderer>,
    /// The file a plugin drawing for a device writes its page to while snapshots
    /// of the device are asked for, and until when they are.
    stage: Option<(PathBuf, Instant)>,
    /// How many wallpapers were loaded here so far.
    loads: u64,
    /// Where the frame is drawn before output shaders run over it.
    staging: Option<gpu::Target>,
    /// Frames drawn in the current second, and the count of the last full one.
    frames: (Instant, u32, u32),
}

impl Slot {
    fn new(now: Instant) -> Self {
        Self {
            wallpaper: None,
            shown: Shown::default(),
            paused: false,
            clock: 0.0,
            last_tick: now,
            last_draw: now,
            due: now,
            light_frames: 0,
            stale: true,
            waiting: false,
            error: None,
            title: String::new(),
            renderer: None,
            stage: None,
            loads: 0,
            staging: None,
            frames: (now, 0, 0),
        }
    }
}

struct Daemon {
    connection: Connection,
    display: Display,
    context: gpu::Context,
    engine: Engine,
    server: Server,
    /// Config file, then socket changes, then command line.
    settings: Settings,
    from_flags: Vec<(String, wepaper_json::Value)>,
    config_file: PathBuf,
    state: State,
    state_file: PathBuf,
    /// Where what is remembered about each wallpaper is kept.
    memory_dir: PathBuf,
    /// By output id: a monitor's, or a device's.
    slots: HashMap<u32, Slot>,
    /// The outputs that are not screens.
    devices: Vec<device::Device>,
    /// The devices of the config file, as it describes them.
    device_config: wepaper_json::Value,
    /// The devices there are, on or off: the config file's and those set over the socket.
    device_specs: std::collections::BTreeMap<String, device::Spec>,
    /// The devices changed: `devices` is to be made to match.
    devices_dirty: bool,
    next_device: u32,
    /// The control socket, for the programs devices start.
    socket: PathBuf,
    /// When the outputs were last looked at for drawing.
    last_round: Instant,
    last_upkeep: Instant,
    /// When what a wallpaper that went left behind is given back once more.
    release_at: Option<Instant>,
    /// How many videos are being re-encoded right now.
    optimising: usize,
    plugins: plugins::Plugins,
    /// What the outputs showed and which of them played when sound was last settled.
    sounding: Vec<sound::Playing>,
    /// The compiled shader plugins, by plugin id, in the order they run.
    shaders: Vec<(String, wepaper_engine::post::PostPass)>,
    /// The pass that gives an output its look, when the look is not plain.
    look_pass: Option<wepaper_engine::post::PostPass>,
    /// What ends the daemon when its loop cannot.
    watch: watch::Watch,
    quit: bool,
}

pub fn run(mut args: lexopt::Parser) -> Result<(), Failure> {
    use lexopt::prelude::*;
    let (mut wallpaper, mut socket, mut assets) = (None::<String>, None::<PathBuf>, None::<PathBuf>);
    let mut config_file = xdg_dir("XDG_CONFIG_HOME", ".config").join("config.json");
    let mut state_file = xdg_dir("XDG_STATE_HOME", ".local/state").join("state.json");
    let mut from_flags = Vec::new();
    let mut plugin_dirs: Vec<PathBuf> = Vec::new();
    while let Some(arg) = args.next().map_err(|e| e.to_string())? {
        match arg {
            Long("socket") => socket = Some(value(&mut args)?.into()),
            Long("config") => config_file = value(&mut args)?.into(),
            Long("state") => state_file = value(&mut args)?.into(),
            Long("assets") => assets = Some(value(&mut args)?.into()),
            Long("set") => from_flags.push(crate::cli::parse_property(&value(&mut args)?)?),
            Long("plugins") => plugin_dirs.push(value(&mut args)?.into()),
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            Value(name) if wallpaper.is_none() => wallpaper = Some(name.to_string_lossy().into_owned()),
            other => return Err(other.unexpected().to_string()),
        }
    }

    // Few arenas: the loader's threads would otherwise each keep their own heap for good.
    // SAFETY: plain libc calls with valid arguments, before any other thread exists.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
        // A block of this size and up is a mapping of its own, back with the system the
        // moment it is freed. Left alone the allocator raises the limit to the largest
        // block it has freed, up to 32 MB: the pictures a wallpaper is decoded from then
        // come out of the heap, and the heap keeps what it was once given. A daemon that
        // had shown a dozen wallpapers held 140 MB that nothing used.
        libc::mallopt(libc::M_MMAP_THRESHOLD, 256 * 1024);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as *const () as usize;
        // No SA_RESTART: a signal must end the wait in poll.
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
    }

    let socket = socket.unwrap_or_else(wepaper_ipc::default_socket);
    let server = Server::bind(&socket)?;
    let (connection, mut queue, display) = Display::connect()?;
    // SAFETY: the display pointer is the connection's, which the daemon keeps for as long as the context.
    let context = unsafe { gpu::Context::wayland(connection.backend().display_ptr().cast()) }
        .map_err(|e| e.to_string())?;
    log::info!("drawing with {}", context.description);
    let state = State::load(&state_file);
    let mut settings = Settings::default();
    let device_config = control::read_config(&mut settings, &config_file);
    settings.apply(&wepaper_json::Value::Object(state.overrides.clone()), "saved settings");
    for (key, value) in &from_flags {
        settings.set(key, value)?;
    }
    let options = Options {
        assets,
        cache_dir: Some(crate::cli::cache_dir()),
        max_texture_side: settings.texture_max_side,
    };
    let engine = Engine::new(context.gl(), options).map_err(|e| e.to_string())?;

    let found = plugins::discover(&plugins::search_dirs(&plugin_dirs));
    let mut shaders = Vec::new();
    for manifest in found.iter().filter(|m| m.kind == plugins::Kind::Shader) {
        let compiled = std::fs::read_to_string(manifest.dir.join(&manifest.shader))
            .map_err(|e| e.to_string())
            .and_then(|source| {
                wepaper_engine::post::PostPass::compile(context.gl(), &source).map_err(|e| e.to_string())
            });
        match compiled {
            Ok(mut pass) => {
                pass.enabled = manifest.enabled;
                for (name, value) in &manifest.params {
                    pass.set(name, *value);
                }
                shaders.push((manifest.id.clone(), pass));
            }
            Err(why) => log::warn!("shader plugin {}: {why}", manifest.id),
        }
    }
    let now = Instant::now();
    let context_gl = &context.gl().clone();
    let mut daemon = Daemon {
        connection,
        display,
        context,
        engine,
        server,
        settings,
        from_flags,
        config_file,
        state,
        memory_dir: state_file.parent().unwrap_or(std::path::Path::new(".")).join("wallpapers"),
        state_file,
        slots: HashMap::new(),
        devices: Vec::new(),
        device_config,
        device_specs: std::collections::BTreeMap::new(),
        devices_dirty: true,
        next_device: device::FIRST_ID,
        socket: socket.clone(),
        last_round: now,
        last_upkeep: now,
        release_at: None,
        optimising: 0,
        plugins: plugins::Plugins::new(found, socket),
        sounding: Vec::new(),
        shaders,
        look_pass: wepaper_engine::post::PostPass::compile(context_gl, look::SHADER)
            .inspect_err(|why| log::warn!("looks: {why}"))
            .ok(),
        watch: watch::Watch::start(&STOP),
        quit: false,
    };
    daemon.apply_sound();
    if let Some(wallpaper) = wallpaper {
        // Shown once the outputs are known; until then it is what "last" is.
        let folder = wepaper_engine::locate::wallpaper(&wallpaper)
            .ok_or_else(|| format!("no such wallpaper: {wallpaper}"))?;
        daemon.state.last = Shown { wallpaper: Some(folder), ..Shown::default() };
        daemon.state.outputs.clear();
    }
    daemon.plugins.announce(&plugins::Happening { event: "started", ..Default::default() });
    let result = daemon.run(&mut queue);
    // Stopping takes freeing every wallpaper, player and plugin: none of them holds it up.
    daemon.watch.leaving();
    daemon.plugins.announce(&plugins::Happening { event: "stopping", ..Default::default() });
    // Wallpapers and surfaces go while the context is still there to free them with,
    // and so does everything else that holds what the context made: fields are
    // dropped in the order they are declared, and the context is declared first.
    daemon.slots.clear();
    daemon.devices.clear();
    daemon.display.heads.clear();
    let Daemon { engine, shaders, context, .. } = daemon;
    drop(shaders);
    drop(engine);
    drop(context);
    result
}

unsafe extern "C" {
    /// libwayland-client: dispatches what waits on a display's default queue, without reading.
    fn wl_display_dispatch_pending(display: *mut std::ffi::c_void) -> std::ffi::c_int;
}

impl Daemon {
    fn run(&mut self, queue: &mut EventQueue<Display>) -> Result<(), Failure> {
        self.watch.working();
        while !self.quit && !STOP.load(Ordering::Relaxed) {
            queue
                .dispatch_pending(&mut self.display)
                .map_err(|e| format!("the compositor went away: {e}"))?;
            // Our objects live on a queue of their own; what a library made on the
            // connection's first queue (the driver's buffers, since it stopped keeping a
            // queue for them) is read by nobody unless it is read here, and every event
            // sent to it would stay in memory for good. None of it has a listener.
            // SAFETY: the display is the live one of `self.connection`, used on this thread only.
            unsafe { wl_display_dispatch_pending(self.connection.backend().display_ptr().cast()) };
            self.follow_outputs();
            self.follow_devices();
            for incoming in self.server.receive() {
                let response = match incoming.request {
                    Ok(request) => self.handle(request, incoming.client),
                    Err(why) => wepaper_ipc::error(why),
                };
                self.server.answer(incoming.client, &response);
            }
            self.settle_sound();
            let wake = self.draw_due();
            self.server.flush();
            let _ = self.connection.flush();

            // Wait for the compositor, a client, a new video frame, or the next frame's time.
            let Some(guard) = queue.prepare_read() else { continue };
            let waker = self.engine.videos().waker().map(|w| w.fd());
            let mut fds: Vec<libc::pollfd> = std::iter::once(guard.connection_fd().as_raw_fd())
                .chain(self.server.fds())
                .chain(waker)
                .map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 })
                .collect();
            let timeout = wake
                .map_or(1000, |at| at.saturating_duration_since(Instant::now()).as_millis().min(1000) as i32);
            self.watch.resting();
            // SAFETY: `fds` is a live array of `fds.len()` pollfd structures.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            self.watch.working();
            if ready > 0 && fds[0].revents != 0 {
                guard.read().map_err(|e| format!("the compositor went away: {e}"))?;
            } else {
                drop(guard);
            }
            if let Some(waker) = self.engine.videos().waker() {
                waker.drain();
            }
        }
        Ok(())
    }

    /// Gives new outputs a surface and what they showed last, and lets go of unplugged ones.
    fn follow_outputs(&mut self) {
        let now = Instant::now();
        let gone: Vec<u32> = self
            .display
            .heads
            .iter()
            .filter(|head| head.removed || head.sheet.as_ref().is_some_and(|sheet| sheet.closed))
            .map(|head| head.id)
            .collect();
        for id in gone {
            self.slots.remove(&id);
            self.display.heads.retain(|head| head.id != id);
            self.release();
        }
        let fresh: Vec<(u32, String)> = self
            .display
            .heads
            .iter()
            .filter(|head| head.announced && head.sheet.is_none())
            .filter_map(|head| Some((head.id, head.name.clone()?)))
            .collect();
        for (id, name) in fresh {
            self.display.cover(id, self.settings.layer, self.settings.passthrough);
            let mut slot = Slot::new(now);
            slot.shown = self.state.outputs.get(&name).cloned().unwrap_or_else(|| self.state.last.clone());
            // Its wallpaper loads once the compositor has said how large the output is.
            slot.waiting = slot.shown.wallpaper.is_some();
            self.slots.insert(id, slot);
        }
        // Surfaces whose size or scale changed get buffers to match.
        for head in &mut self.display.heads {
            let Some(sheet) = head.sheet.as_mut().filter(|sheet| sheet.resized) else { continue };
            if !sheet.fit(&self.context) {
                continue;
            }
            if let Some(slot) = self.slots.get_mut(&head.id) {
                slot.stale = true;
            }
        }
        let waiting: Vec<(u32, String)> = self
            .display
            .heads
            .iter()
            .filter(|head| head.sheet.as_ref().is_some_and(|sheet| sheet.egl.is_some()))
            .filter(|head| self.slots.get(&head.id).is_some_and(|slot| slot.waiting))
            .map(|head| (head.id, head.name.clone().unwrap_or_default()))
            .collect();
        for (id, name) in waiting {
            let Some(slot) = self.slots.get_mut(&id) else { continue };
            slot.waiting = false;
            let shown = slot.shown.clone();
            if let Err(why) = self.load(id, &shown) {
                log::error!("{name}: {why}");
            }
        }
    }

    /// Makes the devices that are outputs match the devices there are: new ones get
    /// what they showed last, changed ones are sized or started anew, those that went
    /// or were turned off stop.
    fn follow_devices(&mut self) {
        if self.devices_dirty {
            self.devices_dirty = false;
            self.device_specs = device::specs(&self.device_config, &self.state.devices);
            let now = Instant::now();
            let gone: Vec<u32> = self
                .devices
                .iter()
                .filter(|device| !self.device_specs.get(&device.name).is_some_and(|spec| spec.enabled))
                .map(|device| device.id)
                .collect();
            for id in gone {
                if let Some(device) = self.devices.iter().find(|device| device.id == id) {
                    log::info!("{}: the device is no output any more", device.name);
                }
                self.devices.retain(|device| device.id != id);
                self.slots.remove(&id);
                self.release();
            }
            let wanted: Vec<(String, device::Spec)> = self
                .device_specs
                .iter()
                .filter(|(_, spec)| spec.enabled)
                .map(|(name, spec)| (name.clone(), spec.clone()))
                .collect();
            for (name, spec) in wanted {
                if self.display.heads.iter().any(|head| head.name.as_deref() == Some(name.as_str())) {
                    log::warn!("device {name}: a monitor has that name; the device is not made an output");
                    continue;
                }
                match self.devices.iter_mut().find(|device| device.name == name) {
                    Some(device) if device.spec == spec => {}
                    Some(device) => {
                        let resized = device.spec.size != spec.size;
                        if resized || device.spec.exec != spec.exec {
                            // Its program was started for the old size, or is another now.
                            device.stop();
                        }
                        device.spec = spec;
                        if let Some(slot) = self.slots.get_mut(&device.id) {
                            slot.stale = true;
                            // A plugin draws for the size and into the program it was
                            // started with: it is started again.
                            if slot.renderer.is_some() {
                                slot.renderer = None;
                                slot.waiting = slot.shown.wallpaper.is_some();
                            }
                            if resized {
                                // The wallpaper was loaded for the old size.
                                device.frame = None;
                                slot.staging = None;
                                slot.waiting = slot.shown.wallpaper.is_some();
                            }
                        }
                    }
                    None => {
                        let id = self.next_device;
                        self.next_device += 1;
                        log::info!(
                            "{name}: a device of {}x{} at {} frames a second",
                            spec.size.0,
                            spec.size.1,
                            spec.fps
                        );
                        let mut slot = Slot::new(now);
                        slot.shown =
                            self.state.outputs.get(&name).cloned().unwrap_or_else(|| self.state.last.clone());
                        slot.waiting = slot.shown.wallpaper.is_some();
                        self.slots.insert(id, slot);
                        self.devices.push(device::Device::new(id, name, spec));
                    }
                }
            }
        }
        let waiting: Vec<(u32, String)> = self
            .devices
            .iter()
            .filter(|device| self.slots.get(&device.id).is_some_and(|slot| slot.waiting))
            .map(|device| (device.id, device.name.clone()))
            .collect();
        for (id, name) in waiting {
            let Some(slot) = self.slots.get_mut(&id) else { continue };
            slot.waiting = false;
            let shown = slot.shown.clone();
            if let Err(why) = self.load(id, &shown) {
                log::error!("{name}: {why}");
            }
        }
    }

    /// The name of an output: a monitor's connector, or a device's.
    fn name_of(&self, id: u32) -> Option<String> {
        self.display
            .heads
            .iter()
            .find(|head| head.id == id)
            .and_then(|head| head.name.clone())
            .or_else(|| self.devices.iter().find(|device| device.id == id).map(|device| device.name.clone()))
    }

    /// The size an output is drawn at: a device's own, a surface's; a guess until
    /// the compositor has said.
    fn viewport(&self, id: u32) -> Viewport {
        if let Some(device) = self.devices.iter().find(|device| device.id == id) {
            return Viewport { width: device.spec.size.0, height: device.spec.size.1 };
        }
        let size = self
            .display
            .heads
            .iter()
            .find(|head| head.id == id)
            .and_then(|head| head.sheet.as_ref())
            .filter(|sheet| sheet.logical.0 > 0)
            .map(|sheet| sheet.pixels());
        let (width, height) = size.unwrap_or((1920, 1080));
        Viewport { width, height }
    }

    /// Loads what `shown` says for an output: the wallpaper, with its property values
    /// and with what was changed inside it. On failure the output keeps what it had.
    fn load(&mut self, id: u32, shown: &Shown) -> Result<(), String> {
        let Some(folder) = shown.wallpaper.as_deref() else {
            return Err("there is no wallpaper to load".to_owned());
        };
        let began = Instant::now();
        let properties: Vec<(String, wepaper_json::Value)> =
            shown.properties.iter().map(|(name, value)| (name.clone(), value.clone())).collect();
        let properties = properties.as_slice();
        // Videos are stored no wider than the widest output: nobody sees more.
        let widest = self
            .display
            .heads
            .iter()
            .filter_map(|head| head.sheet.as_ref())
            .filter(|sheet| sheet.logical.0 > 0)
            .map(|sheet| sheet.pixels().0)
            .max()
            .unwrap_or(0);
        let max_width =
            if self.settings.video_max_width > 0 { self.settings.video_max_width } else { widest };
        self.engine.videos().options.max_width = max_width;
        self.engine.videos().options.hardware_decode = self.settings.video_hardware;
        // A video starts as loud as this output has it: turned down to nothing, it is
        // not heard for the moment it takes to say so once it plays.
        let share = self.slots.get(&id).map_or(1.0, |slot| slot.shown.look.volume);
        self.engine.videos().options.volume = self.settings.volume * share;
        // Under a frame cap, a video's extra frames would be decoded for nothing.
        self.engine.videos().limits =
            wepaper_engine::video::Limits { max_width, max_fps: self.settings.fps as f32 };
        let viewport = self.viewport(id);
        let (_, project) = self.engine.open(folder).map_err(|e| e.to_string())?;
        let kind = project.kind.name();
        let name = self.name_of(id).unwrap_or_default();

        // A kind the engine does not draw goes to the plugin that does.
        let loaded = if let Some(manifest) = self.plugins.renderer_for(kind).cloned() {
            let mut arguments: Vec<String> =
                ["--dir", &folder.to_string_lossy(), "--file", &project.file].map(str::to_owned).into();
            // A device has no surface: the plugin draws for it unseen, at the device's
            // size and rate, and writes the frames to the device's program itself.
            let frames = match self.devices.iter_mut().find(|device| device.id == id) {
                Some(device) => {
                    // The plugin that drew here before goes first: while it holds the
                    // pipe, the program it fed does not see its input end.
                    if let Some(slot) = self.slots.get_mut(&id) {
                        slot.renderer = None;
                    }
                    let pipe = device.pipe(&self.socket).inspect_err(|why| {
                        if let Some(slot) = self.slots.get_mut(&id) {
                            slot.error = Some(why.clone());
                        }
                    })?;
                    arguments.extend([
                        "--frames".to_owned(),
                        format!("{}x{}", device.spec.size.0, device.spec.size.1),
                        "--frames-fd".to_owned(),
                        plugins::FRAMES_FD.to_string(),
                        "--fps".to_owned(),
                        device.spec.fps.to_string(),
                    ]);
                    Some(pipe)
                }
                None => {
                    arguments.extend(
                        [
                            "--output",
                            &name,
                            "--layer",
                            self.settings.layer.name(),
                            "--namespace",
                            wayland::NAMESPACE,
                            "--fps",
                            &self.settings.fps.to_string(),
                        ]
                        .map(str::to_owned),
                    );
                    if self.settings.passthrough {
                        arguments.push("--passthrough".to_owned());
                    }
                    None
                }
            };
            arguments.extend(["--volume".to_owned(), format!("{:.0}", self.settings.volume)]);
            // Silent until the sound is settled: of the outputs that show this
            // wallpaper, one is heard.
            arguments.push("--muted".to_owned());
            for (property, value) in properties {
                arguments.extend(["--property".to_owned(), format!("{property}={}", value.to_json())]);
            }
            self.plugins.render(&manifest, &arguments, frames).map(Err)
        } else {
            self.engine.load(folder, viewport, properties).map(Ok).map_err(|e| e.to_string())
        };
        let Some(slot) = self.slots.get_mut(&id) else { return Err("the output is gone".to_owned()) };
        match loaded {
            Ok(made) => {
                slot.clock = 0.0;
                slot.stale = true;
                slot.error = None;
                slot.loads += 1;
                slot.title = project.title.clone();
                match made {
                    Ok(mut wallpaper) => {
                        for (path, value) in &shown.edits {
                            // A wallpaper that changed since may not have what was edited.
                            if let Err(why) = wallpaper.edit(&wepaper_ipc::split_path(path), value) {
                                log::warn!("{name}: {path} is not set: {why}");
                            }
                        }
                        wallpaper.set_fill(slot.shown.fill.unwrap_or(self.settings.fill));
                        wallpaper.set_rate(slot.shown.look.rate);
                        wallpaper.set_frame(slot.shown.look.frame());
                        wallpaper.set_paused(slot.paused);
                        slot.wallpaper = Some(wallpaper);
                        slot.renderer = None;
                        // A device a plugin drew for: its program ends with the plugin,
                        // and the engine's frames start one anew.
                        if let Some(device) = self.devices.iter_mut().find(|device| device.id == id) {
                            device.unpipe();
                        }
                    }
                    Err(mut renderer) => {
                        if slot.paused {
                            renderer.send(&wepaper_json::obj! { "cmd" => "pause" });
                        }
                        // A device's plugin cuts its frames from the page itself.
                        if self.devices.iter().any(|device| device.id == id) {
                            renderer.send(
                                &slot.shown.look.plugin_frame(slot.shown.fill.unwrap_or(self.settings.fill)),
                            );
                        }
                        slot.stage = None;
                        slot.wallpaper = None;
                        slot.renderer = Some(renderer);
                        // Our own surface goes blank under the plugin's: a new one has nothing on it.
                        if let Some(head) = self.display.heads.iter_mut().find(|head| head.id == id) {
                            head.sheet = None;
                        }
                        self.display.cover(id, self.settings.layer, self.settings.passthrough);
                    }
                }
                // What the old wallpaper held alone is freed now, not at the next load.
                self.release();
                log::info!(
                    "{name}: playing \"{}\" ({}), loaded in {} ms",
                    project.title,
                    folder.display(),
                    began.elapsed().as_millis()
                );
                self.plugins.announce(&plugins::Happening {
                    event: "shown",
                    output: name,
                    wallpaper: folder.to_string_lossy().into_owned(),
                    title: project.title.clone(),
                    kind: kind.to_owned(),
                });
                Ok(())
            }
            Err(error) => {
                slot.error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Draws every output that is due a frame. Returns when to wake up for the next one, if a timer is what it waits for.
    fn draw_due(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let interval =
            (self.settings.fps > 0).then(|| Duration::from_secs_f64(1.0 / f64::from(self.settings.fps)));
        let mut wake: Option<Instant> = None;
        // Somebody listens to the system's sound only while a playing wallpaper reads the spectrum.
        let listen = self.settings.audio_response
            && self
                .slots
                .values()
                .any(|slot| !slot.paused && slot.wallpaper.as_ref().is_some_and(Wallpaper::wants_audio));
        self.engine.listen(listen, now.duration_since(self.last_round).as_secs_f32().min(0.1));
        // The same for the system's media player: followed while a script shows its track.
        let follow = self
            .slots
            .values()
            .any(|slot| !slot.paused && slot.wallpaper.as_ref().is_some_and(Wallpaper::wants_media));
        self.engine.follow_media(follow);
        self.last_round = now;
        // Once a second is plenty for looking after the videos' cheaper copies.
        if now.duration_since(self.last_upkeep) >= Duration::from_secs(1) {
            self.last_upkeep = now;
            self.optimising = self.engine.videos().optimise();
            self.plugins.upkeep();
            for device in &mut self.devices {
                device.upkeep();
            }
            if self.release_at.is_some_and(|at| now >= at) {
                self.release();
                self.release_at = None;
            }
            // A wallpaper that gave up (a video that cannot be played) is said to have:
            // the output's `error` names it instead of the tree calling it playing.
            for slot in self.slots.values_mut() {
                if let Some(why) = slot.wallpaper.as_ref().and_then(Wallpaper::failure)
                    && slot.error.is_none()
                {
                    slot.error = Some(why.to_owned());
                    log::error!("{why}");
                }
            }
            // A device's page is written for snapshots only while they are asked for.
            for slot in self.slots.values_mut() {
                if slot.stage.as_ref().is_some_and(|(_, until)| now >= *until)
                    && let Some((file, _)) = slot.stage.take()
                {
                    if let Some(renderer) = &mut slot.renderer {
                        renderer.send(
                            &wepaper_json::obj! { "cmd" => "stage", "file" => wepaper_json::Value::Null },
                        );
                    }
                    let _ = std::fs::remove_file(file);
                }
            }
            // A renderer plugin that died leaves its output empty, and says so.
            for slot in self.slots.values_mut() {
                if slot.renderer.as_mut().is_some_and(|renderer| !renderer.alive()) {
                    let plugin = slot.renderer.take().map(|r| r.plugin.clone()).unwrap_or_default();
                    slot.error = Some(format!("the {plugin} plugin stopped"));
                    log::error!("the {plugin} plugin stopped");
                }
            }
        }
        let ids: Vec<u32> = self.display.heads.iter().map(|head| head.id).collect();
        for id in ids {
            let Some(head) = self.display.heads.iter().find(|head| head.id == id) else { continue };
            let Some(sheet) = head.sheet.as_ref().filter(|sheet| sheet.egl.is_some() && !sheet.resized)
            else {
                continue;
            };
            let Some(slot) = self.slots.get_mut(&id) else { continue };
            let Some(wallpaper) = slot.wallpaper.as_mut() else { continue };
            if slot.paused && !slot.stale {
                continue;
            }
            if sheet.frame_pending && !slot.stale {
                continue;
            }
            if let Some(interval) = interval
                && !slot.stale
            {
                // A frame may come up to a millisecond early: a display at the cap's
                // own rate would otherwise miss every other refresh.
                if now + Duration::from_millis(1) < slot.due {
                    wake = Some(wake.map_or(slot.due, |at| at.min(slot.due)));
                    continue;
                }
                // Due times advance by whole intervals, so being late once is not
                // paid for again; after a long gap they start over from now.
                slot.due = (slot.due + interval).max(now);
            }
            if !slot.paused {
                // A long gap (a suspended laptop, a stalled compositor) does not fast-forward the scene.
                slot.clock += now.duration_since(slot.last_tick).as_secs_f64().min(0.25)
                    * f64::from(slot.shown.look.rate);
            }
            slot.last_tick = now;
            let egl = sheet.egl.as_ref().expect("checked above");
            if self.context.make_current(Some(egl)).is_err() {
                continue;
            }
            self.context.disable_vsync();
            let input = FrameInput {
                time: slot.clock,
                pointer: head.pointer,
                pointer_down: head.pointer_down,
                daytime: control::daytime(),
            };
            let size = sheet.pixels();
            let look = slot.shown.look;
            if let Some(pass) = &mut self.look_pass {
                pass.enabled = !look.plain_picture();
                look.apply(pass);
            }
            let shaded = self.shaders.iter().any(|(_, pass)| pass.enabled)
                || self.look_pass.as_ref().is_some_and(|pass| pass.enabled);
            if shaded {
                // The frame goes into a buffer first, then through the output shaders onto the surface.
                if slot.staging.as_ref().is_none_or(|t| (t.width(), t.height()) != size) {
                    slot.staging =
                        gpu::Target::new(self.engine.gl(), size.0, size.1, gpu::TextureFormat::Rgba8)
                            .inspect_err(|e| log::warn!("{e}"))
                            .ok();
                }
            } else {
                slot.staging = None;
            }
            let drew = match &slot.staging {
                Some(staging) => {
                    let drew = wallpaper.draw(
                        &mut self.engine,
                        &input,
                        Some(staging.framebuffer()),
                        size,
                        slot.stale,
                    );
                    if drew {
                        // The look first: output shaders work on what the output shows.
                        let passes: Vec<&wepaper_engine::post::PostPass> =
                            self.look_pass.iter().chain(self.shaders.iter().map(|(_, pass)| pass)).collect();
                        self.engine.post(&passes, staging, None, size, slot.clock as f32);
                    }
                    drew
                }
                None => wallpaper.draw(&mut self.engine, &input, None, size, slot.stale),
            };
            if !drew {
                continue; // A video without a new frame: what is on screen stands.
            }
            if log::log_enabled!(log::Level::Trace) {
                use glow::HasContext;
                let gl = self.engine.gl().raw();
                let mut pixel = [0u8; 4];
                // SAFETY: reads one pixel of the bound default framebuffer into a four-byte buffer.
                unsafe {
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
                    gl.read_pixels(
                        (size.0 / 2) as i32,
                        (size.1 / 2) as i32,
                        1,
                        1,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut pixel)),
                    );
                    log::trace!(
                        "frame on output {id}: centre pixel {pixel:?}, GL error {:#x}",
                        gl.get_error()
                    );
                }
            }
            slot.stale = false;
            slot.last_draw = now;
            if now.duration_since(slot.frames.0) >= Duration::from_secs(1) {
                slot.frames = (now, 0, slot.frames.1);
            }
            slot.frames.1 += 1;
            // The GPU finishes a heavy frame while we sleep, not while the driver spins
            // in the swap. Light frames are not worth the naps: they are asked about
            // once a second only, in case the scene has grown heavier.
            if slot.light_frames == 0 {
                let took = self.engine.gl().wait_for_gpu(Duration::from_millis(50));
                if took < Duration::from_micros(600) {
                    slot.light_frames = 60;
                }
            } else {
                slot.light_frames -= 1;
            }
            // The request for the next frame rides on the commit the swap makes.
            self.display.request_frame(id);
            if let Some(egl) = self
                .display
                .heads
                .iter()
                .find(|head| head.id == id)
                .and_then(|head| head.sheet.as_ref())
                .and_then(|sheet| sheet.egl.as_ref())
                && let Err(error) = self.context.present(egl)
            {
                log::warn!("{error}");
            }
            // Where the driver's library keeps a buffer release for every frame shown, a
            // surface that has shown its share gives way to a new one: the releases go
            // with it. The buffers are sized again before the next frame is drawn.
            if self.context.hoards_releases()
                && let Some(sheet) = self
                    .display
                    .heads
                    .iter_mut()
                    .find(|head| head.id == id)
                    .and_then(|head| head.sheet.as_mut())
                && sheet.shown_one()
                && self.context.make_current(None).is_ok()
            {
                sheet.renew();
            }
        }
        self.draw_devices(now, wake)
    }

    /// Draws every device that is due a frame and hands the frame to its program.
    /// A device has no display to ask for frames: its own rate is its clock.
    fn draw_devices(&mut self, now: Instant, mut wake: Option<Instant>) -> Option<Instant> {
        for index in 0..self.devices.len() {
            let device = &mut self.devices[index];
            let (id, size) = (device.id, device.spec.size);
            let Some(slot) = self.slots.get_mut(&id) else { continue };
            let Some(wallpaper) = slot.wallpaper.as_mut() else { continue };
            if !device::due(slot.paused, slot.stale, now, slot.due) {
                if !slot.paused || slot.stale {
                    wake = Some(wake.map_or(slot.due, |at| at.min(slot.due)));
                }
                continue;
            }
            let interval = Duration::from_secs_f64(1.0 / f64::from(device.spec.fps.max(1)));
            slot.due = (slot.due + interval).max(now);
            if !slot.paused {
                wake = Some(wake.map_or(slot.due, |at| at.min(slot.due)));
                slot.clock += now.duration_since(slot.last_tick).as_secs_f64().min(0.25)
                    * f64::from(slot.shown.look.rate);
            }
            slot.last_tick = now;
            if self.context.make_current(None).is_err() {
                continue;
            }
            let input = FrameInput {
                time: slot.clock,
                pointer: [0.5, 0.5],
                pointer_down: false,
                daytime: control::daytime(),
            };
            let look = slot.shown.look;
            if let Some(pass) = &mut self.look_pass {
                pass.enabled = !look.plain_picture();
                look.apply(pass);
            }
            let shaded = self.shaders.iter().any(|(_, pass)| pass.enabled)
                || self.look_pass.as_ref().is_some_and(|pass| pass.enabled);
            if shaded {
                if slot.staging.as_ref().is_none_or(|t| (t.width(), t.height()) != size) {
                    slot.staging =
                        gpu::Target::new(self.engine.gl(), size.0, size.1, gpu::TextureFormat::Rgba8)
                            .inspect_err(|e| log::warn!("{e}"))
                            .ok();
                }
            } else {
                slot.staging = None;
            }
            let Some(frame) = device.target(self.engine.gl()) else { continue };
            let drew = match &slot.staging {
                Some(staging) => {
                    let drew = wallpaper.draw(
                        &mut self.engine,
                        &input,
                        Some(staging.framebuffer()),
                        size,
                        slot.stale,
                    );
                    if drew {
                        let passes: Vec<&wepaper_engine::post::PostPass> =
                            self.look_pass.iter().chain(self.shaders.iter().map(|(_, pass)| pass)).collect();
                        self.engine.post(
                            &passes,
                            staging,
                            Some(frame.framebuffer()),
                            size,
                            slot.clock as f32,
                        );
                    }
                    drew
                }
                None => wallpaper.draw(&mut self.engine, &input, Some(frame.framebuffer()), size, slot.stale),
            };
            if !drew {
                continue; // A video without a new frame: the device keeps the one it has.
            }
            slot.stale = false;
            slot.last_draw = now;
            if now.duration_since(slot.frames.0) >= Duration::from_secs(1) {
                slot.frames = (now, 0, slot.frames.1);
            }
            slot.frames.1 += 1;
            let rgb = device::rgb_of(&frame.read_rgba());
            device.send(rgb, &self.socket);
        }
        wake
    }

    /// Settles the sound again when what the outputs show, or which of them play,
    /// is no longer what it was settled for.
    fn settle_sound(&mut self) {
        let now: Vec<sound::Playing> = self
            .display
            .heads
            .iter()
            .filter_map(|head| {
                let slot = self.slots.get(&head.id)?;
                let showing = slot.wallpaper.is_some() || slot.renderer.is_some();
                Some(sound::Playing {
                    id: head.id,
                    wallpaper: slot.shown.wallpaper.clone().filter(|_| showing),
                    paused: slot.paused,
                    loads: slot.loads,
                })
            })
            // After the monitors: a wallpaper that is on a monitor too is heard there.
            .chain(self.devices.iter().filter_map(|device| {
                let slot = self.slots.get(&device.id)?;
                Some(sound::Playing {
                    id: device.id,
                    wallpaper: slot.shown.wallpaper.clone().filter(|_| slot.wallpaper.is_some()),
                    paused: slot.paused,
                    loads: slot.loads,
                })
            }))
            .collect();
        if now != self.sounding {
            self.sounding = now;
            self.apply_sound();
        }
    }

    /// Applies the sound settings to everything that plays. A wallpaper on several
    /// outputs is heard on one of them.
    fn apply_sound(&mut self) {
        let (audio, volume) = (!self.settings.mute && self.settings.volume > 0.0, self.settings.volume);
        self.engine.videos().set_sound(audio, volume);
        let heard = sound::heard(&self.sounding);
        for (id, slot) in &mut self.slots {
            let on = audio && heard.contains(id);
            let volume = volume * slot.shown.look.volume;
            // A video on several outputs is one video: the output it is heard on sets it.
            if let Some(wallpaper) = &mut slot.wallpaper
                && (wallpaper.animates() || heard.contains(id))
            {
                wallpaper.set_sound(on, volume);
            }
            if let Some(renderer) = &mut slot.renderer {
                renderer.send(&wepaper_json::obj! { "cmd" => "volume", "value" => volume });
                renderer.send(&wepaper_json::obj! { "cmd" => "mute", "value" => !on });
            }
        }
    }

    /// Gives back what a wallpaper that went held: the engine's idle buffers and dead
    /// cache entries, and the allocator's free memory, to the system. Once now and once
    /// more in a moment: players are shut down on threads of their own, and what they
    /// held is free only when they are gone.
    fn release(&mut self) {
        self.engine.trim();
        // SAFETY: returns free memory to the system; no pointers are invalidated.
        unsafe { libc::malloc_trim(0) };
        self.release_at = Some(Instant::now() + Duration::from_secs(2));
    }

    /// Remembers what the outputs show, for the next run.
    fn save_state(&mut self) {
        for head in &self.display.heads {
            if let (Some(name), Some(slot)) = (&head.name, self.slots.get(&head.id)) {
                self.state.outputs.insert(name.clone(), slot.shown.clone());
            }
        }
        for device in &self.devices {
            if let Some(slot) = self.slots.get(&device.id) {
                self.state.outputs.insert(device.name.clone(), slot.shown.clone());
            }
        }
        self.state.save(&self.state_file);
    }
}
