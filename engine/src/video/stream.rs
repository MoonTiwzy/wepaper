//! One playing video.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::gpu::{Gl, MemoryClass, Target, Texture, TextureFormat};

use super::mpv::{self, Api};

/// Why a video does not play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoError(pub String);

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VideoError {}

/// Something a thread can poke to wake the loop that draws: an eventfd.
///
/// libmpv says "a new frame is ready" from its own threads; the daemon sleeps in
/// `poll` on this descriptor among others.
#[derive(Clone)]
pub struct Waker(Arc<OwnedFd>);

impl Waker {
    /// A new waker.
    pub fn new() -> std::io::Result<Self> {
        // SAFETY: plain syscall; the descriptor it returns is owned by nothing else.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh, valid descriptor.
        Ok(Self(Arc::new(unsafe { OwnedFd::from_raw_fd(fd) })))
    }

    /// The descriptor to wait on; it reads as ready after a wake.
    pub fn fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    /// Wakes whoever waits on the descriptor.
    pub fn wake(&self) {
        let one = 1u64.to_ne_bytes();
        // SAFETY: writes eight bytes from a live buffer to a descriptor this object owns.
        unsafe { libc::write(self.fd(), one.as_ptr().cast(), 8) };
    }

    /// Clears the wake, so the descriptor is quiet until the next one.
    pub fn drain(&self) {
        let mut buffer = [0u8; 8];
        // SAFETY: reads eight bytes into a live buffer from a descriptor this object owns.
        unsafe { libc::read(self.fd(), buffer.as_mut_ptr().cast(), 8) };
    }
}

/// How a video is played.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoOptions {
    /// Decode on the GPU. Off by default: a decoder session costs more video memory
    /// than the frames themselves.
    pub hardware_decode: bool,
    /// Frames are drawn no wider than this many pixels; 0 for the video's own size.
    pub max_width: u32,
    /// Play the sound track.
    pub audio: bool,
    /// Volume, 0 to 100.
    pub volume: f32,
    /// Decoder threads; each holds frames, so fewer means less memory.
    pub decode_threads: u32,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self { hardware_decode: false, max_width: 0, audio: false, volume: 100.0, decode_threads: 3 }
    }
}

/// What libmpv's threads share with the stream.
struct Signal {
    fresh: AtomicBool,
    waker: Option<Waker>,
}

/// A playing video and the texture its current frame is in.
pub struct VideoStream {
    api: &'static Api,
    gl: Gl,
    handle: *mut mpv::Handle,
    render: *mut mpv::RenderContext,
    /// Leaked into libmpv's callback for as long as `render` lives.
    signal: *const Signal,
    target: Option<Target>,
    max_width: u32,
    /// Who holds the stream, and whether each wants it paused.
    holders: Vec<(u64, bool)>,
    paused: bool,
    ended: bool,
    /// Counts the frames drawn into the texture.
    serial: u64,
}

unsafe extern "C" fn proc_address(context: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: `context` is the `Gl` handed to libmpv at creation, which outlives the
    // render context; `name` is a NUL-terminated string from libmpv.
    unsafe {
        let gl = &*(context as *const Gl);
        gl.proc_address(CStr::from_ptr(name)) as *mut c_void
    }
}

unsafe extern "C" fn on_update(context: *mut c_void) {
    // SAFETY: `context` is the `Signal` leaked at creation and freed only after the
    // callback is unset.
    let signal = unsafe { &*(context as *const Signal) };
    signal.fresh.store(true, Ordering::Release);
    if let Some(waker) = &signal.waker {
        waker.wake();
    }
}

