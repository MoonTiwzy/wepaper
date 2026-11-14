//! Shader programs: compiling, linking, reflection and the on-disk cache.

use std::path::PathBuf;

use glow::HasContext;

use super::Gl;
use crate::read::fnv;

/// Vertex attributes sit at the same location in every program, so one mesh draws
/// with any of them.
pub const ATTRIBUTES: &[(&str, u32)] = &[
    ("a_Position", 0),
    ("a_TexCoord", 1),
    ("a_Color", 2),
    ("a_TexCoordVec4", 3),
    ("a_TexCoordVec4C1", 4),
    ("a_TexCoordVec4C2", 5),
    ("a_TexCoordC2", 6),
    ("a_BlendIndices", 7),
    ("a_BlendWeights", 8),
    ("a_Normal", 9),
    ("a_Tangent4", 10),
    ("a_TexCoordVec3", 11),
    ("a_PositionVec4", 12),
];

/// A shader that did not compile or link, with what the driver said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramError {
    /// `vertex`, `fragment` or `link`.
    pub stage: &'static str,
    /// The driver's log.
    pub log: String,
}

impl std::fmt::Display for ProgramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} shader: {}", self.stage, self.log.trim())
    }
}

impl std::error::Error for ProgramError {}

/// One uniform of a linked program.
#[derive(Debug, Clone)]
pub struct Uniform {
    /// Its name, without `[0]`.
    pub name: String,
    /// Its GL type (`glow::FLOAT_VEC4`, `glow::SAMPLER_2D`, ...).
    pub kind: u32,
    /// How many elements it has; 1 unless it is an array.
    pub count: i32,
    location: glow::UniformLocation,
}

/// A linked program. Deleted when dropped.
pub struct Program {
    gl: Gl,
    raw: glow::Program,
    /// Every uniform the driver kept.
    pub uniforms: Vec<Uniform>,
}

