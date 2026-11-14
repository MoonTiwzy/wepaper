//! A scene's sound layer: what it plays, and when.
//!
//! A layer has one or more files and a way of playing them: `loop` plays them one
//! after another for ever, `single` plays the first once, `random` plays one picked at
//! random, waits a random while and picks again. A layer that starts silent plays
//! nothing until a script tells it to; scripts play, pause and stop any layer.
//!
//! There is a player only while the layer is heard: sound is on, the layer is visible
//! and it is not stopped. A scene with sixty silent layers has no players at all.

use std::path::PathBuf;

use crate::particles::Rng;

/// What plays a layer's files: libmpv in the engine, a recorder in the tests.
pub(crate) trait Player: Sized {
    /// Starts playing `files` in order, over and over when `looping`, at `volume`
    /// (0 to 100). `None` when nothing can play them.
    fn play(files: &[PathBuf], looping: bool, volume: f32) -> Option<Self>;
    fn set_volume(&mut self, volume: f32);
    fn set_paused(&mut self, paused: bool);
    /// Whether it is done: what it played once came to its end, or it could not be
    /// played at all. A looping player is only ever done the second way.
    fn ended(&mut self) -> bool;
}

/// How long after a sound could not be played it is tried again: the sound server may
/// not be up yet when the desktop starts.
const RETRY: f64 = 5.0;

/// How a layer plays its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Loop,
    Single,
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Stopped,
    Playing,
    Paused,
}

/// Whether the layer is heard this frame, and how.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hearing {
    /// Sound is on and the layer is visible.
    pub(crate) audible: bool,
    /// The engine's volume, 0 to 100.
    pub(crate) volume: f32,
    /// The scene stands still.
    pub(crate) paused: bool,
}

/// A sound layer as it plays.
pub(crate) struct SoundLayer<P = crate::audio::Sound> {
    files: Vec<PathBuf>,
    mode: Mode,
    /// The shortest and the longest wait between two random plays, in seconds.
    wait: (f32, f32),
    /// The layer's own volume, 0 to 1.
    pub(crate) volume: f32,
    state: State,
    player: Option<P>,
    /// Nothing starts before this moment of the scene's time: the wait between two
    /// random plays, or before another try at what would not play. `None`: no wait is
    /// set, and a random layer has its first one to draw.
    next_at: Option<f64>,
    rng: Rng,
}

impl<P: Player> SoundLayer<P> {
    /// `mode` is the scene's word for it (`loop`, `single`, `random`); `seed` tells the
    /// random plays of one layer from another's.
    pub(crate) fn new(
        files: Vec<PathBuf>,
        mode: &str,
        wait: (f32, f32),
        volume: f32,
        start_silent: bool,
        seed: u64,
    ) -> Self {
        let mode = match mode {
            "single" => Mode::Single,
            "random" => Mode::Random,
            _ => Mode::Loop,
        };
        Self {
            files,
            mode,
            wait: (wait.0.max(0.0), wait.1.max(wait.0).max(0.0)),
            volume: volume.clamp(0.0, 1.0),
            state: if start_silent { State::Stopped } else { State::Playing },
            player: None,
            next_at: None,
            rng: Rng::new(seed),
        }
    }

    /// A script's `play()`: a stopped layer starts over, a paused one goes on.
    pub(crate) fn play(&mut self) {
        if self.state == State::Stopped {
            // A random layer told to play plays now, not after its wait.
            self.next_at = Some(f64::MIN);
        }
        self.state = State::Playing;
    }

    /// A script's `pause()`.
    pub(crate) fn pause(&mut self) {
        if self.state == State::Playing {
            self.state = State::Paused;
        }
    }

    /// A script's `stop()`.
    pub(crate) fn stop(&mut self) {
        self.state = State::Stopped;
        self.player = None;
        self.next_at = None;
    }

    /// A script's `isPlaying()`: what the layer was told, heard or not.
    pub(crate) fn is_playing(&self) -> bool {
        self.state == State::Playing
    }

