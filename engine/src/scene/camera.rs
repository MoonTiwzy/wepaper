//! The perspective camera of a 3D scene.
//!
//! A scene whose `orthogonalprojection` is null is seen through a camera. When the
//! scene has a camera object, that is it: the camera stands where the object is and
//! looks along the object's -z with its y up, with the object's field of view; it moves
//! as the object does (timelines, scripts, parents). A scene without one is seen from
//! the document's `eye`, looking at its `center` (that is where the editor's view was
//! when the scene was saved). Everything, flat layers included, is placed in the same
//! world and drawn through that camera. The flat camera of 2D scenes is in `draw.rs`
//! and does not change.

use crate::math::{Mat4, cross, dot, normalize, sub};

/// The camera as the scene document sets it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Perspective {
    pub(crate) eye: [f32; 3],
    pub(crate) center: [f32; 3],
    pub(crate) up: [f32; 3],
    /// From the bottom of the view to its top, in degrees.
    pub(crate) fov: f32,
    pub(crate) near: f32,
    pub(crate) far: f32,
}

/// The camera of one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct View {
    pub(crate) view: Mat4,
    pub(crate) projection: Mat4,
    /// `projection * view`.
    pub(crate) view_projection: Mat4,
    /// Where the camera is.
    pub(crate) eye: [f32; 3],
    /// The camera's right, up and the way it looks, in the world.
    pub(crate) right: [f32; 3],
    pub(crate) up: [f32; 3],
    pub(crate) forward: [f32; 3],
}

/// A camera object of the scene, by the layer it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CameraLayer {
    pub(crate) index: usize,
    /// From the bottom of its view to the top, in degrees.
    pub(crate) fov: f32,
    /// How far it is zoomed in: its view is that much narrower.
    pub(crate) zoom: f32,
}

impl CameraLayer {
    /// The camera where its layer is this frame, `world` being the layer's place.
    pub(crate) fn place(&self, world: &Mat4, near: f32, far: f32) -> Perspective {
        let eye = [world.0[12], world.0[13], world.0[14]];
        let forward = normalize(world.transform_vector([0.0, 0.0, -1.0]));
        let up = normalize(world.transform_vector([0.0, 1.0, 0.0]));
        let zoom = if self.zoom > 0.0 { self.zoom } else { 1.0 };
        let half = (self.fov.clamp(1.0, 179.0).to_radians() / 2.0).tan() / zoom;
        Perspective {
            eye,
            center: [eye[0] + forward[0], eye[1] + forward[1], eye[2] + forward[2]],
            up,
            fov: (half.atan() * 2.0).to_degrees(),
            near,
            far,
        }
    }
}

impl Perspective {
    /// The camera for an output of `aspect` (width over height), moved sideways by
    /// `shift`: right and up in the camera's own plane, in world units. Parallax and
    /// shake move it that way; it keeps looking the way it looked.
    pub(crate) fn view(&self, aspect: f32, shift: [f32; 2]) -> View {
        let forward = normalize(sub(self.center, self.eye));
        let right = normalize(cross(forward, self.up));
        let up = cross(right, forward);
        let moved = |point: [f32; 3]| {
            [
                point[0] + right[0] * shift[0] + up[0] * shift[1],
                point[1] + right[1] * shift[0] + up[1] * shift[1],
                point[2] + right[2] * shift[0] + up[2] * shift[1],
            ]
        };
        let (eye, center) = (moved(self.eye), moved(self.center));
        let view = Mat4::look_at(eye, center, self.up);
        // A document can say anything; a camera needs a near plane in front of it and a far one beyond.
        let near = self.near.max(1e-4);
        let far = self.far.max(near * 2.0);
        let fov = self.fov.clamp(1.0, 179.0).to_radians();
        let projection = Mat4::perspective(fov, aspect.max(1e-3), near, far);
        View { view, projection, view_projection: projection.mul(&view), eye, right, up, forward }
    }

    /// How far the camera is from what it looks at: the scale its moves are made in.
    pub(crate) fn distance(&self) -> f32 {
        let between = sub(self.center, self.eye);
        dot(between, between).sqrt()
    }
}

impl View {
    /// How many pixels of an output `height` pixels high one world unit covers at `point`.
    /// What is behind the camera, or at it, covers nothing.
    pub(crate) fn pixels_per_unit(&self, point: [f32; 3], height: f32) -> f32 {
        let depth = -self.view.transform_point(point)[2];
        if depth <= 1e-6 { 0.0 } else { self.projection.0[5] * height / 2.0 / depth }
    }
}

