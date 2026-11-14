//! A video wallpaper: one video, drawn to fill an output.

use crate::doc::Project;
use crate::gpu::{Blend, Mesh};
use crate::math::Mat4;
use crate::{Engine, Fill};

use super::hub::SharedVideo;

/// A holder's place on a shared stream, given up when dropped.
pub(crate) struct VideoHold {
    pub(crate) stream: SharedVideo,
    key: u64,
}

impl VideoHold {
    pub(crate) fn new(stream: SharedVideo) -> Self {
        let key = stream.borrow_mut().join();
        Self { stream, key }
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.stream.borrow_mut().hold_paused(self.key, paused);
    }
}

impl Drop for VideoHold {
    fn drop(&mut self) {
        // A stream that is being borrowed right now is being dropped itself.
        if let Ok(mut stream) = self.stream.try_borrow_mut() {
            stream.leave(self.key);
        }
    }
}

/// The quad a video is drawn on, and what it was made for.
struct Quad {
    mesh: Mesh,
    /// The video's size.
    video: (u32, u32),
    /// The output's size.
    output: (u32, u32),
    fill: Fill,
    frame: crate::Frame,
}

/// One playing copy of a video wallpaper, for one output.
pub struct VideoInstance {
    project: Project,
    hold: VideoHold,
    fill: Fill,
    /// The part of the video the output shows, on top of the fill.
    frame: crate::Frame,
    /// The quad the video is drawn on, for the sizes it was last made for.
    quad: Option<Quad>,
    /// The frame this instance drew last.
    seen: u64,
}

impl VideoInstance {
    pub(crate) fn new(project: Project, stream: SharedVideo) -> Self {
        Self {
            project,
            hold: VideoHold::new(stream),
            fill: Fill::Cover,
            frame: crate::Frame::default(),
            quad: None,
            seen: 0,
        }
    }

    /// The wallpaper's `project.json`.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Whether the video ended for good: the file cannot be played.
    pub fn failed(&self) -> bool {
        self.hold.stream.borrow().ended()
    }

    /// Changes how the video fills an output of another shape.
    pub fn set_fill(&mut self, fill: Fill) {
        self.fill = fill;
    }

    /// Changes which part of the video the output shows.
    pub fn set_frame(&mut self, frame: crate::Frame) {
        self.frame = frame;
    }

    /// Sets how loud the video is, 0 to 100. Shown on several outputs it is one
    /// video, as loud as it was set last.
    pub fn set_volume(&mut self, volume: f32) {
        self.hold.stream.borrow().set_volume(volume);
    }

    /// Sets how fast the video plays: 1 as it was made. Shown on several outputs it
    /// is one video, at the speed set last.
    pub fn set_rate(&mut self, rate: f32) {
        self.hold.stream.borrow().set_speed(rate);
    }

    /// Pauses or resumes. A video shown on several outputs stops only when all pause it.
    pub fn set_paused(&mut self, paused: bool) {
        self.hold.set_paused(paused);
    }

    /// Draws the current frame into `to` if it is newer than what this instance drew
    /// last, or if `force` says the destination lost its contents. Returns whether it drew.
    pub fn draw(
        &mut self,
        engine: &mut Engine,
        to: Option<glow::Framebuffer>,
        size: (u32, u32),
        force: bool,
    ) -> bool {
        let mut stream = self.hold.stream.borrow_mut();
        stream.poll();
        let serial = stream.serial();
        let (Some(texture), Some(video)) = (stream.texture(), stream.size()) else { return false };
        if serial == self.seen && !force {
            return false;
        }
        self.seen = serial;
        if self.quad.as_ref().is_none_or(|quad| {
            (quad.video, quad.output, quad.fill, quad.frame) != (video, size, self.fill, self.frame)
        }) {
            let (vw, vh, ow, oh) = (video.0 as f32, video.1 as f32, size.0 as f32, size.1 as f32);
            let scale = match self.fill {
                Fill::Cover => (ow / vw).max(oh / vh),
                Fill::Contain => (ow / vw).min(oh / vh),
                Fill::Stretch => 0.0,
            };
            // Half the video's size on the output, in clip units; what is past the edge is cropped.
            let (x, y) =
                if self.fill == Fill::Stretch { (1.0, 1.0) } else { (vw * scale / ow, vh * scale / oh) };
            // The frame: the quad larger or smaller about the middle, and moved.
            let (x, y) = (x * self.frame.zoom, y * self.frame.zoom);
            let (dx, dy) = (self.frame.shift[0] * 2.0, self.frame.shift[1] * 2.0);
            match Mesh::quad(
                &engine.gl,
                &[
                    [dx - x, dy - y, 0.0, 0.0, 1.0],
                    [dx + x, dy - y, 0.0, 1.0, 1.0],
                    [dx - x, dy + y, 0.0, 0.0, 0.0],
                    [dx + x, dy + y, 0.0, 1.0, 0.0],
                ],
            ) {
                Ok(mesh) => {
                    self.quad = Some(Quad { mesh, video, output: size, fill: self.fill, frame: self.frame })
                }
                Err(error) => {
                    log::warn!("video quad: {error}");
                    return false;
                }
            }
        }
        engine.gl.bind_framebuffer(to, size);
        // Where the video may not reach: bars, or what a frame moved it away from.
        if self.fill == Fill::Contain || self.frame != crate::Frame::default() {
            engine.clear([0.0, 0.0, 0.0, 1.0]);
        }
        engine.gl.set_blend(Blend::None);
        engine.composite(texture, &Mat4::IDENTITY, 1.0);
        if let Some(Quad { mesh, .. }) = &self.quad {
            mesh.draw();
        }
        true
    }
}
