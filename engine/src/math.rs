//! The little linear algebra the engine needs. Vectors are plain arrays.

/// A 4x4 matrix, column-major, as OpenGL takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    /// The matrix that changes nothing.
    pub const IDENTITY: Self =
        Self([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

    /// A move by `by`.
    pub fn translation(by: [f32; 3]) -> Self {
        let mut m = Self::IDENTITY;
        m.0[12] = by[0];
        m.0[13] = by[1];
        m.0[14] = by[2];
        m
    }

    /// A scale by `by` along each axis.
    pub fn scaling(by: [f32; 3]) -> Self {
        let mut m = Self::IDENTITY;
        m.0[0] = by[0];
        m.0[5] = by[1];
        m.0[10] = by[2];
        m
    }

    /// A rotation by `angles` radians around x, y and z, applied in that order.
    pub fn rotation(angles: [f32; 3]) -> Self {
        let (sx, cx) = angles[0].sin_cos();
        let (sy, cy) = angles[1].sin_cos();
        let (sz, cz) = angles[2].sin_cos();
        // Rz * Ry * Rx
        Self([
            cz * cy,
            sz * cy,
            -sy,
            0.0,
            cz * sy * sx - sz * cx,
            sz * sy * sx + cz * cx,
            cy * sx,
            0.0,
            cz * sy * cx + sz * sx,
            sz * sy * cx - cz * sx,
            cy * cx,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    }

    /// An orthographic projection of the box between the given planes.
    pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Self {
        let mut m = Self::IDENTITY;
        m.0[0] = 2.0 / (right - left);
        m.0[5] = 2.0 / (top - bottom);
        m.0[10] = -2.0 / (far - near);
        m.0[12] = -(right + left) / (right - left);
        m.0[13] = -(top + bottom) / (top - bottom);
        m.0[14] = -(far + near) / (far - near);
        m
    }

    /// A perspective projection: `fov_y` radians from the bottom of the view to its top,
    /// `aspect` its width over its height, depth mapped from `near` to `far`.
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Self {
        let focal = 1.0 / (fov_y / 2.0).tan();
        let mut m = Self([0.0; 16]);
        m.0[0] = focal / aspect;
        m.0[5] = focal;
        m.0[10] = (far + near) / (near - far);
        m.0[11] = -1.0;
        m.0[14] = 2.0 * far * near / (near - far);
        m
    }

    /// The view of a camera at `eye` looking at `center`, with `up` as near to up as
    /// the view allows: the camera's right is x, its up y, and it looks down -z.
    pub fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> Self {
        let forward = normalize(sub(center, eye));
        // A camera looking straight along `up` takes any other up: z, or x if that is up too.
        let side = cross(forward, up);
        let side = if dot(side, side) > 1e-12 {
            normalize(side)
        } else {
            let other = if forward[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
            normalize(cross(forward, other))
        };
        let up = cross(side, forward);
        Self([
            side[0],
            up[0],
            -forward[0],
            0.0,
            side[1],
            up[1],
            -forward[1],
            0.0,
            side[2],
            up[2],
            -forward[2],
            0.0,
            -dot(side, eye),
            -dot(up, eye),
            dot(forward, eye),
            1.0,
        ])
    }

    /// The 3x3 matrix that turns normals the way this one turns positions: the inverse
    /// of its transpose, column-major. A scale stretches a surface; its normals
    /// shrink along the same axis.
    pub fn normal_matrix(&self) -> [f32; 9] {
        let inverse = self.inverse().0;
        [
            inverse[0],
            inverse[4],
            inverse[8],
            inverse[1],
            inverse[5],
            inverse[9],
            inverse[2],
            inverse[6],
            inverse[10],
        ]
    }

    /// Where the matrix sends a direction (`w` is 0: no move).
    pub fn transform_vector(&self, vector: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        let [x, y, z] = vector;
        [m[0] * x + m[4] * y + m[8] * z, m[1] * x + m[5] * y + m[9] * z, m[2] * x + m[6] * y + m[10] * z]
    }

    /// `self * other`: `other` applies first.
    pub fn mul(&self, other: &Self) -> Self {
        let (a, b) = (&self.0, &other.0);
        let mut out = [0.0; 16];
        for column in 0..4 {
            for row in 0..4 {
                out[column * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
            }
        }
        Self(out)
    }

    /// Where the matrix puts `point`, after the perspective divide.
    pub fn transform_point(&self, point: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        let [x, y, z] = point;
        let w = m[3] * x + m[7] * y + m[11] * z + m[15];
        let w = if w == 0.0 { 1.0 } else { w };
        [
            (m[0] * x + m[4] * y + m[8] * z + m[12]) / w,
            (m[1] * x + m[5] * y + m[9] * z + m[13]) / w,
            (m[2] * x + m[6] * y + m[10] * z + m[14]) / w,
        ]
    }

    /// The matrix that undoes this one; the identity when there is none.
    pub fn inverse(&self) -> Self {
        let m = &self.0;
        let mut inv = [0.0f32; 16];
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
            + m[9] * m[7] * m[14]
            + m[13] * m[6] * m[11]
            - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
            - m[8] * m[7] * m[14]
            - m[12] * m[6] * m[11]
            + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
            + m[8] * m[7] * m[13]
            + m[12] * m[5] * m[11]
            - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
            - m[8] * m[6] * m[13]
            - m[12] * m[5] * m[10]
            + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
            - m[9] * m[3] * m[14]
            - m[13] * m[2] * m[11]
            + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
            + m[8] * m[3] * m[14]
            + m[12] * m[2] * m[11]
            - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
            - m[8] * m[3] * m[13]
            - m[12] * m[1] * m[11]
            + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
            + m[8] * m[2] * m[13]
            + m[12] * m[1] * m[10]
            - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
            + m[5] * m[3] * m[14]
            + m[13] * m[2] * m[7]
            - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
            - m[4] * m[3] * m[14]
            - m[12] * m[2] * m[7]
            + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
            + m[4] * m[3] * m[13]
            + m[12] * m[1] * m[7]
            - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
            - m[4] * m[2] * m[13]
            - m[12] * m[1] * m[6]
            + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
            - m[5] * m[3] * m[10]
            - m[9] * m[2] * m[7]
            + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
            + m[4] * m[3] * m[10]
            + m[8] * m[2] * m[7]
            - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
            - m[4] * m[3] * m[9]
            - m[8] * m[1] * m[7]
            + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
            + m[4] * m[2] * m[9]
            + m[8] * m[1] * m[6]
            - m[8] * m[2] * m[5];
        let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        if det.abs() < 1e-12 {
            return Self::IDENTITY;
        }
        for value in &mut inv {
            *value /= det;
        }
        Self(inv)
    }
}

/// `a - b`.
pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The dot product.
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The cross product.
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `v` made one long; a zero vector stays zero.
pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length > 0.0 { [v[0] / length, v[1] / length, v[2] / length] } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn transforms_compose_right_to_left() {
        let m = Mat4::translation([10.0, 0.0, 0.0]).mul(&Mat4::scaling([2.0, 2.0, 1.0]));
        assert!(close(m.transform_point([1.0, 1.0, 0.0]), [12.0, 2.0, 0.0]));
        let quarter = Mat4::rotation([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        assert!(close(quarter.transform_point([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn inverse_undoes_and_orthographic_maps_to_clip_space() {
        let m = Mat4::translation([3.0, -2.0, 1.0])
            .mul(&Mat4::rotation([0.3, 0.2, 0.9]))
            .mul(&Mat4::scaling([2.0, 0.5, 1.5]));
        let p = [0.7, -1.3, 2.1];
        assert!(close(m.inverse().transform_point(m.transform_point(p)), p));
        let ortho = Mat4::orthographic(0.0, 1920.0, 0.0, 1080.0, -1.0, 1.0);
        assert!(close(ortho.transform_point([0.0, 0.0, 0.0]), [-1.0, -1.0, 0.0]));
        assert!(close(ortho.transform_point([1920.0, 1080.0, 0.0]), [1.0, 1.0, 0.0]));
    }

    #[test]
    fn a_camera_sees_what_it_looks_at_in_the_middle_and_far_things_smaller() {
        let view = Mat4::look_at([0.0, 2.0, 10.0], [0.0, 2.0, 0.0], [0.0, 1.0, 0.0]);
        // In front of the camera is -z, its up is y.
        assert!(close(view.transform_point([0.0, 2.0, 0.0]), [0.0, 0.0, -10.0]));
        assert!(close(view.transform_point([1.0, 3.0, 10.0]), [1.0, 1.0, 0.0]));
        let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 2.0, 1.0, 100.0);
        let camera = projection.mul(&view);
        let middle = camera.transform_point([0.0, 2.0, 0.0]);
        assert!(middle[0].abs() < 1e-5 && middle[1].abs() < 1e-5, "{middle:?}");
        // A quarter turn of view: at distance 10 the top of the view is 10 up; the view is twice as wide.
        let top = camera.transform_point([0.0, 12.0, 0.0]);
        let right = camera.transform_point([20.0, 2.0, 0.0]);
        assert!((top[1] - 1.0).abs() < 1e-4 && (right[0] - 1.0).abs() < 1e-4, "{top:?} {right:?}");
        let far = camera.transform_point([0.0, 12.0, -10.0]);
        assert!((far[1] - 0.5).abs() < 1e-4, "twice as far is half as high: {far:?}");
        // The near plane maps to -1 and the far one to 1.
        assert!((camera.transform_point([0.0, 2.0, 9.0])[2] + 1.0).abs() < 1e-4);
        assert!((camera.transform_point([0.0, 2.0, -90.0])[2] - 1.0).abs() < 1e-3);
        // Looking straight down still makes a view.
        let down = Mat4::look_at([0.0, 5.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(close(down.transform_point([0.0, 0.0, 0.0]), [0.0, 0.0, -5.0]));
    }

    #[test]
    fn normals_stay_upright_on_a_stretched_surface() {
        // A slope of 45 degrees stretched twice as wide is flatter; its normal leans less.
        let stretch = Mat4::scaling([2.0, 1.0, 1.0]);
        let n = stretch.normal_matrix();
        let normal = [1.0f32, 1.0, 0.0];
        let turned = [
            n[0] * normal[0] + n[3] * normal[1] + n[6] * normal[2],
            n[1] * normal[0] + n[4] * normal[1] + n[7] * normal[2],
            n[2] * normal[0] + n[5] * normal[1] + n[8] * normal[2],
        ];
        assert!(close(turned, [0.5, 1.0, 0.0]), "{turned:?}");
        // A rotation turns normals as it turns everything.
        let quarter = Mat4::rotation([0.0, 0.0, std::f32::consts::FRAC_PI_2]);
        let n = quarter.normal_matrix();
        assert!(close([n[0], n[1], n[2]], [0.0, 1.0, 0.0]));
        assert!(close(quarter.transform_vector([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        assert!(close(Mat4::translation([5.0, 5.0, 5.0]).transform_vector([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]));
    }
}
