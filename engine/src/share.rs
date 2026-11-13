//! The resource cache every wallpaper instance draws from.
//!
//! Textures, compiled programs and shader sources are loaded once per process and
//! handed out by reference. An instance holds what it uses; when the last holder
//! goes, the resource goes. A wallpaper shown on three outputs is three instances
//! over one set of textures.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::glsl::{Combos, Declared, Source, Stage};
use crate::gpu::{Gl, Program, Sampling, Texture, TextureFormat};
use crate::read::{FNV_SEED, Files, PixelFormat, Pixels, SpriteSheet, TextureFile, fnv, picture};

/// A texture with what shaders and layers need to know about it.
pub struct SharedTexture {
    /// The texture.
    pub texture: Texture,
    /// The stored size and the size of the picture inside it.
    pub resolution: [f32; 4],
    /// Its frames, when it is animated.
    pub sprites: Option<SpriteSheet>,
    /// The format's number in the shaders' `TEXnFORMAT` combos.
    pub format_code: i32,
}

/// A shader's two stages with their merged declarations.
pub struct ShaderPair {
    /// The vertex stage.
    pub vertex: Source,
    /// The fragment stage.
    pub fragment: Source,
    /// What both stages declare.
    pub declared: Declared,
}

/// The cache. One per engine.
pub struct Share {
    gl: Gl,
    textures: HashMap<u64, Weak<SharedTexture>>,
    programs: HashMap<u64, Weak<Program>>,
    shaders: HashMap<String, Option<Rc<ShaderPair>>>,
    white: Option<Rc<SharedTexture>>,
    /// Texture sides above this are met by dropping the largest levels.
    pub max_texture_side: u32,
}

impl Share {
    /// An empty cache.
    pub fn new(gl: &Gl) -> Self {
        Self { gl: gl.clone(), textures: HashMap::new(), programs: HashMap::new(), shaders: HashMap::new(), white: None, max_texture_side: u32::MAX }
    }

    /// A 1x1 white texture: what a slot samples when its texture is missing.
    pub fn white(&mut self) -> Rc<SharedTexture> {
        if let Some(white) = &self.white {
            return white.clone();
        }
        let texture = Texture::from_levels(&self.gl, TextureFormat::Rgba8, &[(1, 1, &[255, 255, 255, 255])], Sampling::default())
            .expect("a 1x1 texture always uploads");
        let white = Rc::new(SharedTexture { texture, resolution: [1.0; 4], sprites: None, format_code: 0 });
        self.white = Some(white.clone());
        white
    }

    /// The texture a material calls `name`, loaded once however many ask.
    pub fn texture(&mut self, files: &Files, name: &str) -> Result<Rc<SharedTexture>, String> {
        let path = texture_path(files, name).ok_or_else(|| format!("texture {name} is not in the wallpaper or the assets"))?;
        let located = files.locate(&path).expect("just found");
        let key = fnv(located.key, &self.max_texture_side.to_le_bytes());
        if let Some(texture) = self.textures.get(&key).and_then(Weak::upgrade) {
            return Ok(texture);
        }
        let bytes = files.read(&path).map_err(|e| e.to_string())?;
        let texture = Rc::new(upload(&self.gl, &bytes, self.max_texture_side).map_err(|e| format!("texture {name}: {e}"))?);
        self.textures.insert(key, Rc::downgrade(&texture));
        Ok(texture)
    }

    /// The shader called `name` (`shaders/<name>.vert` and `.frag`), read once.
    pub fn shader(&mut self, files: &Files, name: &str) -> Option<Rc<ShaderPair>> {
        if let Some(pair) = self.shaders.get(name) {
            return pair.clone();
        }
        let include = |file: &str| files.read_text(&format!("shaders/{file}")).ok();
        let load = |stage: Stage, extension: &str| {
            let text = files.read_text(&format!("shaders/{name}.{extension}")).ok()?;
            Some(Source::load(stage, &text, &include))
        };
        let pair = load(Stage::Vertex, "vert").zip(load(Stage::Fragment, "frag")).map(|(vertex, fragment)| {
            let mut declared = fragment.declared.clone();
            declared.merge(vertex.declared.clone());
            Rc::new(ShaderPair { vertex, fragment, declared })
        });
        self.shaders.insert(name.to_owned(), pair.clone());
        pair
    }

    /// The program of `pair` with `combos`, compiled once per distinct source.
    ///
    /// A program linked by an earlier run is restored from disk. Otherwise the
    /// shader is compiled as standard GLSL; what the driver refuses is repaired by
    /// its own message where that is possible, and where the driver has a lenient
    /// mode the shader gets a second chance in it.
    pub fn program(&mut self, pair: &ShaderPair, combos: &Combos) -> Result<Rc<Program>, String> {
        let (vertex, fragment) = (pair.vertex.assemble(combos, false), pair.fragment.assemble(combos, false));
        let key = fnv(fnv(fnv(FNV_SEED, vertex.as_bytes()), &[0]), fragment.as_bytes());
        if let Some(program) = self.programs.get(&key).and_then(Weak::upgrade) {
            return Ok(program);
        }
        let program = match Program::restore(&self.gl, key) {
            Some(program) => program,
            None => {
                let program = self.compile(pair, combos, vertex, fragment, key)?;
                program.save(key);
                program
            }
        };
        let program = Rc::new(program);
        self.programs.insert(key, Rc::downgrade(&program));
        Ok(program)
    }