    /// Whether a player runs for the layer right now.
    pub(crate) fn sounding(&self) -> bool {
        self.player.is_some()
    }

    /// The scene stands still or moves again, between two frames.
    pub(crate) fn hold(&mut self, paused: bool) {
        let paused = paused || self.state == State::Paused;
        if let Some(player) = &mut self.player {
            player.set_paused(paused);
        }
    }

    /// Brings the player in line with what the layer is to do at `now`, the scene's
    /// time in seconds. Run every frame.
    pub(crate) fn tick(&mut self, now: f64, hearing: Hearing) {
        if !hearing.audible || self.state == State::Stopped || self.files.is_empty() {
            self.player = None;
            return;
        }
        let level = hearing.volume * self.volume;
        let paused = hearing.paused || self.state == State::Paused;
        if let Some(player) = &mut self.player {
            if player.ended() {
                self.player = None;
                match self.mode {
                    Mode::Random => self.next_at = Some(now + f64::from(self.drawn_wait())),
                    Mode::Single => self.state = State::Stopped,
                    // A loop has no end: it could not be played.
                    Mode::Loop => self.next_at = Some(now + RETRY),
                }
                return;
            }
            player.set_volume(level);
            player.set_paused(paused);
            return;
        }
        if paused {
            return;
        }
        if self.mode == Mode::Random && self.next_at.is_none() {
            self.next_at = Some(now + f64::from(self.wait.0));
        }
        if self.next_at.is_some_and(|at| now < at) {
            return;
        }
        let (files, looping) = match self.mode {
            Mode::Loop => (self.files.clone(), true),
            Mode::Single => (self.files[..1].to_vec(), false),
            Mode::Random => {
                let pick = (self.rng.next_u32() as usize) % self.files.len();
                (vec![self.files[pick].clone()], false)
            }
        };
        self.player = P::play(&files, looping, level);
        // Nothing plays sound right now: asked again in a while, not every frame.
        self.next_at = self.player.is_none().then_some(now + RETRY);
    }

