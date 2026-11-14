//! The resource cache every wallpaper instance draws from.
//!
//! Textures, compiled programs and shader sources are loaded once per process and
//! handed out by reference. An instance holds what it uses; when the last holder
//! goes, the resource goes. A wallpaper shown on three outputs is three instances
//! over one set of textures.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::glsl::{Combos, Declared, Source, Stage};
use crate::gpu::{Gl, Mesh, Program, Sampling, Texture, TextureFormat};
use crate::read::puppet::Puppet;
use crate::read::{FNV_SEED, Files, PixelFormat, Pixels, SpriteSheet, TextureFile, fnv, picture};
use crate::video::{SharedVideo, VideoHub};

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

/// A puppet and its mesh on the GPU, shared by every layer that is warped with it.
pub struct SharedPuppet {
    /// The puppet as read: its bones and animations.
    pub puppet: Puppet,
    /// Its mesh, in place of an image's quad: texture coordinates across the picture,
    /// as a layer's own buffers are laid out.
    pub mesh: Mesh,
    /// The same mesh for drawing straight from the layer's texture, when that is
    /// padded to a power of two and so is larger than its picture.
    pub padded: Option<Mesh>,
}

impl SharedPuppet {
    /// The mesh that draws the picture straight from its texture.
    pub fn texture_mesh(&self) -> &Mesh {
        self.padded.as_ref().unwrap_or(&self.mesh)
    }
}

/// What a texture name leads to.
pub enum Loaded {
    /// A picture on the GPU.
    Picture(Rc<SharedTexture>),
    /// A video, playing.
    Video(SharedVideo),
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
    puppets: HashMap<u64, Weak<SharedPuppet>>,
    /// The shaders read while a wallpaper loads, by the two files each is made of: a
    /// name is not enough, every wallpaper carries its own copies of the effects it
    /// uses and an author may have changed his. Emptied once the wallpaper is loaded.
    shaders: HashMap<(u64, u64), Option<Rc<ShaderPair>>>,
    white: Option<Rc<SharedTexture>>,
    /// Texture sides above this are met by dropping the largest levels.
    pub max_texture_side: u32,
    /// The fonts text layers hold, by file (a system font by its name).
    fonts: HashMap<u64, Option<Rc<ab_glyph::FontVec>>>,
    /// The videos being played.
    pub videos: VideoHub,
    /// Where videos packed inside textures are unpacked to.
    pub video_dir: std::path::PathBuf,
}

impl Share {
    /// An empty cache.
    pub fn new(gl: &Gl) -> Self {
        Self {
            gl: gl.clone(),
            textures: HashMap::new(),
            programs: HashMap::new(),
            puppets: HashMap::new(),
            shaders: HashMap::new(),
            white: None,
            fonts: HashMap::new(),
            max_texture_side: u32::MAX,
            videos: VideoHub::new(gl),
            video_dir: std::env::temp_dir().join("wepaper-video"),
        }
    }

    /// A 1x1 white texture: what a slot samples when its texture is missing.
    pub fn white(&mut self) -> Rc<SharedTexture> {
        if let Some(white) = &self.white {
            return white.clone();
        }
        let texture = Texture::from_levels(
            &self.gl,
            TextureFormat::Rgba8,
            &[(1, 1, &[255, 255, 255, 255])],
            Sampling::default(),
        )
        .expect("a 1x1 texture always uploads");
        let white = Rc::new(SharedTexture { texture, resolution: [1.0; 4], sprites: None, format_code: 0 });
        self.white = Some(white.clone());
        white
    }

    /// The texture a material calls `name`, loaded once however many ask.
    pub fn texture(&mut self, files: &Files, name: &str) -> Result<Loaded, String> {
        let path = texture_path(files, name)
            .ok_or_else(|| format!("texture {name} is not in the wallpaper or the assets"))?;
        let located = files.locate(&path).expect("just found");
        let key = fnv(located.key, &self.max_texture_side.to_le_bytes());
        if let Some(texture) = self.textures.get(&key).and_then(Weak::upgrade) {
            return Ok(Loaded::Picture(texture));
        }
        let bytes = files.read(&path).map_err(|e| e.to_string())?;
        let file = TextureFile::parse(&bytes).map_err(|e| format!("texture {name}: {e}"))?;
        if file.video {
            return self
                .video(&file, &bytes, located.key)
                .map(Loaded::Video)
                .map_err(|e| format!("texture {name}: {e}"));
        }
        let texture = Rc::new(
            upload(&self.gl, &file, &bytes, self.max_texture_side)
                .map_err(|e| format!("texture {name}: {e}"))?,
        );
        self.textures.insert(key, Rc::downgrade(&texture));
        Ok(Loaded::Picture(texture))
    }

