//! The player behind a scene's sound layer.

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};

use crate::video::mpv::{self, Api};

/// Sound files playing through libmpv, without video.
///
/// Nothing here waits for the player. libmpv's plain calls return when the player's
/// own thread has got to them, and that thread can stand in the audio output for as
/// long as the sound server lets it: the engine, which draws on the thread that calls,
/// would stand with it. So a change is sent and not waited for, and a player is shut
/// down on a thread of its own.
pub struct Sound {
    api: &'static Api,
    handle: *mut mpv::Handle,
    /// The volume and the pause last sent: only a change is sent.
    sent: (Option<String>, Option<bool>),
    looping: bool,
}

/// The options of a player that only plays sound.
const SETTINGS: &[(&CStr, &CStr)] = &[
    (c"vid", c"no"),
    (c"terminal", c"no"),
    (c"msg-level", c"all=no"),
    (c"config", c"no"),
    (c"load-scripts", c"no"),
    (c"ytdl", c"no"),
    // The player's own scripts are for a window somebody sits at.
    (c"load-stats-overlay", c"no"),
    (c"load-console", c"no"),
    (c"load-osd-console", c"no"),
    (c"load-auto-profiles", c"no"),
    (c"load-select", c"no"),
    (c"load-positioning", c"no"),
    (c"load-commands", c"no"),
    (c"load-context-menu", c"no"),
    (c"audio-client-name", c"wepaper"),
    (c"cache", c"no"),
];

fn text(path: &Path) -> Option<CString> {
    CString::new(path.to_string_lossy().as_bytes()).ok()
}

fn level(volume: f32) -> String {
    format!("{:.0}", volume.clamp(0.0, 100.0))
}

impl Sound {
    /// Starts playing `files` one after another, over and over when `looping`, at
    /// `volume` (0 to 100). `None` when libmpv is not there.
    pub fn play(files: &[PathBuf], looping: bool, volume: f32) -> Option<Self> {
        let api = mpv::api()?;
        let files: Vec<CString> = files.iter().filter_map(|file| text(file)).collect();
        let first = files.first()?;
        let volume = level(volume);
        let volume_c = CString::new(volume.as_str()).ok()?;
        // One file loops without a gap; several are a list that starts over.
        let (one, list) = match (looping, files.len()) {
            (false, _) => (c"no", c"no"),
            (true, 1) => (c"inf", c"no"),
            (true, _) => (c"no", c"inf"),
        };
        // SAFETY: every pointer handed to libmpv is a live NUL-terminated string, and
        // every command an array of them ending in a null; the handle is checked
        // before use and shut down on the failure path.
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                return None;
            }
            let own: [(&CStr, &CStr); 3] =
                [(c"loop-file", one), (c"loop-playlist", list), (c"volume", volume_c.as_c_str())];
            for (name, value) in SETTINGS.iter().chain(&own) {
                let _ = (api.set_option_string)(handle, name.as_ptr(), value.as_ptr());
            }
            if (api.initialize)(handle) < 0 {
                mpv::dismiss(api, handle, ());
                return None;
            }
            let mut load = [c"loadfile".as_ptr(), first.as_ptr(), std::ptr::null()];
            let mut queued = (api.command_async)(handle, 0, load.as_mut_ptr()) >= 0;
            for file in &files[1..] {
                let mut append = [c"loadfile".as_ptr(), file.as_ptr(), c"append".as_ptr(), std::ptr::null()];
                queued &= (api.command_async)(handle, 0, append.as_mut_ptr()) >= 0;
            }
            if !queued {
                mpv::dismiss(api, handle, ());
                return None;
            }
            Some(Self { api, handle, sent: (Some(volume), Some(false)), looping })
        }
    }

    fn set(&self, name: &CStr, value: &str) {
        let Ok(value) = CString::new(value) else { return };
        let mut pointer: *const c_char = value.as_ptr();
        // SAFETY: the name and the value are NUL-terminated and live for the call, which
        // copies them; a string property takes a pointer to the string's pointer.
        unsafe {
            (self.api.set_property_async)(
                self.handle,
                0,
                name.as_ptr(),
                mpv::FORMAT_STRING,
                (&raw mut pointer).cast(),
            )
        };
    }

    /// Sets the volume, 0 to 100.
    pub fn set_volume(&mut self, volume: f32) {
        let volume = level(volume);
        if self.sent.0.as_ref() != Some(&volume) {
            self.set(c"volume", &volume);
            self.sent.0 = Some(volume);
        }
    }

    /// Pauses or resumes.
    pub fn set_paused(&mut self, paused: bool) {
        if self.sent.1 != Some(paused) {
            self.set(c"pause", if paused { "yes" } else { "no" });
            self.sent.1 = Some(paused);
        }
    }

    /// Whether the player is done: its file played to its end (the files of a loop
    /// end too, and the loop goes on), or a file could not be played.
    ///
    /// Asked every frame: this is also what empties the player's queue of events, and a
    /// full queue takes no more changes.
    pub fn ended(&mut self) -> bool {
        let mut ended = false;
        loop {
            // SAFETY: the handle is alive; with no time to wait the call returns at once,
            // and the event it returns is valid until the next call.
            let event = unsafe { &*(self.api.wait_event)(self.handle, 0.0) };
            match event.event_id {
                mpv::EVENT_NONE => break,
                mpv::EVENT_SHUTDOWN => {
                    ended = true;
                    break;
                }
                mpv::EVENT_END_FILE if !event.data.is_null() => {
                    // SAFETY: an end-file event's data is an `mpv_event_end_file`.
                    let end = unsafe { &*event.data.cast::<mpv::EndFile>() };
                    if end.reason == mpv::END_FILE_REASON_ERROR {
                        // SAFETY: libmpv returns a static string for any code.
                        let why = unsafe { CStr::from_ptr((self.api.error_string)(end.error)) };
                        log::warn!("a sound could not be played: {}", why.to_string_lossy());
                    }
                    ended |= end.reason == mpv::END_FILE_REASON_ERROR
                        || (end.reason == mpv::END_FILE_REASON_EOF && !self.looping);
                }
                _ => {}
            }
        }
        ended
    }
}

impl crate::scene::Player for Sound {
    fn play(files: &[PathBuf], looping: bool, volume: f32) -> Option<Self> {
        Sound::play(files, looping, volume)
    }
    fn set_volume(&mut self, volume: f32) {
        Sound::set_volume(self, volume);
    }
    fn set_paused(&mut self, paused: bool) {
        Sound::set_paused(self, paused);
    }
    fn ended(&mut self) -> bool {
        Sound::ended(self)
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        // SAFETY: the handle is alive and nothing uses it afterwards.
        unsafe { mpv::dismiss(self.api, self.handle, ()) };
    }
}