    fn drawn_wait(&mut self) -> f32 {
        self.rng.between(self.wait.0, self.wait.1, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        /// What the players of this test were told, in order.
        static TOLD: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// Whether the next `ended` says the file is over.
        static OVER: RefCell<bool> = const { RefCell::new(false) };
    }

    struct Recorder;

    impl Player for Recorder {
        fn play(files: &[PathBuf], looping: bool, volume: f32) -> Option<Self> {
            let names: Vec<_> = files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
            told(format!("play {} loop={looping} volume={volume}", names.join("+")));
            Some(Recorder)
        }
        fn set_volume(&mut self, _: f32) {}
        fn set_paused(&mut self, paused: bool) {
            told(format!("paused={paused}"));
        }
        fn ended(&mut self) -> bool {
            OVER.with(|over| over.replace(false))
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            told("gone".to_owned());
        }
    }

    fn told(what: String) {
        TOLD.with(|all| all.borrow_mut().push(what));
    }

    /// What was told since the last look, without the "not paused" every frame tells.
    fn heard() -> Vec<String> {
        TOLD.with(|all| all.borrow_mut().drain(..).filter(|what| what != "paused=false").collect())
    }

    fn layer(files: &[&str], mode: &str, start_silent: bool) -> SoundLayer<Recorder> {
        heard();
        SoundLayer::new(files.iter().map(PathBuf::from).collect(), mode, (2.0, 2.0), 0.5, start_silent, 7)
    }

    const ON: Hearing = Hearing { audible: true, volume: 80.0, paused: false };
    const OFF: Hearing = Hearing { audible: false, volume: 80.0, paused: false };

    #[test]
    fn a_layer_plays_all_its_files_in_a_loop_at_its_share_of_the_volume() {
        let mut sound = layer(&["a.mp3", "b.mp3"], "loop", false);
        sound.tick(0.0, ON);
        sound.tick(0.1, ON);
        assert_eq!(heard(), ["play a.mp3+b.mp3 loop=true volume=40"]);
    }

    #[test]
    fn a_layer_that_starts_silent_plays_nothing_until_a_script_plays_it() {
        let mut sound = layer(&["a.mp3"], "loop", true);
        sound.tick(0.0, ON);
        assert!(heard().is_empty());
        assert!(!sound.is_playing());
        sound.play();
        sound.tick(0.1, ON);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40"]);
        assert!(sound.is_playing());
    }

    #[test]
    fn a_stopped_layer_has_no_player_and_starts_over_when_played() {
        let mut sound = layer(&["a.mp3"], "loop", false);
        sound.tick(0.0, ON);
        sound.stop();
        sound.tick(0.1, ON);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40", "gone"]);
        sound.play();
        sound.tick(0.2, ON);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40"]);
    }

    #[test]
    fn a_paused_layer_keeps_its_player_and_goes_on() {
        let mut sound = layer(&["a.mp3"], "loop", false);
        sound.tick(0.0, ON);
        sound.pause();
        sound.tick(0.1, ON);
        assert!(!sound.is_playing());
        sound.play();
        sound.tick(0.2, ON);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40", "paused=true"]);
    }

    #[test]
    fn with_sound_off_there_is_no_player() {
        let mut sound = layer(&["a.mp3"], "loop", false);
        sound.tick(0.0, OFF);
        assert!(heard().is_empty());
        sound.tick(0.1, ON);
        sound.tick(0.2, OFF);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40", "gone"]);
        // The layer still counts as playing: scripts do not hear of the volume knob.
        assert!(sound.is_playing());
    }

    #[test]
    fn a_single_play_is_over_when_its_file_is() {
        let mut sound = layer(&["a.mp3", "b.mp3"], "single", false);
        sound.tick(0.0, ON);
        OVER.with(|over| over.replace(true));
        sound.tick(1.0, ON);
        sound.tick(2.0, ON);
        assert_eq!(heard(), ["play a.mp3 loop=false volume=40", "gone"]);
        assert!(!sound.is_playing());
    }

    #[test]
    fn a_random_layer_waits_between_one_file_and_the_next() {
        let mut sound = layer(&["a.mp3"], "random", false);
        // Not before its shortest wait is over.
        sound.tick(0.0, ON);
        sound.tick(1.9, ON);
        assert!(heard().is_empty());
        sound.tick(2.0, ON);
        assert_eq!(heard(), ["play a.mp3 loop=false volume=40"]);
        OVER.with(|over| over.replace(true));
        sound.tick(5.0, ON);
        sound.tick(6.9, ON);
        assert_eq!(heard(), ["gone"]);
        sound.tick(7.0, ON);
        assert_eq!(heard(), ["play a.mp3 loop=false volume=40"]);
    }

    #[test]
    fn a_loop_that_could_not_be_played_is_tried_again_in_a_while() {
        let mut sound = layer(&["a.mp3"], "loop", false);
        sound.tick(0.0, ON);
        OVER.with(|over| over.replace(true));
        sound.tick(1.0, ON);
        sound.tick(1.0 + RETRY - 0.1, ON);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40", "gone"]);
        assert!(sound.is_playing());
        sound.tick(1.0 + RETRY, ON);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40"]);
    }

    #[test]
    fn a_random_layer_a_script_plays_starts_at_once() {
        let mut sound = layer(&["a.mp3"], "random", true);
        sound.play();
        sound.tick(0.0, ON);
        assert_eq!(heard(), ["play a.mp3 loop=false volume=40"]);
    }

    #[test]
    fn a_scene_that_stands_still_pauses_what_plays_and_starts_nothing() {
        let mut sound = layer(&["a.mp3"], "loop", false);
        sound.tick(0.0, Hearing { paused: true, ..ON });
        assert!(heard().is_empty());
        sound.tick(0.1, ON);
        sound.hold(true);
        assert_eq!(heard(), ["play a.mp3 loop=true volume=40", "paused=true"]);
    }
}
