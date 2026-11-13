//! One decoder per file.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use crate::gpu::Gl;

use super::stream::{VideoError, VideoOptions, VideoStream, Waker};

/// A stream that several outputs and scenes may be showing.
pub type SharedVideo = Rc<RefCell<VideoStream>>;

/// Hands out streams, the same one to everybody who asks for the same file.
pub struct VideoHub {
    gl: Gl,
    streams: HashMap<PathBuf, Weak<RefCell<VideoStream>>>,
    waker: Option<Waker>,
    /// How streams opened from now on are played.
    pub options: VideoOptions,
}

impl VideoHub {
    /// A hub with no streams.
    pub fn new(gl: &Gl) -> Self {
        Self { gl: gl.clone(), streams: HashMap::new(), waker: Waker::new().ok(), options: VideoOptions::default() }
    }

    /// The stream playing `path`, started if nobody plays it yet.
    pub fn open(&mut self, path: &Path) -> Result<SharedVideo, VideoError> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        if let Some(stream) = self.streams.get(&key).and_then(Weak::upgrade) {
            return Ok(stream);
        }
        let stream = Rc::new(RefCell::new(VideoStream::open(&self.gl, &key, &self.options, self.waker.clone())?));
        self.streams.retain(|_, stream| stream.strong_count() > 0);
        self.streams.insert(key, Rc::downgrade(&stream));
        Ok(stream)
    }

    /// What is poked when any stream has a new frame.
    pub fn waker(&self) -> Option<&Waker> {
        self.waker.as_ref()
    }

    /// Draws every stream's newest frame and says whether all of them have shown one.
    ///
    /// What renders a single frame to a file waits on this: a video has nothing
    /// to show until its decoder delivers.
    pub fn poll_all(&mut self) -> bool {
        let mut ready = true;
        for stream in self.streams.values().filter_map(Weak::upgrade) {
            let mut stream = stream.borrow_mut();
            stream.poll();
            ready &= stream.texture().is_some() || stream.ended();
        }
        ready
    }

    /// How many videos are being decoded.
    pub fn playing(&self) -> usize {
        self.streams.values().filter(|s| s.strong_count() > 0).count()
    }

    /// Applies the sound settings to every playing stream.
    pub fn set_sound(&mut self, audio: bool, volume: f32) {
        self.options.audio = audio;
        self.options.volume = volume;
        for stream in self.streams.values().filter_map(Weak::upgrade) {
            let stream = stream.borrow();
            stream.set_audio(audio);
            stream.set_volume(volume);
        }
    }
}
