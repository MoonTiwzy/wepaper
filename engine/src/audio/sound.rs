//! A scene's sound layer.

use std::ffi::{CStr, CString};
use std::path::Path;

use crate::video::mpv::{self, Api};

/// A sound file playing through libmpv, without video.
pub struct Sound {
    api: &'static Api,
    handle: *mut mpv::Handle,
}

impl Sound {
    /// Starts playing `path`, in a loop or once, at `volume` (0 to 100). `None` when libmpv is not there.
    pub fn play(path: &Path, looping: bool, volume: f32) -> Option<Self> {
        let api = mpv::api()?;
        let file = CString::new(path.to_string_lossy().as_bytes()).ok()?;
        // SAFETY: every pointer handed to libmpv is a live NUL-terminated string; the
        // handle is checked before use and destroyed on every failure path.
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                return None;
            }
            let volume = CString::new(format!("{:.0}", volume.clamp(0.0, 100.0))).ok()?;
            let settings: &[(&CStr, &CStr)] = &[
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
                (c"loop-file", if looping { c"inf" } else { c"no" }),
                (c"volume", volume.as_c_str()),
                (c"cache", c"no"),
            ];
            for (name, value) in settings {
                let _ = (api.set_option_string)(handle, name.as_ptr(), value.as_ptr());
            }
            let mut command = [c"loadfile".as_ptr(), file.as_ptr(), std::ptr::null()];
            if (api.initialize)(handle) < 0 || (api.command)(handle, command.as_mut_ptr()) < 0 {
                (api.terminate_destroy)(handle);
                return None;
            }
            Some(Self { api, handle })
        }
    }

    fn set(&self, name: &CStr, value: &str) {
        if let Ok(value) = CString::new(value) {
            // SAFETY: both strings are NUL-terminated and live for the call.
            unsafe { (self.api.set_property_string)(self.handle, name.as_ptr(), value.as_ptr()) };
        }
    }

    /// Sets the volume, 0 to 100.
    pub fn set_volume(&self, volume: f32) {
        self.set(c"volume", &format!("{:.0}", volume.clamp(0.0, 100.0)));
    }

    /// Pauses or resumes.
    pub fn set_paused(&self, paused: bool) {
        self.set(c"pause", if paused { "yes" } else { "no" });
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        // SAFETY: the handle is alive and nothing uses it afterwards.
        unsafe { (self.api.terminate_destroy)(self.handle) };
    }
}
