//! `wepaper daemon`: the wallpapers on the desktop.
//!
//! One thread, one loop: it waits on the compositor, the control socket and the
//! video decoders' wake-up at once, and each time round it handles what came in and
//! draws the outputs that are due a frame.

mod control;
mod plugins;
mod server;
mod settings;
mod sound;
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
    /// By output id.
    slots: HashMap<u32, Slot>,
    /// When the outputs were last looked at for drawing.
    last_round: Instant,
    last_upkeep: Instant,
    /// How many videos are being re-encoded right now.
    optimising: usize,
    plugins: plugins::Plugins,
    /// What the outputs showed and which of them played when sound was last settled.
    sounding: Vec<sound::Playing>,
    /// The compiled shader plugins, by plugin id, in the order they run.
    shaders: Vec<(String, wepaper_engine::post::PostPass)>,
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
    control::read_config(&mut settings, &config_file);
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
        last_round: now,
        last_upkeep: now,
        optimising: 0,
        plugins: plugins::Plugins::new(found, socket),
        sounding: Vec::new(),
        shaders,
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
    daemon.plugins.announce(&plugins::Happening { event: "stopping", ..Default::default() });
    // Wallpapers and surfaces go while the context is still there to free them with,
    // and so does everything else that holds what the context made: fields are
    // dropped in the order they are declared, and the context is declared first.
    daemon.slots.clear();
    daemon.display.heads.clear();
    let Daemon { engine, shaders, context, .. } = daemon;
    drop(shaders);
    drop(engine);
    drop(context);
    result
}

impl Daemon {
    fn run(&mut self, queue: &mut EventQueue<Display>) -> Result<(), Failure> {
        while !self.quit && !STOP.load(Ordering::Relaxed) {
            queue
                .dispatch_pending(&mut self.display)
                .map_err(|e| format!("the compositor went away: {e}"))?;
            self.follow_outputs();
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
            // SAFETY: `fds` is a live array of `fds.len()` pollfd structures.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
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
            self.engine.trim();
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

    /// The size the output's surface is drawn at; a guess until the compositor has said.
    fn viewport(&self, id: u32) -> Viewport {
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
        // Under a frame cap, a video's extra frames would be decoded for nothing.
        self.engine.videos().limits =
            wepaper_engine::video::Limits { max_width, max_fps: self.settings.fps as f32 };
        let viewport = self.viewport(id);
        let (_, project) = self.engine.open(folder).map_err(|e| e.to_string())?;
        let kind = project.kind.name();
        let name = self
            .display
            .heads
            .iter()
            .find(|head| head.id == id)
            .and_then(|head| head.name.clone())
            .unwrap_or_default();

        // A kind the engine does not draw goes to the plugin that does.
        let loaded = if let Some(manifest) = self.plugins.renderer_for(kind).cloned() {
            let mut arguments: Vec<String> = [
                "--dir",
                &folder.to_string_lossy(),
                "--file",
                &project.file,
                "--output",
                &name,
                "--layer",
                self.settings.layer.name(),
                "--namespace",
                wayland::NAMESPACE,
            ]
            .map(str::to_owned)
            .into();
            arguments.extend([
                "--fps".to_owned(),
                self.settings.fps.to_string(),
                "--volume".to_owned(),
                format!("{:.0}", self.settings.volume),
            ]);
            // Silent until the sound is settled: of the outputs that show this
            // wallpaper, one is heard.
            arguments.push("--muted".to_owned());
            if self.settings.passthrough {
                arguments.push("--passthrough".to_owned());
            }
            for (property, value) in properties {
                arguments.extend(["--property".to_owned(), format!("{property}={}", value.to_json())]);
            }
            self.plugins.render(&manifest, &arguments).map(Err)
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
                        wallpaper.set_paused(slot.paused);
                        slot.wallpaper = Some(wallpaper);
                        slot.renderer = None;
                    }
                    Err(mut renderer) => {
                        if slot.paused {
                            renderer.send(&wepaper_json::obj! { "cmd" => "pause" });
                        }
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
                self.engine.trim();
                // SAFETY: returns memory the loader freed to the system; no pointers are invalidated.
                unsafe { libc::malloc_trim(0) };
                log::info!("{name}: playing \"{}\" ({})", project.title, folder.display());
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
                slot.clock += now.duration_since(slot.last_tick).as_secs_f64().min(0.25);
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
            let shaded = self.shaders.iter().any(|(_, pass)| pass.enabled);
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
                        let passes: Vec<&wepaper_engine::post::PostPass> =
                            self.shaders.iter().map(|(_, pass)| pass).collect();
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
            if let Some(wallpaper) = &mut slot.wallpaper {
                wallpaper.set_sound(on, volume);
            }
            if let Some(renderer) = &mut slot.renderer {
                renderer.send(&wepaper_json::obj! { "cmd" => "volume", "value" => volume });
                renderer.send(&wepaper_json::obj! { "cmd" => "mute", "value" => !on });
            }
        }
    }

    /// Remembers what the outputs show, for the next run.
    fn save_state(&mut self) {
        for head in &self.display.heads {
            if let (Some(name), Some(slot)) = (&head.name, self.slots.get(&head.id)) {
                self.state.outputs.insert(name.clone(), slot.shown.clone());
            }
        }
        self.state.save(&self.state_file);
    }
}
