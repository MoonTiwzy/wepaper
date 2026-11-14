//! Output shaders: fragment shaders run over the finished frame.
//!
//! This is the hook shader plugins use. A pass is plain GLSL 330 with a small,
//! fixed interface:
//!
//! ```glsl
//! uniform sampler2D u_Frame;      // the frame so far
//! uniform vec2 u_Resolution;      // its size in pixels
//! uniform float u_Time;           // seconds the wallpaper has played
//! in vec2 v_TexCoord;             // 0..1, y up
//! out vec4 color;
//! ```
//!
//! Every other `uniform float` in the shader is a parameter that can be set by name.

use crate::gpu::{Blend, Gl, Program, Target, TextureFormat};
use crate::{Engine, LoadError};

const VERTEX: &str = "#version 330 core\nin vec3 a_Position;\nin vec2 a_TexCoord;\nout vec2 v_TexCoord;\n\
                      void main() { v_TexCoord = a_TexCoord; gl_Position = vec4(a_Position, 1.0); }\n";

/// One fragment shader of the chain.
pub struct PostPass {
    program: Program,
    /// The shader's parameters and their current values.
    pub params: Vec<(String, f32)>,
    /// Whether the pass runs.
    pub enabled: bool,
}

impl PostPass {
    /// Compiles `fragment`, a GLSL 330 fragment shader with the interface above.
    pub fn compile(gl: &Gl, fragment: &str) -> Result<Self, LoadError> {
        let program = Program::link(gl, VERTEX, fragment).map_err(|e| LoadError(e.to_string()))?;
        let params = program
            .uniforms
            .iter()
            .filter(|uniform| uniform.kind == glow::FLOAT && uniform.count == 1 && uniform.name != "u_Time")
            .map(|uniform| (uniform.name.clone(), 0.0))
            .collect();
        Ok(Self { program, params, enabled: true })
    }

    /// Sets a parameter; false when the shader has none of that name.
    pub fn set(&mut self, name: &str, value: f32) -> bool {
        match self.params.iter_mut().find(|(param, _)| param == name) {
            Some(param) => {
                param.1 = value;
                true
            }
            None => false,
        }
    }
}

impl Engine {
    /// Runs the enabled passes of `chain` over `frame` and puts the result into `to`
    /// (`None`: the current surface) of `size`.
    pub fn post(
        &mut self,
        chain: &[&PostPass],
        frame: &Target,
        to: Option<glow::Framebuffer>,
        size: (u32, u32),
        time: f32,
    ) {
        let passes: Vec<&PostPass> = chain.iter().copied().filter(|pass| pass.enabled).collect();
        if passes.is_empty() {
            frame.blit(to, size);
            return;
        }
        // Passes but the last draw into borrowed buffers of the frame's size.
        let mut held: Vec<Target> = Vec::new();
        for (index, pass) in passes.iter().enumerate() {
            let last = index + 1 == passes.len();
            if last {
                self.gl.bind_framebuffer(to, size);
            } else {
                match self.pool.take(frame.width(), frame.height(), TextureFormat::Rgba8) {
                    Ok(target) => {
                        target.bind();
                        held.push(target);
                    }
                    Err(error) => {
                        log::warn!("output shader: {error}");
                        break;
                    }
                }
            }
            // The newest buffer is the one being drawn into; the input is the one before it.
            let input = if last { held.last() } else { held.len().checked_sub(2).and_then(|i| held.get(i)) }
                .map_or(&frame.texture, |t| &t.texture);
            self.gl.set_blend(Blend::None);
            pass.program.bind();
            self.gl.bind_texture(0, input.raw());
            for uniform in &pass.program.uniforms {
                match uniform.name.as_str() {
                    "u_Frame" => pass.program.set_sampler(uniform, 0),
                    "u_Resolution" => {
                        pass.program.set(uniform, &[frame.width() as f32, frame.height() as f32])
                    }
                    "u_Time" => pass.program.set(uniform, &[time]),
                    name => {
                        if let Some((_, value)) = pass.params.iter().find(|(param, _)| param == name) {
                            pass.program.set(uniform, &[*value]);
                        }
                    }
                }
            }
            self.fullscreen.draw();
        }
        for target in held {
            self.pool.give(target);
        }
    }
}
