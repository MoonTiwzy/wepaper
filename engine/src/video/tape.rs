//! A video as a tape of itself, so that it loops without a seam.
//!
//! A player told to loop a file seeks back to its start when the file ends: it waits
//! to be sure the last frame is over, empties its decoders and starts again, and the
//! picture stands still for several frames. Given the file many times in a row as
//! one long edit list, the same player reads from the end of one copy into the start
//! of the next like through any other cut: the frames keep their pace. The tape is
//! long enough to play for hours; only at its very end does the player seek.
//!
//! The cuts have to be exact, to less than a frame, and that needs the length of the
//! picture rather than of the file (a sound track often runs a little longer). MP4
//! and MOV files say it; a file of another kind plays as itself, with the seek.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// How long a tape plays before the player has to seek back, at the least.
const HOURS: f64 = 12.0;
/// The most copies on a tape: each costs the player about a kilobyte.
const MOST: u32 = 2048;
/// More boxes than this in a file and it is not read further.
const BOXES: u32 = 100_000;
/// The least a copy is cut short by, in seconds: far above what adding thousands of
/// lengths together loses, far below anything a frame's time is counted in.
const HAIR: f64 = 1e-9;

/// Where in a file its picture starts and how long it lasts, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Span {
    pub(crate) start: f64,
    pub(crate) length: f64,
    /// How many units a second the file counts the picture's time in.
    pub(crate) scale: u32,
}

/// What a player is given to play a video in a loop.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tape {
    /// What the player opens: an edit list, or the file itself.
    pub(crate) address: Vec<u8>,
    /// How long one copy on the tape lasts; `None` when the address is the file.
    pub(crate) copy: Option<f64>,
}

/// The units a second the player counts a frame's time in, for a file that counts in
/// `scale`: the file's own, made finer when they are coarser than a thousandth.
fn ticks(scale: u32) -> f64 {
    f64::from(if scale >= 1000 { scale } else { scale.saturating_mul(1000u32.div_ceil(scale.max(1))) })
}

/// How many copies of a video go on its tape: enough for hours, and no more than
/// can each be cut a [`HAIR`] short with the cuts still landing on the right tick.
fn repeats(span: Span) -> u32 {
    let wanted = (HOURS * 3600.0 / span.length).ceil().min(f64::from(MOST));
    let exact = (0.25 / (ticks(span.scale) * HAIR)).floor();
    wanted.min(exact) as u32
}

/// The tape of `path`: the file many times in a row as one edit list, every copy cut
/// to the picture, so that a sound track that starts earlier or runs longer does not
/// hold a frame. The copies are named `alias`, another name of the same file: the
/// player keeps the list and every name on it in memory several times over, so a
/// short one is megabytes less.
///
/// The player counts a frame's time in whole ticks and throws away a frame that then
/// falls before the first moment of its copy, which the first frame of every copy
/// would do half of the time: the copies' starts are sums of lengths, and those do
/// not land on ticks. So each copy is cut short by so little that through all the
/// copies its start stays within a quarter of a tick before the frame: the frame
/// rounds up, into its copy.
pub(crate) fn of(path: &Path, alias: &Path) -> Tape {
    let name = alias.as_os_str().as_bytes();
    let whole = || Tape { address: path.as_os_str().as_bytes().to_vec(), copy: None };
    let Some(span) = picture_span(path) else { return whole() };
    let repeats = repeats(span);
    if repeats < 2 {
        return whole();
    }
    let copy = span.length - 0.25 / (ticks(span.scale) * f64::from(repeats));
    let mut entry = format!("%{}%", name.len()).into_bytes();
    entry.extend_from_slice(name);
    // As many digits as it takes to say exactly these numbers.
    entry.extend_from_slice(format!(",{},{copy};", span.start).as_bytes());
    // A chapter a copy would be thousands of them, each with the file's name.
    let mut address = b"edl://!no_chapters;".to_vec();
    address.reserve(entry.len() * repeats as usize);
    for _ in 0..repeats {
        address.extend_from_slice(&entry);
    }
    Tape { address, copy: Some(copy) }
}

