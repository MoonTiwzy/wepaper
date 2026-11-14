//! One decoder per file.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use crate::gpu::Gl;

use super::stream::{VideoError, VideoOptions, VideoStream, Waker};
use super::transcode::{self, Job, Limits};

/// A stream that several outputs and scenes may be showing.
pub type SharedVideo = Rc<RefCell<VideoStream>>;

/// A stream and what is known about making it cheaper.
struct Playing {
    stream: Weak<RefCell<VideoStream>>,
    /// The file it plays is already within the limits, or cannot be made so.
    settled: bool,
    job: Option<Job>,
}

/// Hands out streams, the same one to everybody who asks for the same file.
pub struct VideoHub {
    gl: Gl,
    streams: HashMap<PathBuf, Playing>,
    waker: Option<Waker>,
    /// Videos over these are re-encoded into `cache`.
    pub limits: Limits,
    /// Where cheaper copies are kept; `None` keeps none.
    pub cache: Option<PathBuf>,
    /// How streams opened from now on are played.
    pub options: VideoOptions,
}

impl VideoHub {
    /// A hub with no streams.
    pub fn new(gl: &Gl) -> Self {
        Self {
            gl: gl.clone(),
            streams: HashMap::new(),
            waker: Waker::new().ok(),
            limits: Limits { max_width: 0, max_fps: 0.0 },
            cache: None,
            options: VideoOptions::default(),
        }
    }

    /// The stream playing `path`, started if nobody plays it yet. When a cheaper
    /// copy for the current limits is in the cache, that is what plays.
    pub fn open(&mut self, path: &Path) -> Result<SharedVideo, VideoError> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        if let Some(stream) = self.streams.get(&key).and_then(|playing| playing.stream.upgrade()) {
            return Ok(stream);
        }
        let copy = self
            .cache
            .as_ref()
            .map(|dir| transcode::cached_path(dir, &key, self.limits))
            .filter(|copy| copy.is_file());
        let stream = Rc::new(RefCell::new(VideoStream::open(
            &self.gl,
            copy.as_deref().unwrap_or(&key),
            &self.options,
            self.waker.clone(),
        )?));
        self.streams.retain(|_, playing| playing.stream.strong_count() > 0);
        self.streams
            .insert(key, Playing { stream: Rc::downgrade(&stream), settled: copy.is_some(), job: None });
        Ok(stream)
    }

    /// Looks after the cheaper copies: starts re-encoding videos that turned out
    /// to be over the limits, and moves streams onto copies that are ready. Call
    /// now and then; returns how many videos are being re-encoded.
    pub fn optimise(&mut self) -> usize {
        let Some(dir) = self.cache.clone() else { return 0 };
        let limits = self.limits;
        let mut running = 0;
        for (source, playing) in &mut self.streams {
            let Some(stream) = playing.stream.upgrade() else { continue };
            if let Some(job) = &playing.job {
                match job.finished() {
                    None => running += 1,
                    Some(ok) => {
                        if ok {
                            stream.borrow_mut().switch_to(&job.target);
                            log::info!("{} now plays from its optimised copy", source.display());
                        }
                        playing.job = None;
                    }
                }
                continue;
            }
            if playing.settled {
                continue;
            }
            // What the video really is shows once it plays.
            let (width, fps) = {
                let stream = stream.borrow();
                (stream.source_width(), stream.frame_rate())
            };
            let (Some(width), Some(fps)) = (width, fps) else { continue };
            playing.settled = true;
            if limits.exceeded_by(width, fps) {
                log::info!(
                    "{} is {width} wide at {fps:.0} fps: making a copy within {} wide, {:.0} fps",
                    source.display(),
                    limits.max_width,
                    limits.max_fps
                );
                playing.job = transcode::start(source, &transcode::cached_path(&dir, source, limits), limits);
                running += usize::from(playing.job.is_some());
            }
        }
        running
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
        for stream in self.streams.values().filter_map(|playing| playing.stream.upgrade()) {
            let mut stream = stream.borrow_mut();
            stream.poll();
            ready &= stream.texture().is_some() || stream.ended();
        }
        ready
    }

    /// How many videos are being decoded.
    pub fn playing(&self) -> usize {
        self.streams.values().filter(|playing| playing.stream.strong_count() > 0).count()
    }

    /// Applies the sound settings to every playing stream.
    pub fn set_sound(&mut self, audio: bool, volume: f32) {
        self.options.audio = audio;
        self.options.volume = volume;
        for stream in self.streams.values().filter_map(|playing| playing.stream.upgrade()) {
            let stream = stream.borrow();
            stream.set_audio(audio);
            stream.set_volume(volume);
        }
    }
}
