//! Keyframe animations: a value that follows curves over a timeline.

use wepaper_json::Value;

/// What a timeline does when it reaches its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    /// Stops on the last frame.
    Once,
    /// Starts over.
    Loop,
    /// Plays backwards to the start, then forwards again.
    Mirror,
}

/// One point of a curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// Where on the timeline it is.
    pub frame: f32,
    /// The curve's value there.
    pub value: f32,
    /// The handle towards the next key, as an offset in frames and value, when the
    /// curve leaves this key as a Bezier.
    pub front: Option<[f32; 2]>,
    /// The handle towards the previous key.
    pub back: Option<[f32; 2]>,
}

/// A keyframe animation of a value with one curve per component.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    /// The name scripts find it by.
    pub name: String,
    /// One curve per component of the value; keys in frame order.
    pub curves: Vec<Vec<Key>>,
    /// Frames a second.
    pub fps: f32,
    /// The length of the timeline in frames.
    pub length: f32,
    /// What happens at the end of the timeline.
    pub repeat: Repeat,
    /// The curve continues from its last key round to its first one.
    pub wrap: bool,
    /// It waits for a script to start it.
    pub start_paused: bool,
    /// The curves are offsets from the value written in the document.
    pub relative: bool,
}

impl Animation {
    /// Reads an `animation` object; `None` when it has no usable curve.
    pub fn read(doc: &Value) -> Option<Self> {
        let handle = |doc: &Value| {
            let enabled = doc.get("enabled").as_bool().unwrap_or(false);
            enabled.then(|| [number(doc.get("x"), 0.0), number(doc.get("y"), 0.0)])
        };
        let mut curves = Vec::new();
        for name in ["c0", "c1", "c2", "c3"] {
            let Some(keys) = doc.get(name).as_array() else { break };
            let mut curve: Vec<Key> = keys
                .iter()
                .map(|key| Key {
                    frame: number(key.get("frame"), 0.0),
                    value: number(key.get("value"), 0.0),
                    front: handle(key.get("front")),
                    back: handle(key.get("back")),
                })
                .collect();
            curve.sort_by(|a, b| a.frame.total_cmp(&b.frame));
            curves.push(curve);
        }
        if curves.iter().all(Vec::is_empty) {
            return None;
        }
        let options = doc.get("options");
        let last = curves.iter().filter_map(|curve| curve.last()).map(|key| key.frame).fold(0.0, f32::max);
        Some(Self {
            name: options.get("name").as_str().unwrap_or_default().to_owned(),
            curves,
            fps: number(options.get("fps"), 30.0).max(0.001),
            length: number(options.get("length"), last).max(1.0),
            repeat: match options.get("mode").as_str() {
                Some("loop") => Repeat::Loop,
                Some("mirror") => Repeat::Mirror,
                _ => Repeat::Once,
            },
            wrap: options.get("wraploop").as_bool().unwrap_or(false),
            start_paused: options.get("startpaused").as_bool().unwrap_or(false),
            relative: doc.get("relative").as_bool().unwrap_or(false),
        })
    }

    /// The frame of the timeline that `played` frames of playing lands on.
    pub fn frame_at(&self, played: f32) -> f32 {
        let played = played.max(0.0);
        match self.repeat {
            Repeat::Once => played.min(self.length),
            Repeat::Loop => played % self.length,
            Repeat::Mirror => {
                let there_and_back = played % (self.length * 2.0);
                if there_and_back > self.length { self.length * 2.0 - there_and_back } else { there_and_back }
            }
        }
    }

    /// Whether playing on from `played` frames can still change anything.
    pub fn finished(&self, played: f32) -> bool {
        self.repeat == Repeat::Once && played >= self.length
    }

    /// The value of every component at `frame` of the timeline.
    pub fn sample(&self, frame: f32, out: &mut Vec<f32>) {
        out.clear();
        out.extend(
            self.curves.iter().map(|curve| sample_curve(curve, frame, self.wrap.then_some(self.length))),
        );
    }
}

fn number(doc: &Value, default: f32) -> f32 {
    doc.as_f64().map_or(default, |n| n as f32)
}

/// A curve's value at `frame`. `wrap` is the timeline's length when the curve runs
/// on from its last key to its first.
fn sample_curve(curve: &[Key], frame: f32, wrap: Option<f32>) -> f32 {
    let (Some(first), Some(last)) = (curve.first(), curve.last()) else { return 0.0 };
    let next = curve.partition_point(|key| key.frame <= frame);
    match (next, wrap) {
        (0, Some(length)) if curve.len() > 1 => {
            let before = Key { frame: last.frame - length, ..*last };
            between(&before, first, frame)
        }
        (0, _) => first.value,
        (n, Some(length)) if n == curve.len() && curve.len() > 1 => {
            let after = Key { frame: first.frame + length, ..*first };
            between(last, &after, frame)
        }
        (n, _) if n == curve.len() => last.value,
        (n, _) => between(&curve[n - 1], &curve[n], frame),
    }
}