/// Where a video is in its own time when a tape of copies this long is at `position`.
pub(crate) fn within(position: f64, copy: Option<f64>) -> f64 {
    match copy {
        _ if !position.is_finite() => 0.0,
        Some(copy) if copy > 0.0 => position.rem_euclid(copy),
        _ => position.max(0.0),
    }
}

/// The box that starts at `at`: its kind, and where its body starts and ends.
fn header(file: &mut File, at: u64, limit: u64) -> Option<([u8; 4], u64, u64)> {
    if at.checked_add(8)? > limit {
        return None;
    }
    let mut head = [0u8; 8];
    file.seek(SeekFrom::Start(at)).ok()?;
    file.read_exact(&mut head).ok()?;
    let kind = [head[4], head[5], head[6], head[7]];
    let (body, end) = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) {
        // To the end of what holds it.
        0 => (at + 8, limit),
        // The size did not fit in four bytes: eight more follow.
        1 => {
            file.read_exact(&mut head).ok()?;
            (at.checked_add(16)?, at.checked_add(u64::from_be_bytes(head))?)
        }
        size => (at + 8, at + u64::from(size)),
    };
    (body <= end && end <= limit).then_some((kind, body, end))
}

/// The body of the first box of `kind` between `at` and `limit`.
fn find(file: &mut File, mut at: u64, limit: u64, kind: &[u8; 4], budget: &mut u32) -> Option<(u64, u64)> {
    loop {
        *budget = budget.checked_sub(1)?;
        let (found, body, end) = header(file, at, limit)?;
        if &found == kind {
            return Some((body, end));
        }
        at = end;
    }
}

/// `count` bytes at `at`.
fn bytes<const N: usize>(file: &mut File, at: u64, limit: u64) -> Option<[u8; N]> {
    if at.checked_add(N as u64)? > limit {
        return None;
    }
    let mut read = [0u8; N];
    file.seek(SeekFrom::Start(at)).ok()?;
    file.read_exact(&mut read).ok()?;
    Some(read)
}