impl VideoStream {
    /// Starts playing `path` in a loop. `waker` is poked from libmpv's threads whenever a new frame is ready.
    pub fn open(gl: &Gl, path: &Path, options: &VideoOptions, waker: Option<Waker>) -> Result<Self, VideoError> {
        let api = mpv::api().ok_or_else(|| VideoError("libmpv is not installed".to_owned()))?;
        let text = |value: &str| CString::new(value).map_err(|_| VideoError("a NUL byte in a video option".to_owned()));
        // SAFETY: every pointer handed to libmpv is a live NUL-terminated string or a
        // struct that outlives the call; results are checked before use.
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                return Err(VideoError("libmpv could not create a player".to_owned()));
            }
            let fail = |code: c_int, what: &str| VideoError(format!("{what}: {}", CStr::from_ptr((api.error_string)(code)).to_string_lossy()));
            let threads = options.decode_threads.clamp(1, 16).to_string();
            let volume = format!("{:.0}", options.volume.clamp(0.0, 100.0));
            let settings: &[(&str, &str)] = &[
                ("vo", "libmpv"),
                ("hwdec", if options.hardware_decode { "auto-safe" } else { "no" }),
                ("loop-file", "inf"),
                ("terminal", "no"),
                ("msg-level", "all=no"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("osc", "no"),
                ("osd-level", "0"),
                ("sid", "no"),
                ("aid", if options.audio { "auto" } else { "no" }),
                ("volume", &volume),
                ("audio-client-name", "wepaper"),
                // The plainest renderer: planes in, one pass, no intermediate buffers.
                ("gpu-dumb-mode", "yes"),
                ("dither", "no"),
                // A wallpaper loops a local file: reading ahead only costs memory.
                ("cache", "no"),
                ("demuxer-max-bytes", "4MiB"),
                ("demuxer-max-back-bytes", "0"),
                ("demuxer-readahead-secs", "0"),
                ("vd-lavc-threads", &threads),
                ("load-scripts", "no"),
                ("config", "no"),
                ("ytdl", "no"),
            ];
            for (name, value) in settings {
                let (name_c, value_c) = (text(name)?, text(value)?);
                // An option this libmpv does not know is not worth failing over.
                let _ = (api.set_option_string)(handle, name_c.as_ptr(), value_c.as_ptr());
            }
            let code = (api.initialize)(handle);
            if code < 0 {
                (api.terminate_destroy)(handle);
                return Err(fail(code, "libmpv did not start"));
            }

            // The `Gl` clone libmpv resolves functions through lives as long as the stream.
            let gl_box = Box::into_raw(Box::new(gl.clone()));
            let mut init = mpv::OpenglInitParams { get_proc_address: proc_address, get_proc_address_ctx: gl_box.cast() };
            let api_type = c"opengl";
            let mut params = [
                mpv::RenderParam { kind: mpv::RENDER_PARAM_API_TYPE, data: api_type.as_ptr() as *mut c_void },
                mpv::RenderParam { kind: mpv::RENDER_PARAM_OPENGL_INIT_PARAMS, data: (&raw mut init).cast() },
                mpv::RenderParam { kind: mpv::RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
            ];
            let mut render = std::ptr::null_mut();
            let code = (api.render_context_create)(&mut render, handle, params.as_mut_ptr());
            // libmpv copied what it needs; the context pointer is only read during creation.
            drop(Box::from_raw(gl_box));
            gl.invalidate();
            if code < 0 {
                (api.terminate_destroy)(handle);
                return Err(fail(code, "libmpv could not render with this OpenGL context"));
            }
            let signal = Box::into_raw(Box::new(Signal { fresh: AtomicBool::new(false), waker }));
            (api.render_context_set_update_callback)(render, Some(on_update), signal.cast());

            let stream = Self { api, gl: gl.clone(), handle, render, signal, target: None, max_width: options.max_width, holders: Vec::new(), paused: false, ended: false, serial: 0 };
            let file = text(&path.to_string_lossy())?;
            let mut command = [c"loadfile".as_ptr(), file.as_ptr(), std::ptr::null()];
            let code = (api.command)(handle, command.as_mut_ptr());
            if code < 0 {
                return Err(fail(code, &format!("{} did not open", path.display())));
            }
            Ok(stream)
        }
    }

    fn property_i64(&self, name: &CStr) -> Option<i64> {
        let mut value = 0i64;
        // SAFETY: `value` is an i64, which is what FORMAT_INT64 writes.
        let code = unsafe { (self.api.get_property)(self.handle, name.as_ptr(), mpv::FORMAT_INT64, (&raw mut value).cast()) };
        (code >= 0).then_some(value)
    }

    fn property_f64(&self, name: &CStr) -> Option<f64> {
        let mut value = 0f64;
        // SAFETY: `value` is an f64, which is what FORMAT_DOUBLE writes.
        let code = unsafe { (self.api.get_property)(self.handle, name.as_ptr(), mpv::FORMAT_DOUBLE, (&raw mut value).cast()) };
        (code >= 0).then_some(value)
    }

    fn set(&self, name: &CStr, value: &str) {
        if let Ok(value) = CString::new(value) {
            // SAFETY: both strings are NUL-terminated and live for the call.
            unsafe { (self.api.set_property_string)(self.handle, name.as_ptr(), value.as_ptr()) };
        }
    }