/// The curve between two neighbouring keys at `frame`.
fn between(from: &Key, to: &Key, frame: f32) -> f32 {
    let span = to.frame - from.frame;
    if span <= f32::EPSILON {
        return to.value;
    }
    if from.front.is_none() && to.back.is_none() {
        return from.value + (to.value - from.value) * ((frame - from.frame) / span);
    }
    // A cubic Bezier through (frame, value). Handles are kept between the keys so
    // that the curve has one value per frame.
    let front = from.front.unwrap_or([0.0, 0.0]);
    let back = to.back.unwrap_or([0.0, 0.0]);
    let x =
        [from.frame, from.frame + front[0].clamp(0.0, span), to.frame + back[0].clamp(-span, 0.0), to.frame];
    let y = [from.value, from.value + front[1], to.value + back[1], to.value];
    let bezier = |p: &[f32; 4], t: f32| {
        let u = 1.0 - t;
        u * u * u * p[0] + 3.0 * u * u * t * p[1] + 3.0 * u * t * t * p[2] + t * t * t * p[3]
    };
    let (mut low, mut high) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if bezier(&x, middle) < frame { low = middle } else { high = middle }
    }
    bezier(&y, (low + high) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn animation(text: &str) -> Animation {
        Animation::read(&wepaper_json::parse(text).unwrap()).unwrap()
    }

    const LINEAR: &str = r#"{"c0":[{"frame":0,"value":0,"front":{"enabled":false,"x":1,"y":0},"back":{"enabled":false,"x":-1,"y":0}},
        {"frame":60,"value":1,"front":{"enabled":false,"x":1,"y":0},"back":{"enabled":false,"x":-1,"y":0}}],
        "options":{"fps":60,"length":120,"mode":"loop","name":"fade"}}"#;

    fn at(animation: &Animation, frame: f32) -> f32 {
        let mut out = Vec::new();
        animation.sample(frame, &mut out);
        out[0]
    }

    #[test]
    fn straight_between_keys_and_flat_outside_them() {
        let a = animation(LINEAR);
        assert_eq!(
            (a.name.as_str(), a.fps, a.length, a.repeat, a.relative),
            ("fade", 60.0, 120.0, Repeat::Loop, false)
        );
        assert_eq!(at(&a, 0.0), 0.0);
        assert!((at(&a, 15.0) - 0.25).abs() < 1e-6);
        assert_eq!(at(&a, 60.0), 1.0);
        assert_eq!(at(&a, 100.0), 1.0);
    }

    #[test]
    fn timeline_ends_as_its_mode_says() {
        let mut a = animation(LINEAR);
        assert_eq!(a.frame_at(130.0), 10.0);
        assert!(!a.finished(500.0));
        a.repeat = Repeat::Mirror;
        assert_eq!(a.frame_at(130.0), 110.0);
        assert_eq!(a.frame_at(250.0), 10.0);
        a.repeat = Repeat::Once;
        assert_eq!(a.frame_at(130.0), 120.0);
        assert!(a.finished(120.0) && !a.finished(119.0));
    }

    #[test]
    fn wrapped_curve_runs_from_the_last_key_to_the_first() {
        let mut a = animation(LINEAR);
        a.wrap = true;
        // From (60, 1) round to (120, 0), which is key 0 one length later.
        assert!((at(&a, 90.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn bezier_handles_bend_the_curve_through_both_keys() {
        let eased = LINEAR
            .replace("\"enabled\":false,\"x\":1,", "\"enabled\":true,\"x\":30,")
            .replace("\"enabled\":false,\"x\":-1,", "\"enabled\":true,\"x\":-30,");
        let a = animation(&eased);
        assert!(at(&a, 0.0).abs() < 1e-4 && (at(&a, 60.0) - 1.0).abs() < 1e-4);
        // Flat handles ease in and out: slow at the ends, symmetric in the middle.
        assert!(at(&a, 6.0) < 0.05, "{}", at(&a, 6.0));
        assert!((at(&a, 30.0) - 0.5).abs() < 1e-3);
        assert!(at(&a, 54.0) > 0.95);
    }

    #[test]
    fn nothing_to_play_is_no_animation() {
        assert!(Animation::read(&wepaper_json::parse(r#"{"options":{"fps":30}}"#).unwrap()).is_none());
        assert!(Animation::read(&wepaper_json::parse(r#"{"c0":[]}"#).unwrap()).is_none());
    }
}
