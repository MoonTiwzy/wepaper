//! Which output a wallpaper is heard on.
//!
//! A wallpaper shown on several outputs is drawn on each of them, and each of those
//! copies would play the wallpaper's sounds: a scene's sound layers, a web page's
//! music. Two copies a moment apart are an echo. So a wallpaper sounds on one output
//! only, however many show it. (A video's sound track is not in this: a file is
//! decoded once for every output that shows it.)

use std::path::PathBuf;

/// An output, as far as sound goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Playing {
    /// The output's id.
    pub(super) id: u32,
    /// The wallpaper it shows, if it shows one.
    pub(super) wallpaper: Option<PathBuf>,
    pub(super) paused: bool,
    /// How many wallpapers were loaded on it so far: a wallpaper loaded again is a
    /// new copy to tell, though the output shows the same as before.
    pub(super) loads: u64,
}

/// The outputs that sound: of those showing the same wallpaper, the first that
/// plays; a paused one only when all of them are.
pub(super) fn heard(outputs: &[Playing]) -> Vec<u32> {
    let mut heard: Vec<u32> = Vec::new();
    for output in outputs {
        let Some(wallpaper) = &output.wallpaper else { continue };
        let mut same = outputs.iter().filter(|other| other.wallpaper.as_ref() == Some(wallpaper));
        let first = same.clone().find(|other| !other.paused).or_else(|| same.next());
        if first.is_some_and(|first| first.id == output.id) {
            heard.push(output.id);
        }
    }
    heard
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(id: u32, wallpaper: &str, paused: bool) -> Playing {
        Playing { id, wallpaper: (!wallpaper.is_empty()).then(|| PathBuf::from(wallpaper)), paused, loads: 1 }
    }

    #[test]
    fn a_wallpaper_on_two_outputs_is_heard_on_the_first() {
        assert_eq!(heard(&[output(4, "/w/a", false), output(7, "/w/a", false)]), [4]);
    }

    #[test]
    fn wallpapers_of_their_own_are_each_heard() {
        assert_eq!(
            heard(&[output(4, "/w/a", false), output(7, "/w/b", false), output(9, "/w/a", false)]),
            [4, 7]
        );
    }

    #[test]
    fn a_paused_output_passes_the_sound_to_one_that_plays() {
        assert_eq!(heard(&[output(4, "/w/a", true), output(7, "/w/a", false)]), [7]);
        // All paused: the first keeps it, to go on from where it stopped.
        assert_eq!(heard(&[output(4, "/w/a", true), output(7, "/w/a", true)]), [4]);
    }

    #[test]
    fn an_output_that_shows_nothing_is_not_heard() {
        assert_eq!(heard(&[output(4, "", false), output(7, "/w/a", false)]), [7]);
        assert_eq!(heard(&[]), Vec::<u32>::new());
    }
}
