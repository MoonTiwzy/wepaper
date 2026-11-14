//! Web wallpapers for wepaper: a renderer plugin on WebKitGTK.
//!
//! One process shows one web wallpaper (a folder with a page in it) as the background
//! of one output. The wepaper daemon starts it, sends commands as JSON lines on stdin
//! and reads events from stdout; `plugins/web/README.md` has the contract.
//!
//! - [`args`]: the command line.
//! - [`protocol`]: the lines exchanged with the daemon.
//! - [`properties`]: user property values, from `project.json` and the command line.
//! - [`bridge`]: Wallpaper Engine's JavaScript API inside the page, and the messages to it.
//! - [`page`]: the web view.
//! - [`surface`]: the layer-shell window.
//! - [`frames`]: the page for a device: no surface anybody sees, its frames written out.

mod args;
mod bridge;
mod frames;
mod log;
mod page;
mod properties;
mod protocol;
mod surface;

use std::cell::{Cell, RefCell};
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;

use gtk::prelude::*;
use gtk::{gdk, glib};

use args::{Args, Invocation};
use bridge::{FromPage, PageState};
use page::{Location, Page, PageEvent};
use protocol::{Command, Event};
use surface::SurfaceOptions;

/// The exit code for an output that is not there (the daemon tells it from other failures).
const NO_SUCH_OUTPUT: u8 = 2;

/// How many times a page whose process died is loaded again before giving up.
const CRASHES_FORGIVEN: u32 = 3;

/// Why the plugin cannot go on, and the code to exit with.
struct Fatal {
    message: String,
    code: u8,
}

impl Fatal {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: 1 }
    }
}

