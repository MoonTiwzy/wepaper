//! Reading wallpapers from disk: archives, layered files, LZ4, texture containers.

use std::path::PathBuf;

use wepaper_engine::read::{Archive, Files, PixelFormat, Pixels, TextureFile, lz4};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wepaper-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn u32le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A package as Wallpaper Engine writes it.
fn archive_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    u32le(&mut out, 8);
    out.extend_from_slice(b"PKGV0019");
    u32le(&mut out, files.len() as u32);
    let mut offset = 0;
    for (name, data) in files {
        u32le(&mut out, name.len() as u32);
        out.extend_from_slice(name.as_bytes());
        u32le(&mut out, offset);
        u32le(&mut out, data.len() as u32);
        offset += data.len() as u32;
    }
    for (_, data) in files {
        out.extend_from_slice(data);
    }
    out
}

fn archive(dir: &std::path::Path, files: &[(&str, &[u8])]) -> Archive {
    let path = dir.join("scene.pkg");
    std::fs::write(&path, archive_bytes(files)).unwrap();
    Archive::open(&path).unwrap()
}

#[test]
fn reads_entries_by_name() {
    let dir = temp("entries");
    let pkg = archive(&dir, &[("scene.json", b"{}"), ("materials/a.tex", b"texture bytes")]);
    assert_eq!(pkg.read("materials/a.tex").unwrap().unwrap(), b"texture bytes");
    assert_eq!(pkg.read("scene.json").unwrap().unwrap(), b"{}");
    assert!(pkg.read("missing").is_none());
    let mut names: Vec<&str> = pkg.names().collect();
    names.sort_unstable();
    assert_eq!(names, ["materials/a.tex", "scene.json"]);
}

#[test]
fn lookup_is_case_insensitive() {
    let dir = temp("case");
    let pkg = archive(&dir, &[("Materials/Фон 1.TEX", b"x")]);
    assert!(pkg.read("materials/фон 1.tex").is_some());
    assert!(pkg.read("MATERIALS\\Фон 1.tex").is_some());
    std::fs::create_dir_all(dir.join("Shaders")).unwrap();
    std::fs::write(dir.join("Shaders/Common.h"), "// header").unwrap();
    let mut files = Files::new();
    files.mount_dir(dir.clone());
    assert_eq!(files.read_text("shaders/common.h").unwrap(), "// header");
    assert!(files.exists("SHADERS/COMMON.H"));
}

#[test]
fn later_mount_shadows_earlier() {
    let (a, b) = (temp("shadow-a"), temp("shadow-b"));
    std::fs::write(a.join("x.json"), "assets").unwrap();
    std::fs::write(a.join("only.json"), "only").unwrap();
    let mut files = Files::new();
    files.mount_dir(a);
    files.mount_archive(archive(&b, &[("x.json", b"wallpaper")]));
    assert_eq!(files.read_text("x.json").unwrap(), "wallpaper");
    assert_eq!(files.read_text("only.json").unwrap(), "only");
    assert!(files.read("nope.json").is_err());
    // The same file under two spellings is one cache identity; different files differ.
    let key = files.locate("x.json").unwrap().key;
    assert_eq!(files.locate("./X.JSON").unwrap().key, key);
    assert_ne!(files.locate("only.json").unwrap().key, key);
}

#[test]
fn truncated_archive_is_an_error() {
    let dir = temp("truncated");
    let bytes = archive_bytes(&[("scene.json", b"{}"), ("b", b"1234")]);
    for cut in [0, 3, 11, 20, 30] {
        let path = dir.join(format!("cut{cut}.pkg"));
        std::fs::write(&path, &bytes[..cut]).unwrap();
        assert!(Archive::open(&path).is_err(), "a package cut at {cut} bytes opened");
    }
    // An entry that points past the end of the file: opens, the read fails.
    let path = dir.join("short.pkg");
    std::fs::write(&path, &bytes[..bytes.len() - 2]).unwrap();
    let pkg = Archive::open(&path).unwrap();
    assert!(pkg.read("b").unwrap().is_err());
}

#[test]
fn memory_layer_serves_generated_files() {
    let mut files = Files::new();
    files.mount_memory([("materials/util/white.json".to_owned(), b"{}".to_vec())]);
    assert_eq!(files.read("Materials/Util/White.json").unwrap(), b"{}");
}

#[test]
fn lz4_decodes_literals_matches_and_overlaps() {
    // 5 literals, then a match of 12 copying from 5 back (overlapping run).
    let block = [0x58, b'a', b'b', b'c', b'd', b'e', 5, 0, 0x10, b'!'];
    assert_eq!(lz4::decompress(&block, 18).unwrap(), b"abcdeabcdeabcdeab!");
    // A long literal run uses length extension bytes.
    let mut long = vec![0xf0, 255, 5];
    long.extend(std::iter::repeat_n(b'z', 15 + 255 + 5));
    assert_eq!(lz4::decompress(&long, 275).unwrap(), vec![b'z'; 275]);
}

#[test]
fn lz4_rejects_overrun() {
    let block = [0x58, b'a', b'b', b'c', b'd', b'e', 5, 0, 0x10, b'!'];
    assert!(lz4::decompress(&block, 10).is_err(), "output larger than promised");
    assert!(lz4::decompress(&block[..4], 18).is_err(), "input cut short");
    assert!(lz4::decompress(&[0x10, b'a', 9, 0], 8).is_err(), "match before the start");
}

