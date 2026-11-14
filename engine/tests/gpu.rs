//! The GPU layer on a headless context. Skipped on machines where EGL gives none.

use wepaper_engine::gpu::{
    Blend, Context, Mesh, Program, Sampling, Target, TargetPool, Texture, TextureFormat,
};

fn context() -> Option<Context> {
    match Context::headless(None) {
        Ok(context) => Some(context),
        Err(error) => {
            eprintln!("skipped: {error}");
            None
        }
    }
}

const VERTEX: &str = "#version 330 core\nin vec3 a_Position;\nin vec2 a_TexCoord;\nout vec2 v_TexCoord;\nuniform mat4 g_Transform;\n\
                      void main() { v_TexCoord = a_TexCoord; gl_Position = g_Transform * vec4(a_Position, 1.0); }\n";
const FRAGMENT: &str = "#version 330 core\nin vec2 v_TexCoord;\nout vec4 color;\nuniform sampler2D g_Texture0;\nuniform vec4 g_Tint;\nuniform float g_Unused;\n\
                        void main() { color = texture(g_Texture0, v_TexCoord) * g_Tint; }\n";
const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

#[test]
fn headless_context_clears_and_reads_back() {
    let Some(context) = context() else { return };
    let gl = context.gl();
    assert!(!gl.caps().renderer.is_empty());
    let target = Target::new(gl, 8, 4, TextureFormat::Rgba8).unwrap();
    target.clear([1.0, 0.0, 1.0, 1.0]);
    let pixels = target.read_rgba();
    assert_eq!(pixels.len(), 8 * 4 * 4);
    assert_eq!(&pixels[..4], &[255, 0, 255, 255]);
    assert_eq!(&pixels[pixels.len() - 4..], &[255, 0, 255, 255]);
}

#[test]
fn draws_a_texture_the_right_way_up() {
    let Some(context) = context() else { return };
    let gl = context.gl();
    // Two rows: the first (top of the picture) red, the second blue.
    let pixels = [255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255];
    let texture = Texture::from_levels(
        gl,
        TextureFormat::Rgba8,
        &[(2, 2, &pixels)],
        Sampling { clamp: true, nearest: true },
    )
    .unwrap();
    let program = Program::link(gl, VERTEX, FRAGMENT).unwrap();
    let target = Target::new(gl, 2, 2, TextureFormat::Rgba8).unwrap();
    target.clear([0.0; 4]);
    program.bind();
    program.set(program.uniform("g_Transform").unwrap(), &IDENTITY);
    program.set(program.uniform("g_Tint").unwrap(), &[1.0, 1.0, 1.0, 1.0]);
    program.set_sampler(program.uniform("g_Texture0").unwrap(), 0);
    gl.bind_texture(0, texture.raw());
    gl.set_blend(Blend::None);
    // A layer's quad: the top corners carry v = 0, the picture's first row.
    let quad = Mesh::quad(
        gl,
        &[
            [-1.0, -1.0, 0.0, 0.0, 1.0],
            [1.0, -1.0, 0.0, 1.0, 1.0],
            [-1.0, 1.0, 0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0, 1.0, 0.0],
        ],
    )
    .unwrap();
    quad.draw();
    let out = target.read_rgba();
    assert_eq!(&out[..4], &[255, 0, 0, 255], "the top row read back is the picture's first row");
    assert_eq!(&out[8..12], &[0, 0, 255, 255]);
}

#[test]
fn program_reflection_lists_uniforms() {
    let Some(context) = context() else { return };
    let program = Program::link(context.gl(), VERTEX, FRAGMENT).unwrap();
    let mut names: Vec<&str> = program.uniforms.iter().map(|u| u.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["g_Texture0", "g_Tint", "g_Transform"], "an unused uniform is not there");
    assert_eq!(program.uniform("g_Tint").unwrap().kind, glow::FLOAT_VEC4);
    assert!(program.uniform("g_Unused").is_none());
}

