//! The layered filesystem.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use super::{Archive, FNV_SEED, fnv, normalize};

enum Layer {
    Dir(PathBuf),
    Archive(Archive),
    Memory(HashMap<String, Vec<u8>>),
}

/// Where a lookup found its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// The file's identity for caches: the same for every spelling of its path,
    /// different for the same path in another layer.
    pub key: u64,
    /// The file itself, when it is a plain file on disk and not inside an archive.
    pub disk: Option<PathBuf>,
}

/// Folders, archives and generated files stacked into one tree.
///
/// What was mounted last is looked in first, so a wallpaper's own files shadow the
/// stock assets under them. Paths are compared without regard to case or the kind
/// of slash, as wallpapers written on Windows expect.
#[derive(Default)]
pub struct Files {
    layers: Vec<Layer>,
}

impl Files {
    /// An empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts a folder on top.
    pub fn mount_dir(&mut self, dir: PathBuf) {
        self.layers.push(Layer::Dir(dir));
    }

    /// Puts an archive on top.
    pub fn mount_archive(&mut self, archive: Archive) {
        self.layers.push(Layer::Archive(archive));
    }

    /// Puts files that exist only in memory on top.
    pub fn mount_memory(&mut self, files: impl IntoIterator<Item = (String, Vec<u8>)>) {
        self.layers
            .push(Layer::Memory(files.into_iter().map(|(name, data)| (normalize(&name).1, data)).collect()));
    }

    /// The contents of `path` from the topmost layer that has it.
    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let (clean, key) = normalize(path);
        for layer in self.layers.iter().rev() {
            match layer {
                Layer::Dir(root) => {
                    if let Some(found) = find_in_dir(root, &clean) {
                        return std::fs::read(found);
                    }
                }
                Layer::Archive(archive) => {
                    if let Some(result) = archive.read(&key) {
                        return result;
                    }
                }
                Layer::Memory(files) => {
                    if let Some(data) = files.get(&key) {
                        return Ok(data.clone());
                    }
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no such file in the wallpaper or the assets: {clean}"),
        ))
    }

    /// The contents of `path` as text, without a byte order mark.
    pub fn read_text(&self, path: &str) -> io::Result<String> {
        let bytes = self.read(path)?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
    }

    /// Whether any layer has `path`.
    pub fn exists(&self, path: &str) -> bool {
        self.locate(path).is_some()
    }

    /// Which file a lookup of `path` lands on.
    pub fn locate(&self, path: &str) -> Option<Located> {
        let (clean, key) = normalize(path);
        for (depth, layer) in self.layers.iter().enumerate().rev() {
            match layer {
                Layer::Dir(root) => {
                    if let Some(found) = find_in_dir(root, &clean) {
                        let seed = fnv(FNV_SEED, root.to_string_lossy().as_bytes());
                        return Some(Located { key: fnv(seed, key.as_bytes()), disk: Some(found) });
                    }
                }
                Layer::Archive(archive) => {
                    if archive.contains(&key) {
                        return Some(Located { key: fnv(archive.key(), key.as_bytes()), disk: None });
                    }
                }
                Layer::Memory(files) => {
                    if files.contains_key(&key) {
                        let seed = fnv(FNV_SEED, &(depth as u64).to_le_bytes());
                        return Some(Located { key: fnv(seed, key.as_bytes()), disk: None });
                    }
                }
            }
        }
        None
    }
}

/// `relative` under `root`, whatever case its parts are written in on disk.
fn find_in_dir(root: &Path, relative: &str) -> Option<PathBuf> {
    let direct = root.join(relative);
    if direct.is_file() {
        return Some(direct);
    }
    let mut current = root.to_owned();
    for part in relative.split('/') {
        let exact = current.join(part);
        if exact.exists() {
            current = exact;
            continue;
        }
        let wanted = part.to_lowercase();
        let entry = std::fs::read_dir(&current)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().to_lowercase() == wanted)?;
        current = entry.path();
    }
    current.is_file().then_some(current)
}
