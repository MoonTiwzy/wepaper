//! Vertex data.

use glow::HasContext;

use super::{Gl, GpuError};

/// Vertices on the GPU, drawn as triangles. Deleted when dropped.
pub struct Mesh {
    gl: Gl,
    array: glow::VertexArray,
    buffer: glow::Buffer,
    indices: Option<glow::Buffer>,
    count: i32,
    capacity: usize,
    layout: Vec<(u32, i32)>,
}

impl Mesh {
    /// A mesh of interleaved float attributes.
    ///
    /// `layout` lists (attribute location, component count) in the order they are
    /// interleaved in `vertices`; `indices`, when given, are 16-bit triangle indices.
    pub fn new(gl: &Gl, layout: &[(u32, i32)], vertices: &[f32], indices: Option<&[u16]>, dynamic: bool) -> Result<Self, GpuError> {
        let stride: i32 = layout.iter().map(|(_, n)| n * 4).sum();
        let usage = if dynamic { glow::DYNAMIC_DRAW } else { glow::STATIC_DRAW };
        // SAFETY: the context is current; attribute offsets stay within `stride`, and the
        // byte views cover exactly the slices they are made from.
        unsafe {
            let raw = gl.raw();
            let array = raw.create_vertex_array().map_err(GpuError)?;
            let buffer = raw.create_buffer().map_err(GpuError)?;
            raw.bind_vertex_array(Some(array));
            raw.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            raw.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(vertices), usage);
            let mut offset = 0;
            for &(location, components) in layout {
                raw.enable_vertex_attrib_array(location);
                raw.vertex_attrib_pointer_f32(location, components, glow::FLOAT, false, stride, offset);
                offset += components * 4;
            }
            let (index_buffer, count) = match indices {
                Some(indices) => {
                    let index_buffer = raw.create_buffer().map_err(GpuError)?;
                    raw.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
                    raw.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(indices), usage);
                    (Some(index_buffer), indices.len() as i32)
                }
                None => (None, vertices.len() as i32 * 4 / stride.max(1)),
            };
            raw.bind_vertex_array(None);
            Ok(Self { gl: gl.clone(), array, buffer, indices: index_buffer, count, capacity: vertices.len(), layout: layout.to_vec() })
        }
    }

    /// A quad of four corners (bottom left, bottom right, top left, top right), each
    /// `x, y, z, u, v`, at attribute locations 0 (position) and 1 (texture coordinate).
    pub fn quad(gl: &Gl, corners: &[[f32; 5]; 4]) -> Result<Self, GpuError> {
        let order = [0, 1, 2, 2, 1, 3];
        let vertices: Vec<f32> = order.iter().flat_map(|&i| corners[i]).collect();
        Self::new(gl, &[(0, 3), (1, 2)], &vertices, None, false)
    }

    /// The quad that covers a whole target, texture coordinates running with the
    /// picture: `(0, 0)` at the first row drawn.
    pub fn fullscreen(gl: &Gl) -> Result<Self, GpuError> {
        Self::quad(gl, &[[-1.0, -1.0, 0.0, 0.0, 0.0], [1.0, -1.0, 0.0, 1.0, 0.0], [-1.0, 1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0, 1.0]])
    }

    /// Replaces the vertices of a dynamic mesh without indices.
    pub fn update(&mut self, vertices: &[f32]) {
        let stride: i32 = self.layout.iter().map(|(_, n)| n * 4).sum();
        // SAFETY: the buffer is alive; the byte view covers exactly `vertices`.
        unsafe {
            let raw = self.gl.raw();
            raw.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            if vertices.len() > self.capacity {
                raw.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(vertices), glow::DYNAMIC_DRAW);
                self.capacity = vertices.len();
            } else {
                raw.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, as_bytes(vertices));
            }
        }
        if self.indices.is_none() {
            self.count = vertices.len() as i32 * 4 / stride.max(1);
        }
    }

    /// Draws the mesh with whatever program and state are set.
    pub fn draw(&self) {
        // SAFETY: the vertex array is alive and its buffers hold `count` vertices or indices.
        unsafe {
            let raw = self.gl.raw();
            raw.bind_vertex_array(Some(self.array));
            if self.indices.is_some() {
                raw.draw_elements(glow::TRIANGLES, self.count, glow::UNSIGNED_SHORT, 0);
            } else {
                raw.draw_arrays(glow::TRIANGLES, 0, self.count);
            }
        }
    }
}

impl Drop for Mesh {
    fn drop(&mut self) {
        // SAFETY: the objects belong to the process's one context, current on this thread.
        unsafe {
            let raw = self.gl.raw();
            raw.delete_vertex_array(self.array);
            raw.delete_buffer(self.buffer);
            if let Some(indices) = self.indices {
                raw.delete_buffer(indices);
            }
        }
    }
}

/// The bytes of a slice of plain numbers.
fn as_bytes<T: Copy>(values: &[T]) -> &[u8] {
    // SAFETY: `T` is only ever `f32` or `u16` here: no padding, any bit pattern is a valid `u8`.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}