#[test]
fn compile_error_carries_log() {
    let Some(context) = context() else { return };
    let error =
        Program::link(context.gl(), VERTEX, "#version 330 core\nvoid main() { nonsense }\n").err().unwrap();
    assert_eq!(error.stage, "fragment");
    assert!(!error.log.trim().is_empty());
    let error =
        Program::link(context.gl(), "#version 330 core\nvoid main() { broken\n", FRAGMENT).err().unwrap();
    assert_eq!(error.stage, "vertex");
}

#[test]
fn program_binaries_are_cached_on_disk() {
    let Some(context) = context() else { return };
    let gl = context.gl();
    if !gl.caps().program_binary {
        eprintln!("skipped: no program binaries on this driver");
        return;
    }
    let dir = std::env::temp_dir().join(format!("wepaper-test-programs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    gl.set_cache_dir(Some(dir.clone()));
    assert!(Program::restore(gl, 42).is_none(), "nothing was saved yet");
    let first = Program::link(gl, VERTEX, FRAGMENT).unwrap();
    first.save(42);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the linked program was saved");
    let second = Program::restore(gl, 42).expect("the saved program comes back");
    assert_eq!(
        second.uniforms.len(),
        first.uniforms.len(),
        "a restored program reflects like a compiled one"
    );
    assert!(Program::restore(gl, 43).is_none(), "another key is another program");
    // A cache file that is garbage is ignored, not trusted.
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    std::fs::write(&file, b"garbage that is no program").unwrap();
    assert!(Program::restore(gl, 42).is_none());
    gl.set_cache_dir(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pool_reuses_returned_targets() {
    let Some(context) = context() else { return };
    let gl = context.gl();
    let mut pool = TargetPool::new(gl);
    let before = gl.memory().targets;
    let a = pool.take(64, 32, TextureFormat::Rgba8).unwrap();
    let b = pool.take(64, 32, TextureFormat::Rgba8).unwrap();
    assert_eq!(gl.memory().targets - before, 2 * 64 * 32 * 4);
    let first = a.texture.raw();
    pool.give(a);
    let again = pool.take(64, 32, TextureFormat::Rgba8).unwrap();
    assert_eq!(again.texture.raw(), first, "a returned target is handed out again");
    assert_eq!(gl.memory().targets - before, 2 * 64 * 32 * 4, "nothing new was allocated");
    let other = pool.take(32, 32, TextureFormat::Rgba8).unwrap();
    assert_ne!(other.texture.raw(), first, "another size is another target");
    assert_eq!(pool.peak_lent, 3);
    pool.give(again);
    pool.give(b);
    pool.give(other);
    assert_eq!(pool.idle(), 3);
    pool.trim();
    assert_eq!((pool.idle(), gl.memory().targets), (0, before), "trimming frees what is not lent");
}

#[test]
fn memory_stats_follow_allocations() {
    let Some(context) = context() else { return };
    let gl = context.gl();
    let before = gl.memory();
    let big = vec![0u8; 16 * 16 * 4];
    let small = vec![0u8; 8 * 8 * 4];
    let texture = Texture::from_levels(
        gl,
        TextureFormat::Rgba8,
        &[(16, 16, &big), (8, 8, &small)],
        Sampling::default(),
    )
    .unwrap();
    assert_eq!(texture.bytes(), 16 * 16 * 4 + 8 * 8 * 4);
    assert_eq!(gl.memory().textures - before.textures, texture.bytes());
    if gl.caps().s3tc {
        let block = vec![0u8; 16];
        let dxt =
            Texture::from_levels(gl, TextureFormat::Dxt5, &[(4, 4, &block)], Sampling::default()).unwrap();
        assert_eq!(dxt.bytes(), 16, "a compressed texture stays compressed: 16 bytes for 16 pixels");
    }
    assert!(
        Texture::from_levels(gl, TextureFormat::Rgba8, &[(16, 16, &small)], Sampling::default()).is_err(),
        "too little data is refused"
    );
    drop(texture);
    assert_eq!(gl.memory().textures, before.textures);
}