/// Where the picture of an MP4 or MOV file starts and how long it lasts: the
/// durations of its frames added up, which is what the file says of its video track,
/// after the hole its edit list may put before it. `None` for any other kind of
/// file, and for one whose edit list cuts inside the track.
pub(crate) fn picture_span(path: &Path) -> Option<Span> {
    let mut file = File::open(path).ok()?;
    let file = &mut file;
    let end = file.metadata().ok()?.len();
    let mut budget = BOXES;
    let (movie, movie_end) = find(file, 0, end, b"moov", &mut budget)?;
    let mut at = movie;
    loop {
        budget = budget.checked_sub(1)?;
        let (kind, track, track_end) = header(file, at, movie_end)?;
        at = track_end;
        if &kind != b"trak" {
            continue;
        }
        let Some((media, media_end)) = find(file, track, track_end, b"mdia", &mut budget) else { continue };
        // What the track holds: version and flags, four bytes of nothing, then the kind.
        let Some((handler, handler_end)) = find(file, media, media_end, b"hdlr", &mut budget) else {
            continue;
        };
        if bytes::<4>(file, handler + 8, handler_end)? != *b"vide" {
            continue;
        }
        // The edit list: holes (a start of -1) that delay the track, then the track
        // itself. Lengths are in the movie's units, not the track's.
        let mut hole = 0u64;
        if let Some((edits, edits_end)) = find(file, track, track_end, b"edts", &mut budget)
            && let Some((list, list_end)) = find(file, edits, edits_end, b"elst", &mut budget)
        {
            let [version, _, _, _, count @ ..] = bytes::<8>(file, list, list_end)?;
            let count = u64::from(u32::from_be_bytes(count));
            let mut shown = 0;
            for index in 0..count.min(u64::from(BOXES)) {
                let (length, start) = if version == 0 {
                    let entry = list + 8 + index * 12;
                    let length = u32::from_be_bytes(bytes(file, entry, list_end)?);
                    (u64::from(length), i64::from(i32::from_be_bytes(bytes(file, entry + 4, list_end)?)))
                } else {
                    let entry = list + 8 + index * 20;
                    let length = u64::from_be_bytes(bytes(file, entry, list_end)?);
                    (length, i64::from_be_bytes(bytes(file, entry + 8, list_end)?))
                };
                match (start < 0, shown) {
                    (true, 0) => hole = hole.checked_add(length)?,
                    (false, 0) => shown = 1,
                    // A hole or a second piece after the track began: cuts inside it,
                    // and its length on screen is not its media's.
                    _ => return None,
                }
            }
        }
        let start = if hole == 0 {
            0.0
        } else {
            // Version, flags, two dates (four or eight bytes each), then the scale.
            let (header, header_end) = find(file, movie, movie_end, b"mvhd", &mut budget)?;
            let [version] = bytes::<1>(file, header, header_end)?;
            let scale =
                u32::from_be_bytes(bytes(file, header + if version == 0 { 12 } else { 20 }, header_end)?);
            (scale > 0).then(|| hole as f64 / f64::from(scale))?
        };
        let (header, header_end) = find(file, media, media_end, b"mdhd", &mut budget)?;
        let [version] = bytes::<1>(file, header, header_end)?;
        // Version, flags, two dates (four or eight bytes each), then the time scale
        // and the duration in its units.
        let (scale, duration) = if version == 0 {
            let scale = u32::from_be_bytes(bytes(file, header + 12, header_end)?);
            let duration = u32::from_be_bytes(bytes(file, header + 16, header_end)?);
            (scale, (duration != u32::MAX).then_some(u64::from(duration))?)
        } else {
            let scale = u32::from_be_bytes(bytes(file, header + 20, header_end)?);
            let duration = u64::from_be_bytes(bytes(file, header + 24, header_end)?);
            (scale, (duration != u64::MAX).then_some(duration)?)
        };
        return (scale > 0 && duration > 0).then(|| Span {
            start,
            length: duration as f64 / f64::from(scale),
            scale,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box: its size, its kind, its body.
    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn handler(kind: &[u8; 4]) -> Vec<u8> {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(kind);
        body.extend_from_slice(&[0u8; 13]);
        boxed(b"hdlr", &body)
    }

    /// A media header of version 0: `duration` units of `1 / scale` seconds.
    fn media_header(scale: u32, duration: u32) -> Vec<u8> {
        let mut body = vec![0u8; 12];
        body.extend_from_slice(&scale.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
        body.extend_from_slice(&[0u8; 4]);
        boxed(b"mdhd", &body)
    }

    fn track(kind: &[u8; 4], scale: u32, duration: u32, edits: &[u8]) -> Vec<u8> {
        let media = boxed(b"mdia", &[media_header(scale, duration), handler(kind)].concat());
        boxed(b"trak", &[boxed(b"tkhd", &[0u8; 84]), edits.to_vec(), media].concat())
    }

    /// An edit list of version 0 with these (length, start) entries.
    fn edits(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut body = vec![0u8; 4];
        body.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (length, start) in entries {
            body.extend_from_slice(&length.to_be_bytes());
            body.extend_from_slice(&start.to_be_bytes());
            body.extend_from_slice(&[0, 1, 0, 0]);
        }
        boxed(b"edts", &boxed(b"elst", &body))
    }

    fn file(name: &str, tracks: &[Vec<u8>]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("wepaper-tape-{}-{name}.mp4", std::process::id()));
        // The movie counts in thousandths of a second.
        let mut movie_header = vec![0u8; 100];
        movie_header[12..16].copy_from_slice(&1000u32.to_be_bytes());
        let movie = boxed(b"moov", &[boxed(b"mvhd", &movie_header), tracks.concat()].concat());
        // The frames come first, as in a file that was not made for streaming.
        std::fs::write(&path, [boxed(b"ftyp", b"isom\0\0\0\0"), boxed(b"mdat", &[7u8; 300]), movie].concat())
            .expect("written");
        path
    }

    fn span_of(name: &str, tracks: &[Vec<u8>]) -> Option<Span> {
        let path = file(name, tracks);
        let span = picture_span(&path);
        let _ = std::fs::remove_file(path);
        span
    }

    #[test]
    fn the_picture_is_as_long_as_its_frames_not_as_the_sound() {
        // 442 frames at 60 a second, and a sound track a little longer.
        let tracks =
            [track(b"soun", 44100, 325_634, &[]), track(b"vide", 15360, 113_152, &edits(&[(7367, 512)]))];
        let span = span_of("both", &tracks).expect("a span");
        assert!(
            span.start == 0.0 && (span.length - 442.0 / 60.0).abs() < 1e-9 && span.scale == 15360,
            "{span:?}"
        );
    }

    #[test]
    fn a_hole_before_the_picture_is_where_it_starts() {
        // Forty thousandths of nothing, then ten seconds of frames.
        let held = track(b"vide", 600, 6000, &edits(&[(40, -1), (10000, 20)]));
        assert_eq!(span_of("held", &[held]), Some(Span { start: 0.04, length: 10.0, scale: 600 }));
    }

    #[test]
    fn a_file_without_a_picture_or_with_cuts_has_no_span() {
        assert_eq!(span_of("sound", &[track(b"soun", 44100, 44100, &[])]), None);
        // Two pieces of the track, and a hole in the middle of it.
        assert_eq!(span_of("pieces", &[track(b"vide", 600, 6000, &edits(&[(5000, 0), (5000, 3000)]))]), None);
        assert_eq!(span_of("gap", &[track(b"vide", 600, 6000, &edits(&[(5000, 0), (100, -1)]))]), None);
        assert_eq!(span_of("empty", &[track(b"vide", 600, 0, &[])]), None);
        assert_eq!(picture_span(Path::new("/nonexistent/video.mp4")), None);
    }

    #[test]
    fn a_file_that_is_not_made_of_boxes_has_no_span() {
        let path = std::env::temp_dir().join(format!("wepaper-tape-{}-noise.webm", std::process::id()));
        std::fs::write(&path, [0x1a, 0x45, 0xdf, 0xa3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).expect("written");
        assert_eq!(picture_span(&path), None);
        // A box that says it is larger than the file.
        std::fs::write(&path, &[boxed(b"moov", &[0u8; 8]), vec![0xff; 3]].concat()[..12]).expect("written");
        assert_eq!(picture_span(&path), None);
        let _ = std::fs::remove_file(path);
    }

    /// Against a real library, when `WEPAPER_TEST_WORKSHOP` names one and `ffprobe`
    /// is there to ask: the span read here is the one FFmpeg reads.
    #[test]
    fn the_span_agrees_with_ffprobe_on_a_real_library() {
        let Some(library) = std::env::var_os("WEPAPER_TEST_WORKSHOP").map(std::path::PathBuf::from) else {
            eprintln!("skipped: WEPAPER_TEST_WORKSHOP is not set");
            return;
        };
        let mut compared = 0;
        for folder in std::fs::read_dir(library).into_iter().flatten().flatten() {
            for entry in std::fs::read_dir(folder.path()).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("mp4")) {
                    continue;
                }
                let asked = std::process::Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-select_streams",
                        "v:0",
                        "-show_entries",
                        "stream=start_time,duration",
                    ])
                    .args(["-of", "csv=p=0"])
                    .arg(&path)
                    .output();
                let Ok(asked) = asked else {
                    eprintln!("skipped: no ffprobe");
                    return;
                };
                let said = String::from_utf8_lossy(&asked.stdout);
                let mut said = said.trim().split(',').map(str::parse::<f64>);
                let (Some(Ok(start)), Some(Ok(length))) = (said.next(), said.next()) else { continue };
                let ours = picture_span(&path);
                let same = |a: f64, b: f64| (a - b).abs() < 0.001;
                assert!(
                    ours.is_some_and(|ours| same(ours.start, start) && same(ours.length, length)),
                    "{}: {ours:?}, ffprobe starts at {start} for {length}",
                    path.display()
                );
                compared += 1;
            }
        }
        eprintln!("{compared} videos compared");
    }

    #[test]
    fn the_tape_repeats_the_file_with_its_name_guarded() {
        // Ten seconds: the hours ask for 4320 copies, and get the most there are.
        let tracks = [track(b"vide", 90000, 900_000, &edits(&[(40, -1), (10000, 0)]))];
        let path = file("a, b;c", &tracks);
        let tape = of(&path, Path::new("/short/a, b"));
        let _ = std::fs::remove_file(&path);
        let name = "/short/a, b";
        let copy = tape.copy.expect("a tape");
        let entry = format!("%{}%{name},0.04,{copy};", name.len());
        let address = String::from_utf8(tape.address).expect("text");
        assert!(address.starts_with(&format!("edl://!no_chapters;{entry}{entry}")), "{address:.200}");
        assert_eq!(address.len(), "edl://!no_chapters;".len() + entry.len() * MOST as usize);
        assert!(copy < 10.0 && 10.0 - copy < 1e-8, "{copy}");
    }

    #[test]
    fn every_copy_starts_within_a_quarter_tick_before_its_first_frame() {
        for (scale, duration) in
            [(15360u32, 113_152u32), (90000, 663_000), (600, 4420), (30, 221), (1000, 7367)]
        {
            let span = Span { start: 0.0, length: f64::from(duration) / f64::from(scale), scale };
            let path = file(&format!("ticks-{scale}"), &[track(b"vide", scale, duration, &[])]);
            let copy = of(&path, &path).copy.expect("a tape");
            let _ = std::fs::remove_file(&path);
            let ticks = ticks(scale);
            // The player adds the copies' lengths up one by one, as here.
            let mut start = 0.0f64;
            for index in 1..=repeats(span) {
                start += copy;
                let frame = (start * ticks).round() / ticks;
                let truth = f64::from(index) * span.length;
                assert!(frame >= start, "{scale}: copy {index} would lose its first frame");
                assert!(
                    (frame - truth).abs() < 0.5 / ticks,
                    "{scale}: copy {index} starts at {frame}, not {truth}"
                );
            }
        }
    }

    #[test]
    fn a_file_that_does_not_say_its_picture_plays_as_itself() {
        let path = std::env::temp_dir().join(format!("wepaper-tape-{}-plain.webm", std::process::id()));
        std::fs::write(&path, [0x1a, 0x45, 0xdf, 0xa3]).expect("written");
        let tape = of(&path, Path::new("/short"));
        let _ = std::fs::remove_file(&path);
        assert_eq!(tape, Tape { address: path.as_os_str().as_bytes().to_vec(), copy: None });
    }

    #[test]
    fn the_place_on_a_tape_is_a_place_in_the_video() {
        assert!((within(3.0 * 7.5 + 2.25, Some(7.5)) - 2.25).abs() < 1e-9);
        assert_eq!(within(4.0, None), 4.0);
        assert_eq!(within(f64::NAN, Some(7.5)), 0.0);
    }

    #[test]
    fn a_tape_plays_for_hours_and_stays_exact() {
        let span = |length: f64, scale: u32| Span { start: 0.0, length, scale };
        assert_eq!(repeats(span(3600.0, 90000)), 12);
        assert_eq!(repeats(span(7.37, 15360)), MOST);
        // A file that counts finely cannot have as many exact cuts.
        assert_eq!(repeats(span(7.37, 600_000)), 416);
        assert_eq!(repeats(span(7.37, 1_000_000)), 250);
        // And one that counts in thousand-millionths has none: no tape.
        assert_eq!(repeats(span(7.37, 1_000_000_000)), 0);
    }
}