fn main() -> ExitCode {
    page::prepare_environment();
    let args = match args::parse(std::env::args_os().skip(1)) {
        Ok(Invocation::Run(args)) => args,
        Ok(Invocation::Help) => {
            // Not stdout: that is the event stream.
            let _ = writeln!(std::io::stderr(), "{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(message) => return fail(&Fatal::new(format!("{message} (see --help)"))),
    };
    match run(args) {
        Ok(code) => ExitCode::from(code),
        Err(fatal) => fail(&fatal),
    }
}

/// Tells the daemon and the log why the plugin stops.
fn fail(fatal: &Fatal) -> ExitCode {
    emit(&Event::Error(fatal.message.clone()));
    log::error(&fatal.message);
    ExitCode::from(fatal.code)
}

/// Writes an event line. A daemon that is gone shows as EOF on stdin, so a failed
/// write here is not acted on.
fn emit(event: &Event) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", event.encode());
    let _ = stdout.flush();
}

/// One wallpaper on one output, from start to the exit code.
fn run(args: Args) -> Result<u8, Fatal> {
    // Before GTK starts: a wallpaper that is not there should not cost a connection
    // to the compositor.
    let location = Location::find(&args.dir, &args.file).map_err(Fatal::new)?;
    let display = surface::connect().map_err(Fatal::new)?;

    let monitor = match (&args.frames, args.output.is_empty()) {
        // A device's page: any output holds the window nobody sees.
        (Some(_), true) => wait_for_any_monitor(&display),
        _ => wait_for_monitor(&display, &args.output),
    };
    let monitor = monitor.ok_or_else(|| Fatal {
        message: format!(
            "no output named {}; there are: {}",
            args.output,
            surface::connectors(&display).join(", ")
        ),
        code: NO_SUCH_OUTPUT,
    })?;

    let mut values = properties::load(&args.dir).unwrap_or_else(|problem| {
        log::warning(&format!("no user properties: {problem}"));
        Default::default()
    });
    properties::merge(&mut values, args.properties.iter().map(|(name, value)| (name.as_str(), value)));

    let refresh = surface::refresh_rate(&monitor);
    let session = Rc::new(Session {
        page: Page::new(location),
        state: RefCell::new(PageState {
            properties: values,
            fps: args.fps.unwrap_or(refresh),
            limit: args.fps.unwrap_or(0),
            paused: false,
            volume: args.volume,
            unseen: args.frames.is_some(),
        }),
        refresh,
        main_loop: glib::MainLoop::new(None, false),
        exit_code: Cell::new(0),
        loaded: Cell::new(false),
        mapped: Cell::new(false),
        ready: Cell::new(false),
        audio_wanted: Cell::new(false),
        crashes: Cell::new(0),
        frames_paused: Rc::new(Cell::new(false)),
    });
    session.page.set_muted(args.muted);

    let window = match args.frames {
        Some(frames) => frames::hidden_window(&monitor, frames.size, session.page.widget()),
        None => surface::create(
            &SurfaceOptions {
                monitor: &monitor,
                layer: args.layer,
                namespace: &args.namespace,
                passthrough: args.passthrough,
            },
            session.page.widget(),
        ),
    };
    // A device's frames: taken from the page for as long as the session lasts.
    let _capture = args.frames.map(|frames| {
        let weak = Rc::downgrade(&session);
        let paused = session.frames_paused.clone();
        frames::Capture::start(session.page.widget(), frames, args.fps.unwrap_or(30), paused, move || {
            if let Some(session) = weak.upgrade() {
                session.stop_with(Fatal::new("the device's program stopped taking frames"));
            }
        })
    });

    let weak = Rc::downgrade(&session);
    session.page.on_event(move |event| {
        if let Some(session) = weak.upgrade() {
            session.page_event(event);
        }
    });
    let weak = Rc::downgrade(&session);
    window.connect_map(move |_| {
        if let Some(session) = weak.upgrade() {
            session.mapped.set(true);
            session.announce_ready();
        }
    });
    // GTK would move the window to another output; this process is for one only.
    let weak = Rc::downgrade(&session);
    let output = args.output.clone();
    monitor.connect_invalidate(move |_| {
        if let Some(session) = weak.upgrade() {
            session.stop_with(Fatal { message: format!("output {output} is gone"), code: NO_SUCH_OUTPUT });
        }
    });

    let weak = Rc::downgrade(&session);
    read_stdin(move |input| {
        if let Some(session) = weak.upgrade() {
            session.input(input);
        }
    });

    session.page.load(&session.state.borrow());
    window.present();
    log::info(&format!("{} on {}", args.dir.join(&args.file).display(), args.output));
    session.main_loop.run();
    window.destroy();
    Ok(session.exit_code.get())
}

/// The output called `name`. The compositor names its outputs in a round trip after
/// the first, so an output that is not there yet gets a moment to appear.
fn wait_for_monitor(display: &gdk::Display, name: &str) -> Option<gdk::Monitor> {
    let context = glib::MainContext::default();
    for _ in 0..40 {
        if let Some(monitor) = surface::monitor_named(display, name) {
            return Some(monitor);
        }
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Any output, for a window nobody sees; one that is not there yet gets a moment.
fn wait_for_any_monitor(display: &gdk::Display) -> Option<gdk::Monitor> {
    let context = glib::MainContext::default();
    for _ in 0..40 {
        if let Some(name) = surface::connectors(display).first()
            && let Some(monitor) = surface::monitor_named(display, name)
        {
            return Some(monitor);
        }
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// What stdin brought.
enum Input {
    /// One line.
    Line(String),
    /// The end: the daemon closed the pipe or is gone.
    Closed,
}

/// What the main thread does with what stdin brought.
type InputHandler = Box<dyn Fn(Input)>;

thread_local! {
    /// Where the main thread takes stdin's lines; the reading thread only posts them.
    static INBOX: RefCell<Option<InputHandler>> = const { RefCell::new(None) };
}

/// Hands every line of stdin to `handler` on the main loop.
///
/// A thread does the blocking reads: GLib can watch a pipe, but stdin may as well be
/// a file or a terminal, and a thread reads all three.
fn read_stdin(handler: impl Fn(Input) + 'static) {
    INBOX.with(|inbox| *inbox.borrow_mut() = Some(Box::new(handler)));
    let deliver = |input: Input| {
        glib::MainContext::default().invoke(move || {
            INBOX.with(|inbox| {
                if let Some(handler) = inbox.borrow().as_ref() {
                    handler(input);
                }
            });
        });
    };
    let reader = move || {
        let mut stdin = std::io::stdin().lock();
        let mut line = Vec::new();
        loop {
            line.clear();
            match stdin.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => deliver(Input::Line(String::from_utf8_lossy(&line).into_owned())),
            }
        }
        deliver(Input::Closed);
    };
    if let Err(error) = std::thread::Builder::new().name("stdin".to_owned()).spawn(reader) {
        log::warning(&format!("stdin is not read, commands will not arrive: {error}"));
    }
}

/// A wallpaper being shown: the page, the state it should be in, and how far along it is.
struct Session {
    page: Page,
    /// What the page was told last; a page loaded anew starts from it.
    state: RefCell<PageState>,
    /// The output's refresh rate: the frame rate a page is told when there is no cap.
    refresh: u32,
    main_loop: glib::MainLoop,
    exit_code: Cell<u8>,
    loaded: Cell<bool>,
    mapped: Cell<bool>,
    /// Whether the daemon has been told the wallpaper is up.
    ready: Cell<bool>,
    /// Whether the page listens to the audio spectrum.
    audio_wanted: Cell<bool>,
    crashes: Cell<u32>,
    /// For a device: no frames are taken while the wallpaper is paused.
    frames_paused: Rc<Cell<bool>>,
}

impl Session {
    fn input(&self, input: Input) {
        let line = match input {
            Input::Line(line) => line,
            Input::Closed => return self.stop(),
        };
        if line.trim().is_empty() {
            return;
        }
        match Command::decode(&line) {
            Ok(command) => self.command(command),
            Err(problem) => {
                log::warning(&format!("a line on stdin was not understood ({problem}): {}", line.trim()))
            }
        }
    }

    fn command(&self, command: Command) {
        match command {
            Command::Properties(changed) => {
                properties::merge(&mut self.state.borrow_mut().properties, changed.iter());
                self.page.run(&bridge::properties_changed(&changed));
            }
            Command::Pause => self.set_paused(true),
            Command::Resume => self.set_paused(false),
            Command::Fps(cap) => {
                let (fps, limit) = if cap == 0 { (self.refresh, 0) } else { (cap, cap) };
                let mut state = self.state.borrow_mut();
                (state.fps, state.limit) = (fps, limit);
                self.page.run(&bridge::frame_rate(fps, limit));
            }
            Command::Volume(percent) => {
                self.state.borrow_mut().volume = percent;
                self.page.run(&bridge::volume(percent));
            }
            Command::Mute(muted) => self.page.set_muted(muted),
            Command::Audio(samples) => {
                // Frames nobody listens to are dropped here, not parsed by the page.
                if self.audio_wanted.get() && !self.state.borrow().paused {
                    self.page.run(&bridge::audio(&samples));
                }
            }
            Command::Quit => self.stop(),
        }
    }

    fn set_paused(&self, paused: bool) {
        self.frames_paused.set(paused);
        self.state.borrow_mut().paused = paused;
        self.page.run(&bridge::paused(paused));
    }

    fn page_event(&self, event: PageEvent) {
        match event {
            PageEvent::Loaded => {
                self.loaded.set(true);
                self.announce_ready();
            }
            PageEvent::Failed(problem) => self.stop_with(Fatal::new(problem)),
            PageEvent::Crashed(problem) => {
                let crashes = self.crashes.get() + 1;
                self.crashes.set(crashes);
                if crashes > CRASHES_FORGIVEN {
                    return self.stop_with(Fatal::new(format!("{problem}, {crashes} times: giving up")));
                }
                emit(&Event::Error(format!("{problem}; loading the page again")));
                log::warning(&format!("{problem}; loading the page again"));
                self.page.load(&self.state.borrow());
            }
            PageEvent::Message(FromPage::Hello) => {
                // A new document has no listener yet, whatever the last one had.
                self.set_audio_wanted(false);
                self.page.run(&bridge::init(&self.state.borrow()));
            }
            PageEvent::Message(FromPage::AudioWanted(wanted)) => self.set_audio_wanted(wanted),
            PageEvent::Message(FromPage::Log(text)) => log::warning(&format!("page: {text}")),
        }
    }

    fn set_audio_wanted(&self, wanted: bool) {
        if self.audio_wanted.replace(wanted) != wanted {
            emit(&Event::Audio { wanted });
        }
    }

    /// Tells the daemon the wallpaper is up, once the page has loaded and the surface is mapped.
    fn announce_ready(&self) {
        if self.loaded.get() && self.mapped.get() && !self.ready.replace(true) {
            emit(&Event::Ready);
        }
    }

    fn stop(&self) {
        self.main_loop.quit();
    }

    fn stop_with(&self, fatal: Fatal) {
        emit(&Event::Error(fatal.message.clone()));
        log::error(&fatal.message);
        self.exit_code.set(fatal.code);
        self.main_loop.quit();
    }
}