    fn compile(&self, pair: &ShaderPair, combos: &Combos, vertex: String, fragment: String, key: u64) -> Result<Program, String> {
        /// How many repairs one shader gets before it is given up on.
        const REPAIRS: usize = 8;
        let mut error = None;
        let lenient = self.gl.caps().lenient_shaders;
        for mode in [false, true] {
            if mode && !lenient {
                break;
            }
            let (mut vertex, mut fragment) = if mode { (pair.vertex.assemble(combos, true), pair.fragment.assemble(combos, true)) } else { (vertex.clone(), fragment.clone()) };
            for _ in 0..=REPAIRS {
                match Program::link(&self.gl, &vertex, &fragment) {
                    Ok(program) => return Ok(program),
                    Err(refused) => {
                        let source = if refused.stage == "vertex" { &mut vertex } else { &mut fragment };
                        let repaired = refused.stage != "link" && crate::glsl::repair(source, &refused.log);
                        if !repaired {
                            // With debug logging on, the source the driver refused is kept for a look.
                            if log::log_enabled!(log::Level::Debug) {
                                let dir = std::env::temp_dir().join("wepaper-shaders");
                                let _ = std::fs::create_dir_all(&dir);
                                let name = format!("{key:016x}{}", if mode { "-lenient" } else { "" });
                                let _ = std::fs::write(dir.join(format!("{name}.vert")), &vertex);
                                let _ = std::fs::write(dir.join(format!("{name}.frag")), &fragment);
                                log::debug!("refused shader kept as {}/{name}.{{vert,frag}}: {refused}", dir.display());
                            }
                            error = Some(refused);
                            break;
                        }
                    }
                }
            }
        }
        Err(error.map_or_else(|| "the shader did not compile".to_owned(), |e| e.to_string()))
    }

    /// Drops the entries nobody holds any more.
    pub fn sweep(&mut self) {
        self.textures.retain(|_, texture| texture.strong_count() > 0);
        self.programs.retain(|_, program| program.strong_count() > 0);
    }

    /// How many textures are alive.
    pub fn live_textures(&self) -> usize {
        self.textures.values().filter(|t| t.strong_count() > 0).count()
    }
}

/// Where the texture a material calls `name` is.
fn texture_path(files: &Files, name: &str) -> Option<String> {
    [format!("materials/{name}.tex"), format!("{name}.tex"), name.to_owned()].into_iter().find(|path| files.exists(path))
}

/// Parses a texture container and puts it on the GPU.
fn upload(gl: &Gl, bytes: &[u8], max_side: u32) -> Result<SharedTexture, String> {
    let file = TextureFile::parse(bytes).map_err(|e| e.to_string())?;
    let image = file.images.first().filter(|i| !i.mips.is_empty()).ok_or("the texture has no picture")?;
    let sampling = Sampling { clamp: file.clamp, nearest: file.nearest };
    // Levels larger than the budget are never read, let alone uploaded.
    let first = image.mips.iter().position(|m| m.width.max(m.height) <= max_side).unwrap_or(image.mips.len() - 1);
    let mut levels: Vec<(u32, u32, std::borrow::Cow<'_, [u8]>)> = Vec::new();
    let mut format = TextureFormat::Rgba8;
    for (level, mip) in image.mips.iter().enumerate().skip(first) {
        match file.pixels(bytes, 0, level).map_err(|e| e.to_string())? {
            Pixels::Raw { format: pixel_format, data } => {
                format = match pixel_format {
                    PixelFormat::Rgba8 => TextureFormat::Rgba8,
                    PixelFormat::Rg8 => TextureFormat::Rg8,
                    PixelFormat::R8 => TextureFormat::R8,
                    PixelFormat::Dxt1 => TextureFormat::Dxt1,
                    PixelFormat::Dxt3 => TextureFormat::Dxt3,
                    PixelFormat::Dxt5 => TextureFormat::Dxt5,
                };
                levels.push((mip.width, mip.height, data));
            }
            Pixels::Encoded(data) => {
                let picture = picture::decode(data).map_err(|e| e.to_string())?;
                levels.push((picture.width, picture.height, picture.rgba.into()));
                break;
            }
            Pixels::Video(_) => return Err("video textures are not loaded as pictures".to_owned()),
        }
    }
    let slices: Vec<(u32, u32, &[u8])> = levels.iter().map(|(w, h, data)| (*w, *h, data.as_ref())).collect();
    let texture = Texture::from_levels(gl, format, &slices, sampling).map_err(|e| e.to_string())?;
    let format_code = match format {
        TextureFormat::Dxt5 => 4,
        TextureFormat::Dxt3 => 6,
        TextureFormat::Dxt1 => 7,
        TextureFormat::Rg8 => 8,
        TextureFormat::R8 => 9,
        _ => 0,
    };
    let resolution = [file.width as f32, file.height as f32, file.image_width as f32, file.image_height as f32];
    Ok(SharedTexture { texture, resolution, sprites: file.sprites, format_code })
}
