//! What the system's media players are playing, for wallpapers that show it.
//!
//! Players announce themselves over MPRIS. Following them is done by `playerctl` in
//! a child process, read by one thread, for the reason sound is captured by a child:
//! a D-Bus stack linked into the engine would cost every user of it, for a feature
//! few wallpapers have. Nothing runs until a playing wallpaper asks.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// What a player is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Playback {
    /// Nothing plays, or there is no player.
    #[default]
    Stopped,
    /// A track is playing.
    Playing,
    /// A track is paused.
    Paused,
}

/// The track the system's player is on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NowPlaying {
    /// Whether it plays, is paused, or nothing is.
    pub playback: Playback,
    /// The track's title.
    pub title: String,
    /// Who it is by.
    pub artist: String,
    /// The album it is on.
    pub album: String,
}

impl NowPlaying {
    /// Reads one line of what `playerctl` was asked to print: the fields, tab-separated.
    fn parse(line: &str) -> Self {
        let mut fields = line.split('\t').map(str::trim);
        let mut next = || fields.next().unwrap_or("").to_owned();
        let playback = match next().as_str() {
            "Playing" => Playback::Playing,
            "Paused" => Playback::Paused,
            _ => Playback::Stopped,
        };
        Self { playback, title: next(), artist: next(), album: next() }
    }
}

/// `playerctl`, following the players, and the newest thing it said.
struct Follower {
    child: Child,
    /// The newest state not yet taken.
    said: Arc<Mutex<Option<NowPlaying>>>,
}

impl Follower {
    fn start() -> Option<Self> {
        let mut child = Command::new("playerctl")
            .args(["--follow", "metadata", "--format", "{{status}}\t{{title}}\t{{artist}}\t{{album}}"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .inspect_err(|error| log::warn!("no media integration: playerctl could not be started ({error})"))
            .ok()?;
        let pipe = child.stdout.take()?;
        let said = Arc::new(Mutex::new(None));
        let shared = said.clone();
        let spawned = std::thread::Builder::new().name("wepaper-media".to_owned()).spawn(move || {
            // Ends when the child does: the pipe closes and the lines run out.
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let Ok(mut said) = shared.lock() else { break };
                *said = Some(NowPlaying::parse(&line));
            }
        });
        if spawned.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        Some(Self { child, said })
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What is playing on the system, kept up to date while somebody wants to know.
#[derive(Default)]
pub struct Media {
    follower: Option<Follower>,
    /// `playerctl` could not be started: it is not tried again.
    unavailable: bool,
    now: NowPlaying,
    /// Goes up every time `now` changes; 0 until anything was heard.
    serial: u64,
}

impl Media {
    /// Brings the state up to date. `wanted` is whether anything shown reads it; while
    /// nothing does, nothing follows the players.
    pub fn update(&mut self, wanted: bool) {
        if !wanted {
            self.follower = None;
            return;
        }
        if self.follower.is_none() && !self.unavailable {
            self.follower = Follower::start();
            self.unavailable = self.follower.is_none();
        }
        // A follower that went away (no session bus) is let go of, not left behind dead.
        if self.follower.as_mut().is_some_and(|follower| !matches!(follower.child.try_wait(), Ok(None))) {
            log::warn!("no media integration: playerctl stopped");
            self.follower = None;
            self.unavailable = true;
        }
        let heard = self.follower.as_ref().and_then(|follower| follower.said.lock().ok()?.take());
        if let Some(heard) = heard
            && (heard != self.now || self.serial == 0)
        {
            self.now = heard;
            self.serial += 1;
        }
    }

    /// The track the player is on, and a number that changes whenever it does (0
    /// while nothing was heard yet).
    pub fn now(&self) -> (&NowPlaying, u64) {
        (&self.now, self.serial)
    }

    /// Whether the players are being followed right now.
    pub fn following(&self) -> bool {
        self.follower.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_of_fields_is_a_track() {
        let track = NowPlaying::parse("Playing\ta song\tsomebody\tan album");
        assert_eq!(
            (track.playback, track.title.as_str(), track.artist.as_str(), track.album.as_str()),
            (Playback::Playing, "a song", "somebody", "an album")
        );
        assert_eq!(NowPlaying::parse("Paused\tsong").playback, Playback::Paused);
        // No player prints an empty line; anything unknown is nothing playing.
        assert_eq!(NowPlaying::parse(""), NowPlaying::default());
        assert_eq!(NowPlaying::parse("Buffering\tx\ty\tz").playback, Playback::Stopped);
    }

    #[test]
    fn nothing_follows_until_somebody_asks() {
        let mut media = Media::default();
        media.update(false);
        assert!(!media.following());
        assert_eq!(media.now(), (&NowPlaying::default(), 0));
    }
}