impl Program {
    /// Compiles and links a program.
    pub fn link(gl: &Gl, vertex: &str, fragment: &str) -> Result<Self, ProgramError> {
        // SAFETY: the context is current; every object is checked before use and deleted on failure.
        unsafe {
            let raw = gl.raw().create_program().map_err(|log| ProgramError { stage: "link", log })?;
            let compile =
                |kind: u32, stage: &'static str, source: &str| -> Result<glow::Shader, ProgramError> {
                    let shader = gl.raw().create_shader(kind).map_err(|log| ProgramError { stage, log })?;
                    gl.raw().shader_source(shader, source);
                    gl.raw().compile_shader(shader);
                    if gl.raw().get_shader_compile_status(shader) {
                        Ok(shader)
                    } else {
                        let log = gl.raw().get_shader_info_log(shader);
                        gl.raw().delete_shader(shader);
                        Err(ProgramError { stage, log })
                    }
                };
            let vs = compile(glow::VERTEX_SHADER, "vertex", vertex)
                .inspect_err(|_| gl.raw().delete_program(raw))?;
            let fs = compile(glow::FRAGMENT_SHADER, "fragment", fragment).inspect_err(|_| {
                gl.raw().delete_shader(vs);
                gl.raw().delete_program(raw);
            })?;
            gl.raw().attach_shader(raw, vs);
            gl.raw().attach_shader(raw, fs);
            for &(name, location) in ATTRIBUTES {
                gl.raw().bind_attrib_location(raw, location, name);
            }
            if gl.caps().program_binary {
                gl.raw().program_binary_retrievable_hint(raw, true);
            }
            gl.raw().link_program(raw);
            gl.raw().detach_shader(raw, vs);
            gl.raw().detach_shader(raw, fs);
            gl.raw().delete_shader(vs);
            gl.raw().delete_shader(fs);
            if !gl.raw().get_program_link_status(raw) {
                let log = gl.raw().get_program_info_log(raw);
                gl.raw().delete_program(raw);
                return Err(ProgramError { stage: "link", log });
            }
            Ok(Self::reflect(gl, raw))
        }
    }

    /// The program saved under `key` by an earlier run on this driver, if there is one.
    pub fn restore(gl: &Gl, key: u64) -> Option<Self> {
        let file = cache_file(gl, key)?;
        let bytes = std::fs::read(file).ok()?;
        if bytes.len() < 4 {
            return None;
        }
        let format = u32::from_le_bytes(bytes[..4].try_into().expect("four bytes"));
        // SAFETY: the context is current; a binary the driver does not accept only
        // fails the link, which is checked before the program is used.
        unsafe {
            let raw = gl.raw().create_program().ok()?;
            gl.raw().program_binary(raw, &glow::ProgramBinary { buffer: bytes[4..].to_vec(), format });
            if !gl.raw().get_program_link_status(raw) {
                gl.raw().delete_program(raw);
                return None;
            }
            Some(Self::reflect(gl, raw))
        }
    }

    /// Saves the linked program under `key` for later runs. Failing to only means
    /// the next run compiles again.
    pub fn save(&self, key: u64) {
        let Some(file) = cache_file(&self.gl, key) else { return };
        // SAFETY: the context is current and the program is linked.
        let Some(binary) = (unsafe { self.gl.raw().get_program_binary(self.raw) }) else { return };
        let mut bytes = binary.format.to_le_bytes().to_vec();
        bytes.extend_from_slice(&binary.buffer);
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let part = file.with_extension(format!("{}.part", std::process::id()));
        if std::fs::write(&part, bytes).is_ok() {
            let _ = std::fs::rename(&part, file);
        }
    }

    /// Reads the uniforms of a linked program.
    ///
    /// # Safety
    /// The context must be current and `raw` a linked program of it.
    unsafe fn reflect(gl: &Gl, raw: glow::Program) -> Self {
        let mut uniforms = Vec::new();
        // SAFETY: per this function's contract.
        unsafe {
            for index in 0..gl.raw().get_active_uniforms(raw) {
                let Some(active) = gl.raw().get_active_uniform(raw, index) else { continue };
                let Some(location) = gl.raw().get_uniform_location(raw, &active.name) else { continue };
                let name = active.name.split('[').next().unwrap_or(&active.name).to_owned();
                uniforms.push(Uniform { name, kind: active.utype, count: active.size, location });
            }
        }
        Self { gl: gl.clone(), raw, uniforms }
    }

    /// The uniform called `name`, if the program has it.
    pub fn uniform(&self, name: &str) -> Option<&Uniform> {
        self.uniforms.iter().find(|u| u.name == name)
    }

    /// Makes this the program that draws.
    pub fn bind(&self) {
        self.gl.use_program(self.raw);
    }

    /// Sets a uniform from floats, as many as its type takes. The program must be bound.
    pub fn set(&self, uniform: &Uniform, values: &[f32]) {
        let location = Some(&uniform.location);
        let gl = self.gl.raw();
        let take = |per: usize| &values[..values.len().min(per * uniform.count.max(1) as usize) / per * per];
        // SAFETY: the program is bound and each slice is a whole number of elements of the uniform's type.
        unsafe {
            match uniform.kind {
                glow::FLOAT => gl.uniform_1_f32_slice(location, take(1)),
                glow::FLOAT_VEC2 => gl.uniform_2_f32_slice(location, take(2)),
                glow::FLOAT_VEC3 => gl.uniform_3_f32_slice(location, take(3)),
                glow::FLOAT_VEC4 => gl.uniform_4_f32_slice(location, take(4)),
                glow::FLOAT_MAT3 => gl.uniform_matrix_3_f32_slice(location, false, take(9)),
                glow::FLOAT_MAT4 => gl.uniform_matrix_4_f32_slice(location, false, take(16)),
                glow::FLOAT_MAT4x3 => gl.uniform_matrix_4x3_f32_slice(location, false, take(12)),
                glow::INT
                | glow::BOOL
                | glow::SAMPLER_2D
                | glow::SAMPLER_3D
                | glow::SAMPLER_2D_SHADOW
                | glow::SAMPLER_CUBE => {
                    if let Some(first) = values.first() {
                        gl.uniform_1_i32(location, *first as i32);
                    }
                }
                _ => {}
            }
        }
    }

    /// Points a sampler uniform at a texture unit. The program must be bound.
    pub fn set_sampler(&self, uniform: &Uniform, unit: u32) {
        // SAFETY: the program is bound and the location is one of its own.
        unsafe { self.gl.raw().uniform_1_i32(Some(&uniform.location), unit as i32) };
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        // SAFETY: the program belongs to the process's one context, current on this thread.
        unsafe { self.gl.raw().delete_program(self.raw) };
        self.gl.invalidate();
    }
}

/// Where the program saved under `key` lives for this exact driver.
fn cache_file(gl: &Gl, key: u64) -> Option<PathBuf> {
    if !gl.caps().program_binary {
        return None;
    }
    let mut key = fnv(key, gl.caps().renderer.as_bytes());
    key = fnv(key, gl.caps().version.as_bytes());
    Some(gl.cache_dir()?.join(format!("{key:016x}.bin")))
}
