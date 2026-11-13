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
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

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
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
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
        let m = Mat4::translation([3.0, -2.0, 1.0]).mul(&Mat4::rotation([0.3, 0.2, 0.9])).mul(&Mat4::scaling([2.0, 0.5, 1.5]));
        let p = [0.7, -1.3, 2.1];
        assert!(close(m.inverse().transform_point(m.transform_point(p)), p));
        let ortho = Mat4::orthographic(0.0, 1920.0, 0.0, 1080.0, -1.0, 1.0);
        assert!(close(ortho.transform_point([0.0, 0.0, 0.0]), [-1.0, -1.0, 0.0]));
        assert!(close(ortho.transform_point([1920.0, 1080.0, 0.0]), [1.0, 1.0, 0.0]));
    }
}