/// A TEXB container around `mips`, each `(width, height, bytes, lz4 output size or 0)`.
fn tex_bytes(
    container: &str,
    format: u32,
    flags: u32,
    size: (u32, u32),
    image_kind: i32,
    mips: &[(u32, u32, Vec<u8>, u32)],
) -> Vec<u8> {
    let mut out = b"TEXV0005\0TEXI0001\0".to_vec();
    for v in [format, flags, size.0.next_power_of_two(), size.1.next_power_of_two(), size.0, size.1, 0] {
        u32le(&mut out, v);
    }
    out.extend_from_slice(container.as_bytes());
    out.push(0);
    u32le(&mut out, 1);
    if container != "TEXB0001" && container != "TEXB0002" {
        u32le(&mut out, image_kind as u32);
    }
    if container == "TEXB0004" {
        u32le(&mut out, 0);
    }
    u32le(&mut out, mips.len() as u32);
    for (w, h, data, unpacked) in mips {
        u32le(&mut out, *w);
        u32le(&mut out, *h);
        if container != "TEXB0001" {
            u32le(&mut out, u32::from(*unpacked != 0));
            u32le(&mut out, *unpacked);
        }
        u32le(&mut out, data.len() as u32);
        out.extend_from_slice(data);
    }
    out
}

#[test]
fn parses_raw_and_packed_mips() {
    let packed = vec![0x1f, 7, 1, 0, 39, 0x50, 7, 7, 7, 7, 7]; // 64 bytes of 7
    let bytes = tex_bytes("TEXB0003", 0, 2, (3, 4), -1, &[(4, 4, packed, 64), (2, 2, vec![9; 16], 0)]);
    let tex = TextureFile::parse(&bytes).unwrap();
    assert_eq!((tex.width, tex.height, tex.image_width, tex.image_height), (4, 4, 3, 4));
    assert_eq!(tex.format, PixelFormat::Rgba8);
    assert!(tex.clamp && !tex.nearest && tex.sprites.is_none());
    assert_eq!(tex.images.len(), 1);
    assert_eq!(tex.images[0].mips.len(), 2);
    assert_eq!((tex.images[0].mips[1].width, tex.images[0].mips[1].height), (2, 2));
    match tex.pixels(&bytes, 0, 0).unwrap() {
        Pixels::Raw { format: PixelFormat::Rgba8, data } => assert_eq!(data, vec![7; 64]),
        other => panic!("unexpected {other:?}"),
    }
    match tex.pixels(&bytes, 0, 1).unwrap() {
        Pixels::Raw { data, .. } => assert_eq!(data, vec![9; 16]),
        other => panic!("unexpected {other:?}"),
    }
    assert!(tex.pixels(&bytes, 0, 2).is_err());
}

#[test]
fn embedded_pictures_are_handed_out_encoded() {
    let bytes = tex_bytes("TEXB0003", 0, 0, (8, 8), 13, &[(8, 8, b"\x89PNG....".to_vec(), 0)]);
    let tex = TextureFile::parse(&bytes).unwrap();
    assert!(
        matches!(tex.pixels(&bytes, 0, 0).unwrap(), Pixels::Encoded(data) if data.starts_with(b"\x89PNG"))
    );
}

#[test]
fn old_container_and_compressed_formats() {
    let bytes = tex_bytes("TEXB0001", 4, 1, (8, 8), -1, &[(8, 8, vec![1; 64], 0)]);
    let tex = TextureFile::parse(&bytes).unwrap();
    assert_eq!(tex.format, PixelFormat::Dxt5);
    assert!(tex.nearest);
    assert!(
        matches!(tex.pixels(&bytes, 0, 0).unwrap(), Pixels::Raw { format: PixelFormat::Dxt5, data } if data.len() == 64)
    );
}

#[test]
fn sprite_sheet_frames() {
    let mut bytes = tex_bytes("TEXB0003", 0, 4, (4, 2), -1, &[(4, 2, vec![0; 32], 0)]);
    bytes.extend_from_slice(b"TEXS0003\0");
    for v in [2u32, 2, 2] {
        u32le(&mut bytes, v); // frames, sheet cell size
    }
    for (x, time) in [(0.0f32, 0.1f32), (2.0, 0.25)] {
        u32le(&mut bytes, 0); // image
        for f in [time, x, 0.0, 2.0, 0.0, 0.0, 2.0] {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    let tex = TextureFile::parse(&bytes).unwrap();
    let sheet = tex.sprites.as_ref().unwrap();
    assert_eq!(sheet.frames.len(), 2);
    assert_eq!((sheet.frames[1].x, sheet.frames[1].width, sheet.frames[1].height), (2.0, 2.0, 2.0));
    assert!((sheet.duration() - 0.35).abs() < 1e-6);
    assert_eq!(sheet.frame_at(0.05), 0);
    assert_eq!(sheet.frame_at(0.2), 1);
    assert_eq!(sheet.frame_at(0.36), 0, "the animation loops");
}

#[test]
fn truncated_tex_is_an_error() {
    let bytes = tex_bytes("TEXB0003", 0, 0, (4, 4), -1, &[(4, 4, vec![0; 64], 0)]);
    for cut in [0, 8, 20, 50, bytes.len() - 70] {
        assert!(TextureFile::parse(&bytes[..cut]).is_err(), "a texture cut at {cut} bytes parsed");
    }
    // Cut inside the pixels: the header is fine, the pixels are not there.
    let tex = TextureFile::parse(&bytes[..bytes.len() - 1]);
    assert!(tex.is_err() || tex.unwrap().pixels(&bytes[..bytes.len() - 1], 0, 0).is_err());
    assert!(TextureFile::parse(b"not a texture at all, just some text").is_err());
}
