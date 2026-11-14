//! `.pkg` archives.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::{FNV_SEED, fnv, normalize};

/// More entries than this is a corrupt header, not a wallpaper.
const MAX_ENTRIES: u32 = 1 << 20;

struct Entry {
    name: String,
    offset: u64,
    length: u64,
}

/// A Wallpaper Engine package: a table of names followed by the files themselves.
///
/// Only the table is read when the archive is opened. Each file is read from its
/// place on disk when asked for, so a 400 MB package costs its table in memory.
pub struct Archive {
    file: File,
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    key: u64,
}

impl Archive {
    /// Opens the archive at `path` and reads its table.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        let mut reader = BufReader::new(&file);
        let mut consumed = 0u64;
        let magic = read_string(&mut reader, &mut consumed, 32)?;
        if !magic.starts_with("PKGV") {
            return Err(invalid("not a package: no PKGV header"));
        }
        let count = read_u32(&mut reader, &mut consumed)?;
        if count > MAX_ENTRIES {
            return Err(invalid("package table is corrupt"));
        }
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = read_string(&mut reader, &mut consumed, 4096)?;
            let offset = u64::from(read_u32(&mut reader, &mut consumed)?);
            let length = u64::from(read_u32(&mut reader, &mut consumed)?);
            entries.push(Entry { name, offset, length });
        }
        // Offsets in the table count from the end of the table.
        let mut index = HashMap::with_capacity(entries.len());
        for (i, entry) in entries.iter_mut().enumerate() {
            entry.offset += consumed;
            // A table that points past the end of the file is not to be believed, and
            // must not decide how much memory a read asks for.
            if entry.offset.checked_add(entry.length).is_none_or(|end| end > meta.len()) {
                return Err(invalid("package table points outside the file"));
            }
            index.insert(normalize(&entry.name).1, i);
        }
        let modified =
            meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        let mut key = fnv(FNV_SEED, path.to_string_lossy().as_bytes());
        key = fnv(key, &meta.len().to_le_bytes());
        key = fnv(key, &modified.to_le_bytes());
        Ok(Self { file, entries, index, key })
    }

    /// The names in the archive, as written in it.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    /// Whether the archive has `name` (compared case-insensitively).
    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(&normalize(name).1)
    }

    /// The contents of `name`; `None` when the archive has no such file.
    pub fn read(&self, name: &str) -> Option<io::Result<Vec<u8>>> {
        let entry = &self.entries[*self.index.get(&normalize(name).1)?];
        let mut data = vec![0; entry.length as usize];
        Some(self.file.read_exact_at(&mut data, entry.offset).map(|()| data))
    }

    /// An identity that changes when the file on disk does: what caches key on.
    pub fn key(&self) -> u64 {
        self.key
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_u32(reader: &mut impl Read, consumed: &mut u64) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    *consumed += 4;
    Ok(u32::from_le_bytes(bytes))
}

fn read_string(reader: &mut impl Read, consumed: &mut u64, limit: u32) -> io::Result<String> {
    let length = read_u32(reader, consumed)?;
    if length > limit {
        return Err(invalid("package table is corrupt"));
    }
    let mut bytes = vec![0; length as usize];
    reader.read_exact(&mut bytes)?;
    *consumed += u64::from(length);
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