impl super::SceneInstance {
    /// This frame's view through the scene's perspective camera. Parallax moves the
    /// camera against the pointer and shake moves it about, both sideways in its own
    /// plane and by a part of how far it looks, so a scene of any scale moves alike.
    pub(super) fn perspective_view(&self, camera: &Perspective, time: f32) -> View {
        let reach = camera.distance().max(1e-3);
        let mut shift = [0.0f32; 2];
        if self.parallax.enabled {
            let parallax = &self.parallax;
            let strength = parallax.amount * parallax.mouse_influence * reach;
            // The pointer counts from the top left; the camera's up is up.
            shift[0] += (self.eased_pointer[0] - 0.5) * strength;
            shift[1] += (0.5 - self.eased_pointer[1]) * strength;
        }
        if self.shake.enabled {
            let shake = &self.shake;
            // The same wandering as the flat camera's.
            let t = time * shake.speed;
            let wander = |a: f32, b: f32| (t * a).sin() + shake.roughness * 0.5 * (t * b + 1.7).sin();
            let amplitude = shake.amplitude * reach * 0.01;
            shift[0] += wander(1.0, 2.9) * amplitude;
            shift[1] += wander(1.3, 3.7) * amplitude;
        }
        let aspect = self.viewport.width as f32 / self.viewport.height.max(1) as f32;
        camera.view(aspect, shift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Perspective {
        Perspective {
            eye: [0.0, 0.0, 10.0],
            center: [0.0, 0.0, 9.0],
            up: [0.0, 1.0, 0.0],
            fov: 90.0,
            near: 0.1,
            far: 100.0,
        }
    }

    #[test]
    fn the_camera_looks_from_its_eye_at_its_center() {
        let view = camera().view(16.0 / 9.0, [0.0, 0.0]);
        let middle = view.view_projection.transform_point([0.0, 0.0, 0.0]);
        assert!(middle[0].abs() < 1e-5 && middle[1].abs() < 1e-5);
        // A quarter turn from bottom to top: ten units away, the top of the view is ten up.
        let top = view.view_projection.transform_point([0.0, 10.0, 0.0]);
        assert!((top[1] - 1.0).abs() < 1e-4, "{top:?}");
        assert_eq!((view.right, view.up, view.forward), ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]));
        assert_eq!(view.eye, [0.0, 0.0, 10.0]);
        assert!((view.pixels_per_unit([0.0, 0.0, 0.0], 1080.0) - 54.0).abs() < 1e-3);
        assert_eq!(view.pixels_per_unit([0.0, 0.0, 20.0], 1080.0), 0.0, "behind the camera");
    }

    #[test]
    fn a_camera_object_looks_along_its_own_minus_z() {
        // Tilted down a quarter turn: it looks straight down, its up is the world's -z.
        let world =
            Mat4::translation([0.0, 5.0, 0.0]).mul(&Mat4::rotation([-std::f32::consts::FRAC_PI_2, 0.0, 0.0]));
        let camera = CameraLayer { index: 0, fov: 60.0, zoom: 2.0 }.place(&world, 0.1, 100.0);
        assert_eq!(camera.eye, [0.0, 5.0, 0.0]);
        let down = sub(camera.center, camera.eye);
        assert!(down[0].abs() < 1e-5 && (down[1] + 1.0).abs() < 1e-5 && down[2].abs() < 1e-5, "{down:?}");
        assert!(camera.up[2] < -0.99, "{:?}", camera.up);
        // Zoomed twice: half the view.
        let half = (camera.fov.to_radians() / 2.0).tan();
        assert!((half - (30.0f32.to_radians().tan() / 2.0)).abs() < 1e-5);
        let view = camera.view(1.0, [0.0, 0.0]);
        let below = view.view_projection.transform_point([0.0, 0.0, 0.0]);
        assert!(below[0].abs() < 1e-5 && below[1].abs() < 1e-5, "{below:?}");
    }

    #[test]
    fn a_shifted_camera_moves_sideways_and_keeps_its_direction() {
        let view = camera().view(1.0, [1.0, -2.0]);
        assert_eq!(view.eye, [1.0, -2.0, 10.0]);
        assert_eq!(view.forward, [0.0, 0.0, -1.0]);
        let ahead = view.view_projection.transform_point([1.0, -2.0, 0.0]);
        assert!(ahead[0].abs() < 1e-5 && ahead[1].abs() < 1e-5, "{ahead:?}");
        assert!((camera().distance() - 1.0).abs() < 1e-6);
    }
}
