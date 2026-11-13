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

/// One playing copy of a video wallpaper, for one output.
pub struct VideoInstance {
    project: Project,
    hold: VideoHold,
    fill: Fill,
    /// The quad the video is drawn on, for the sizes it was last made for.
    quad: Option<(Mesh, (u32, u32), (u32, u32), Fill)>,
    /// The frame this instance drew last.
    seen: u64,
}

impl VideoInstance {
    pub(crate) fn new(project: Project, stream: SharedVideo) -> Self {
        Self { project, hold: VideoHold::new(stream), fill: Fill::Cover, quad: None, seen: 0 }
    }

    /// The wallpaper's `project.json`.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Changes how the video fills an output of another shape.
    pub fn set_fill(&mut self, fill: Fill) {
        self.fill = fill;
    }

    /// Pauses or resumes. A video shown on several outputs stops only when all pause it.
    pub fn set_paused(&mut self, paused: bool) {
        self.hold.set_paused(paused);
    }

    /// Draws the current frame into `to` if it is newer than what this instance drew
    /// last, or if `force` says the destination lost its contents. Returns whether it drew.
    pub fn draw(&mut self, engine: &mut Engine, to: Option<glow::Framebuffer>, size: (u32, u32), force: bool) -> bool {
        let mut stream = self.hold.stream.borrow_mut();
        stream.poll();
        let serial = stream.serial();
        let (Some(texture), Some(video)) = (stream.texture(), stream.size()) else { return false };
        if serial == self.seen && !force {
            return false;
        }
        self.seen = serial;
        if self.quad.as_ref().is_none_or(|(_, v, o, f)| (*v, *o, *f) != (video, size, self.fill)) {
            let (vw, vh, ow, oh) = (video.0 as f32, video.1 as f32, size.0 as f32, size.1 as f32);
            let scale = match self.fill {
                Fill::Cover => (ow / vw).max(oh / vh),
                Fill::Contain => (ow / vw).min(oh / vh),
                Fill::Stretch => 0.0,
            };
            // Half the video's size on the output, in clip units; what is past the edge is cropped.
            let (x, y) = if self.fill == Fill::Stretch { (1.0, 1.0) } else { (vw * scale / ow, vh * scale / oh) };
            match Mesh::quad(&engine.gl, &[[-x, -y, 0.0, 0.0, 1.0], [x, -y, 0.0, 1.0, 1.0], [-x, y, 0.0, 0.0, 0.0], [x, y, 0.0, 1.0, 0.0]]) {
                Ok(mesh) => self.quad = Some((mesh, video, size, self.fill)),
                Err(error) => {
                    log::warn!("video quad: {error}");
                    return false;
                }
            }
        }
        engine.gl.bind_framebuffer(to, size);
        if self.fill == Fill::Contain {
            engine.clear([0.0, 0.0, 0.0, 1.0]);
        }
        engine.gl.set_blend(Blend::None);
        engine.composite(texture, &Mat4::IDENTITY, 1.0);
        if let Some((mesh, ..)) = &self.quad {
            mesh.draw();
        }
        true
    }
}