    /// The puppet at `path`, read and put on the GPU once however many layers use it.
    ///
    /// `extent` is how much of its texture the layer's picture fills: a puppet's
    /// texture coordinates count across the picture, and a texture padded to a power
    /// of two is larger than its picture.
    pub fn puppet(
        &mut self,
        files: &Files,
        path: &str,
        extent: [f32; 2],
    ) -> Result<Rc<SharedPuppet>, String> {
        let located = files.locate(path).ok_or_else(|| format!("puppet {path} is not in the wallpaper"))?;
        let key = fnv(fnv(located.key, &extent[0].to_le_bytes()), &extent[1].to_le_bytes());
        if let Some(puppet) = self.puppets.get(&key).and_then(Weak::upgrade) {
            return Ok(puppet);
        }
        let bytes = files.read(path).map_err(|e| e.to_string())?;
        let puppet = Puppet::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
        // The file's indices are 16 bits wide; anything larger was refused on reading.
        let indices: Vec<u16> = puppet.indices.iter().map(|&index| index as u16).collect();
        let bones: Vec<[u32; 4]> = puppet.vertices.iter().map(|v| v.bones).collect();
        let mesh = |across: [f32; 2]| {
            let vertices: Vec<f32> = puppet
                .vertices
                .iter()
                .flat_map(|v| {
                    let ([x, y, z], [u, t], [a, b, c, d]) = (v.position, v.uv, v.weights);
                    [x, y, z, u * across[0], t * across[1], a, b, c, d]
                })
                .collect();
            Mesh::new(&self.gl, &[(0, 3), (1, 2), (8, 4)], &vertices, Some(&indices), false)
                .and_then(|mesh| mesh.with_integers(7, &bones))
                .map_err(|e| format!("{path}: {e}"))
        };
        let padded = if extent == [1.0, 1.0] { None } else { Some(mesh(extent)?) };
        let mesh = mesh([1.0, 1.0])?;
        let puppet = Rc::new(SharedPuppet { puppet, mesh, padded });
        self.puppets.insert(key, Rc::downgrade(&puppet));
        Ok(puppet)
    }

    /// Plays the video packed in a texture: unpacked once into the cache, then
    /// played from there by the one decoder everybody shares.
    fn video(&mut self, file: &TextureFile, bytes: &[u8], key: u64) -> Result<SharedVideo, String> {
        let unpacked = self.video_dir.join(format!("{key:016x}.mp4"));
        if !unpacked.is_file() {
            let Pixels::Video(data) = file.pixels(bytes, 0, 0).map_err(|e| e.to_string())? else {
                return Err("no video data".to_owned());
            };
            std::fs::create_dir_all(&self.video_dir).map_err(|e| e.to_string())?;
            let part = unpacked.with_extension(format!("{}.part", std::process::id()));
            std::fs::write(&part, data)
                .and_then(|()| std::fs::rename(&part, &unpacked))
                .map_err(|e| e.to_string())?;
        }
        // A picture, not something to listen to.
        self.videos.open(&unpacked, false).map_err(|e| e.to_string())
    }