    /// Draws the newest frame into the texture if there is one. Returns whether the
    /// texture changed. Call once a frame, on the render thread.
    pub fn poll(&mut self) -> bool {
        // SAFETY: the handle is alive; a zero timeout never blocks. Events are read to
        // keep libmpv's queue empty and to notice the end of a file that cannot loop.
        unsafe {
            loop {
                let event = &*(self.api.wait_event)(self.handle, 0.0);
                match event.event_id {
                    mpv::EVENT_NONE => break,
                    mpv::EVENT_SHUTDOWN | mpv::EVENT_END_FILE => self.ended = event.event_id == mpv::EVENT_SHUTDOWN || event.error < 0,
                    _ => {}
                }
            }
            // SAFETY: the signal lives until the stream is dropped.
            if !(*self.signal).fresh.swap(false, Ordering::AcqRel) {
                return false;
            }
            if (self.api.render_context_update)(self.render) & mpv::RENDER_UPDATE_FRAME == 0 {
                return false;
            }
        }
        let (Some(width), Some(height)) = (self.property_i64(c"dwidth"), self.property_i64(c"dheight")) else { return false };
        if width <= 0 || height <= 0 {
            return false;
        }
        // Frames are stored no larger than they will be shown.
        let (mut w, mut h) = (width as u32, height as u32);
        if self.max_width > 0 && w > self.max_width {
            h = ((u64::from(h) * u64::from(self.max_width)) / u64::from(w)).max(1) as u32;
            w = self.max_width;
        }
        if self.target.as_ref().is_none_or(|t| t.width() != w || t.height() != h) {
            self.target = None;
            match Target::new(&self.gl, w, h, TextureFormat::Rgba8) {
                Ok(target) => {
                    // A video's frames count as video memory, not as an effect buffer.
                    self.gl.reclassify(MemoryClass::Targets, MemoryClass::Video, target.texture.bytes());
                    self.target = Some(target);
                }
                Err(error) => {
                    log::warn!("video frame of {w}x{h}: {error}");
                    return false;
                }
            }
        }
        let Some(target) = &self.target else { return false };
        // libmpv's own "top" is our texture's first row, as with pictures from files.
        let mut fbo = mpv::OpenglFbo { fbo: target.framebuffer().0.get() as c_int, w: w as c_int, h: h as c_int, internal_format: 0 };
        let mut flip: c_int = 0;
        let mut params = [
            mpv::RenderParam { kind: mpv::RENDER_PARAM_OPENGL_FBO, data: (&raw mut fbo).cast() },
            mpv::RenderParam { kind: mpv::RENDER_PARAM_FLIP_Y, data: (&raw mut flip).cast() },
            mpv::RenderParam { kind: mpv::RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        // SAFETY: the render context is alive and our GL context is current; the
        // parameters point at locals that outlive the call.
        unsafe { (self.api.render_context_render)(self.render, params.as_mut_ptr()) };
        // libmpv left the context's state as it pleased.
        self.gl.invalidate();
        self.serial += 1;
        true
    }

    /// How many frames have been drawn so far: holders compare it to know a new one came.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// The current frame, once there is one.
    pub fn texture(&self) -> Option<&Texture> {
        self.target.as_ref().map(|t| &t.texture)
    }

    /// The size frames are stored at; `None` before the first frame.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.target.as_ref().map(|t| (t.width(), t.height()))
    }

    /// Whether the file stopped and will not play again (it could not be read).
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// The video's frame rate, when libmpv knows it.
    pub fn frame_rate(&self) -> Option<f64> {
        self.property_f64(c"container-fps").filter(|fps| *fps > 0.0)
    }

    /// Registers a holder and returns its key for [`hold_paused`](Self::hold_paused).
    pub fn join(&mut self) -> u64 {
        let key = self.holders.iter().map(|(k, _)| k + 1).max().unwrap_or(1);
        self.holders.push((key, false));
        self.apply_pause();
        key
    }

    /// Removes a holder.
    pub fn leave(&mut self, key: u64) {
        self.holders.retain(|(k, _)| *k != key);
        self.apply_pause();
    }

    /// Says whether one holder wants the video paused. It pauses only when all do:
    /// freezing one output must not freeze the video on another.
    pub fn hold_paused(&mut self, key: u64, paused: bool) {
        if let Some(holder) = self.holders.iter_mut().find(|(k, _)| *k == key) {
            holder.1 = paused;
        }
        self.apply_pause();
    }

    fn apply_pause(&mut self) {
        let paused = !self.holders.is_empty() && self.holders.iter().all(|(_, paused)| *paused);
        if paused != self.paused {
            self.paused = paused;
            self.set(c"pause", if paused { "yes" } else { "no" });
        }
    }

    /// Sets the volume, 0 to 100.
    pub fn set_volume(&self, volume: f32) {
        self.set(c"volume", &format!("{:.0}", volume.clamp(0.0, 100.0)));
    }

    /// Switches the sound track on or off. Off, it is not decoded at all.
    pub fn set_audio(&self, on: bool) {
        self.set(c"aid", if on { "auto" } else { "no" });
    }

    /// Plays faster or slower; 1 is normal.
    pub fn set_speed(&self, speed: f32) {
        self.set(c"speed", &format!("{:.3}", speed.clamp(0.01, 100.0)));
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        // SAFETY: both pointers are alive; the renderer goes before the player, the
        // callback is unset before its data is freed, and nothing uses them after.
        unsafe {
            (self.api.render_context_set_update_callback)(self.render, None, std::ptr::null_mut());
            (self.api.render_context_free)(self.render);
            (self.api.terminate_destroy)(self.handle);
            drop(Box::from_raw(self.signal as *mut Signal));
        }
        if let Some(target) = &self.target {
            self.gl.reclassify(MemoryClass::Video, MemoryClass::Targets, target.texture.bytes());
        }
        self.gl.invalidate();
    }
}
