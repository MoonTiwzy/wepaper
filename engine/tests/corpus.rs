//! Checks against a real library. They run when these are set and are skipped otherwise:
//!
//! - `WEPAPER_TEST_ASSETS`: Wallpaper Engine's `assets` folder
//! - `WEPAPER_TEST_WORKSHOP`: a folder of wallpapers (`steamapps/workshop/content/431960`)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wepaper_engine::read::{Archive, Pixels, TextureFile, picture};

fn env_dir(name: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(name)?);
    dir.is_dir().then_some(dir)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Parses a texture and decodes its smallest level; returns a label for the tally.
fn check_texture(name: &str, bytes: &[u8]) -> Result<String, String> {
    let tex = TextureFile::parse(bytes).map_err(|e| format!("{name}: {e}"))?;
    let image = tex.images.first().ok_or_else(|| format!("{name}: no images"))?;
    let last = image.mips.len().checked_sub(1).ok_or_else(|| format!("{name}: no levels"))?;
    let label = match tex.pixels(bytes, 0, last).map_err(|e| format!("{name}: {e}"))? {
        Pixels::Raw { format, data } => {
            let mip = &image.mips[last];
            let expected = format.level_size(mip.width, mip.height);
            if data.len() != expected {
                return Err(format!("{name}: level {last} is {} bytes, {expected} expected ({format:?} {}x{})", data.len(), mip.width, mip.height));
            }
            format!("{format:?}")
        }
        Pixels::Encoded(data) => {
            picture::decode(data).map_err(|e| format!("{name}: {e}"))?;
            "picture".to_owned()
        }
        Pixels::Video(data) => {
            if data.len() < 12 || &data[4..8] != b"ftyp" {
                return Err(format!("{name}: video is not an MP4"));
            }
            "video".to_owned()
        }
    };
    Ok(if tex.sprites.is_some() { format!("{label}+sprites") } else { label })
}

#[test]
fn every_package_and_texture_reads() {
    let (Some(assets), Some(workshop)) = (env_dir("WEPAPER_TEST_ASSETS"), env_dir("WEPAPER_TEST_WORKSHOP")) else {
        eprintln!("skipped: WEPAPER_TEST_ASSETS / WEPAPER_TEST_WORKSHOP are not set");
        return;
    };
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut count = |result: Result<String, String>| match result {
        Ok(label) => *tally.entry(label).or_default() += 1,
        Err(why) => failures.push(why),
    };

    let mut files = Vec::new();
    walk(&assets, &mut files);
    walk(&workshop, &mut files);
    let mut packages = 0;
    for path in &files {
        let name = path.to_string_lossy();
        match path.extension().and_then(|e| e.to_str()) {
            Some("tex") => count(check_texture(&name, &std::fs::read(path).unwrap())),
            Some("pkg") => {
                packages += 1;
                let archive = match Archive::open(path) {
                    Ok(archive) => archive,
                    Err(e) => {
                        count(Err(format!("{name}: {e}")));
                        continue;
                    }
                };
                let names: Vec<String> = archive.names().map(str::to_owned).collect();
                for entry in names {
                    match archive.read(&entry).expect("listed entry") {
                        Ok(bytes) if entry.ends_with(".tex") => count(check_texture(&format!("{name}:{entry}"), &bytes)),
                        Ok(_) => {}
                        Err(e) => count(Err(format!("{name}:{entry}: {e}"))),
                    }
                }
            }
            _ => {}
        }
    }
    eprintln!("{packages} packages; textures: {tally:?}");
    assert!(failures.is_empty(), "{} failures, first ones:\n{}", failures.len(), failures.iter().take(15).cloned().collect::<Vec<_>>().join("\n"));
    assert!(packages > 0 && !tally.is_empty(), "nothing was checked");
}