    /// A path on disk for a file of the wallpaper: the file itself when it is one,
    /// else a copy unpacked from the archive into the cache. Players that take a
    /// file name (libmpv) need one.
    pub fn on_disk(&self, files: &Files, path: &str) -> Result<std::path::PathBuf, String> {
        let located = files.locate(path).ok_or_else(|| format!("{path} is not in the wallpaper"))?;
        if let Some(disk) = located.disk {
            return Ok(disk);
        }
        let extension = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("bin");
        let unpacked = self.video_dir.join(format!("{:016x}.{extension}", located.key));
        if !unpacked.is_file() {
            let data = files.read(path).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&self.video_dir).map_err(|e| e.to_string())?;
            let part = unpacked.with_extension(format!("{}.part", std::process::id()));
            std::fs::write(&part, data)
                .and_then(|()| std::fs::rename(&part, &unpacked))
                .map_err(|e| e.to_string())?;
        }
        Ok(unpacked)
    }

    /// The font a text layer names: a file of the wallpaper or the assets, or a
    /// system font (`systemfont_arial`). Loaded once; `None` when there is no such font.
    pub fn font(&mut self, files: &Files, name: &str) -> Option<Rc<ab_glyph::FontVec>> {
        let key = files.locate(name).map_or_else(|| fnv(FNV_SEED, name.as_bytes()), |file| file.key);
        if let Some(font) = self.fonts.get(&key) {
            return font.clone();
        }
        let bytes = files.read(name).ok().or_else(|| std::fs::read(crate::scene::system_font(name)?).ok());
        // Only the file is kept: glyphs are read out of it as text needs them, so a
        // font of thirty thousand glyphs costs its eight megabytes and no more.
        let font = bytes.and_then(|bytes| match ab_glyph::FontVec::try_from_vec(bytes) {
            Ok(font) => Some(Rc::new(font)),
            Err(error) => {
                log::warn!("font {name}: {error}");
                None
            }
        });
        self.fonts.insert(key, font.clone());
        font
    }

    /// The shader called `name` (`shaders/<name>.vert` and `.frag`), read once for
    /// all the layers of a wallpaper that use it.
    pub fn shader(&mut self, files: &Files, name: &str) -> Option<Rc<ShaderPair>> {
        let file =
            |extension: &str| files.locate(&format!("shaders/{name}.{extension}")).map_or(0, |f| f.key);
        let key = (file("vert"), file("frag"));
        if let Some(pair) = self.shaders.get(&key) {
            return pair.clone();
        }
        let include = |file: &str| files.read_text(&format!("shaders/{file}")).ok();
        let load = |stage: Stage, extension: &str| {
            let text = files.read_text(&format!("shaders/{name}.{extension}")).ok()?;
            Some(Source::load(stage, &text, &include))
        };
        let pair =
            load(Stage::Vertex, "vert").zip(load(Stage::Fragment, "frag")).map(|(vertex, fragment)| {
                let mut declared = fragment.declared.clone();
                declared.merge(vertex.declared.clone());
                Rc::new(ShaderPair { vertex, fragment, declared })
            });
        self.shaders.insert(key, pair.clone());
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

    fn compile(
        &self,
        pair: &ShaderPair,
        combos: &Combos,
        vertex: String,
        fragment: String,
        key: u64,
    ) -> Result<Program, String> {
        /// How many repairs one shader gets before it is given up on. A strict driver
        /// names one mistake at a time, and a shader written for a lenient compiler
        /// has one on every other line.
        const REPAIRS: usize = 48;
        let mut error = None;
        let lenient = self.gl.caps().lenient_shaders;
        for mode in [false, true] {
            if mode && !lenient {
                break;
            }
            let (mut vertex, mut fragment) = if mode {
                (pair.vertex.assemble(combos, true), pair.fragment.assemble(combos, true))
            } else {
                (vertex.clone(), fragment.clone())
            };
            for _ in 0..=REPAIRS {
                match Program::link(&self.gl, &vertex, &fragment) {
                    Ok(program) => return Ok(program),
                    Err(refused) => {
                        let repaired = match refused.stage {
                            "vertex" => crate::glsl::repair(&mut vertex, &refused.log),
                            "link" => crate::glsl::repair_link(&mut fragment, &refused.log),
                            _ => crate::glsl::repair(&mut fragment, &refused.log),
                        };
                        if !repaired {
                            // With debug logging on, the source the driver refused is kept for a look.
                            if log::log_enabled!(log::Level::Debug) {
                                let dir = std::env::temp_dir().join("wepaper-shaders");
                                let _ = std::fs::create_dir_all(&dir);
                                let name = format!("{key:016x}{}", if mode { "-lenient" } else { "" });
                                let _ = std::fs::write(dir.join(format!("{name}.vert")), &vertex);
                                let _ = std::fs::write(dir.join(format!("{name}.frag")), &fragment);
                                log::debug!(
                                    "refused shader kept as {}/{name}.{{vert,frag}}: {refused}",
                                    dir.display()
                                );
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
        self.puppets.retain(|_, puppet| puppet.strong_count() > 0);
        // Shader sources are for compiling; what is compiled does not need them.
        self.shaders = HashMap::new();
        // A font nobody writes with is megabytes of file.
        self.fonts.retain(|_, font| font.as_ref().is_some_and(|font| Rc::strong_count(font) > 1));
        self.fonts.shrink_to_fit();
    }

    /// How many textures are alive.
    pub fn live_textures(&self) -> usize {
        self.textures.values().filter(|t| t.strong_count() > 0).count()
    }
}

/// Where the texture a material calls `name` is.
fn texture_path(files: &Files, name: &str) -> Option<String> {
    [format!("materials/{name}.tex"), format!("{name}.tex"), name.to_owned()]
        .into_iter()
        .find(|path| files.exists(path))
}

/// Puts a parsed texture container on the GPU.
fn upload(gl: &Gl, file: &TextureFile, bytes: &[u8], max_side: u32) -> Result<SharedTexture, String> {
    let image = file.images.first().filter(|i| !i.mips.is_empty()).ok_or("the texture has no picture")?;
    let sampling = Sampling { clamp: file.clamp, nearest: file.nearest };
    // Levels larger than the budget are never read, let alone uploaded.
    let first =
        image.mips.iter().position(|m| m.width.max(m.height) <= max_side).unwrap_or(image.mips.len() - 1);
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
    // A picture stored encoded is as large as the picture; raw levels are padded.
    let stored = if file.is_encoded() { (levels[0].0, levels[0].1) } else { (file.width, file.height) };
    let picture = (file.image_width.min(stored.0), file.image_height.min(stored.1));
    let resolution = [stored.0 as f32, stored.1 as f32, picture.0 as f32, picture.1 as f32];
    Ok(SharedTexture { texture, resolution, sprites: file.sprites.clone(), format_code })
}
