//! How an output shows its wallpaper, whatever the wallpaper is.
//!
//! The speed it plays at, how loud it is next to the others, which part of it the
//! output shows (enlarged, moved), and what is done to the finished picture: mirrored,
//! its colours shifted. The part shown is the wallpaper's own framing: the wallpaper
//! is drawn that way, so what the fill cropped comes into view and nothing is blurred.
//! The mirror and the colours are one pass over the frame, and an output without them
//! has no such pass and no buffer for it.

use wepaper_engine::post::PostPass;
use wepaper_json::{Value, obj};

/// One output's look. `Default` is the plain one: the wallpaper as it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// How fast the wallpaper plays: 1 as it was made, 0 stands still.
    pub rate: f32,
    /// The wallpaper's share of the volume, 0 to 1.
    pub volume: f32,
    /// Mirrored left to right.
    pub flip: bool,
    /// Enlarged about the output's middle: 1 as the fill has it, 2 twice as large,
    /// 0.5 half as large (more of the wallpaper, and its background around it).
    pub zoom: f32,
    /// Moved, in widths and heights of the output: 0.5 is half a screen right or up.
    pub shift: [f32; 2],
    /// Colours: 1 is as it is for the first three, the hue turns in degrees.
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub hue: f32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            rate: 1.0,
            volume: 1.0,
            flip: false,
            zoom: 1.0,
            shift: [0.0, 0.0],
            brightness: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            hue: 0.0,
        }
    }
}

/// The names of a look's values, as the control tree has them.
pub const KEYS: [&str; 10] =
    ["rate", "volume", "flip", "zoom", "shift_x", "shift_y", "brightness", "contrast", "saturation", "hue"];

impl Look {
    /// Whether the picture is left as the wallpaper drew it: no pass is needed.
    /// (The zoom and the shifts are the wallpaper's framing, not the pass's.)
    pub fn plain_picture(&self) -> bool {
        let plain = Self::default();
        Self { rate: plain.rate, volume: plain.volume, zoom: plain.zoom, shift: plain.shift, ..*self }
            == plain
    }

    /// The part of the wallpaper the output shows. The mirror is done to the finished
    /// frame, so under it the wallpaper is moved the other way to end up where asked.
    pub fn frame(&self) -> wepaper_engine::Frame {
        let side = if self.flip { -1.0 } else { 1.0 };
        wepaper_engine::Frame { zoom: self.zoom, shift: [self.shift[0] * side, self.shift[1]] }
    }

    /// The value called `key`, `None` for a name that is not one of [`KEYS`].
    pub fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "rate" => Value::from(self.rate),
            "volume" => Value::from(self.volume),
            "flip" => Value::from(self.flip),
            "zoom" => Value::from(self.zoom),
            "shift_x" => Value::from(self.shift[0]),
            "shift_y" => Value::from(self.shift[1]),
            "brightness" => Value::from(self.brightness),
            "contrast" => Value::from(self.contrast),
            "saturation" => Value::from(self.saturation),
            "hue" => Value::from(self.hue),
            _ => return None,
        })
    }

    /// Sets the value called `key`; `null` puts it back to plain.
    pub fn set(&mut self, key: &str, value: &Value) -> Result<(), String> {
        let plain = Self::default();
        let number = |least: f32, most: f32, plain: f32| -> Result<f32, String> {
            match value {
                Value::Null => Ok(plain),
                other => other
                    .as_f64()
                    .map(|number| number as f32)
                    .filter(|number| number.is_finite())
                    .map(|number| number.clamp(least, most))
                    .ok_or(format!("{key} takes a number from {least} to {most}")),
            }
        };
        match key {
            "rate" => self.rate = number(0.0, 8.0, plain.rate)?,
            "volume" => self.volume = number(0.0, 1.0, plain.volume)?,
            "flip" => {
                self.flip = match value {
                    Value::Null => false,
                    other => other.as_bool().ok_or("flip takes true or false")?,
                }
            }
            "zoom" => self.zoom = number(0.05, 20.0, plain.zoom)?,
            "shift_x" => self.shift[0] = number(-20.0, 20.0, 0.0)?,
            "shift_y" => self.shift[1] = number(-20.0, 20.0, 0.0)?,
            "brightness" => self.brightness = number(0.0, 4.0, plain.brightness)?,
            "contrast" => self.contrast = number(0.0, 4.0, plain.contrast)?,
            "saturation" => self.saturation = number(0.0, 4.0, plain.saturation)?,
            "hue" => self.hue = number(-360.0, 360.0, plain.hue)?,
            _ => return Err(format!("a look has no \"{key}\" (it has {})", KEYS.join(", "))),
        }
        Ok(())
    }

    /// The look as the tree and the state file hold it.
    pub fn to_value(self) -> Value {
        let mut out = obj! {};
        if let Value::Object(map) = &mut out {
            for key in KEYS {
                map.insert(key.to_owned(), self.get(key).unwrap_or(Value::Null));
            }
        }
        out
    }

    /// The look a state file holds; what it does not hold, or holds wrongly, is plain.
    pub fn from_value(value: &Value) -> Self {
        let mut look = Self::default();
        for key in KEYS {
            let _ = look.set(key, value.get(key));
        }
        look
    }

    /// Hands the picture's part of the look to the pass that applies it.
    pub fn apply(&self, pass: &mut PostPass) {
        for (name, value) in [
            ("u_Flip", f32::from(u8::from(self.flip))),
            ("u_Brightness", self.brightness),
            ("u_Contrast", self.contrast),
            ("u_Saturation", self.saturation),
            ("u_Hue", self.hue.to_radians()),
        ] {
            pass.set(name, value);
        }
    }
}

/// The pass: the frame mirrored, then its colours.
pub const SHADER: &str = "#version 330 core
uniform sampler2D u_Frame;
uniform float u_Flip;
uniform float u_Brightness;
uniform float u_Contrast;
uniform float u_Saturation;
uniform float u_Hue;
in vec2 v_TexCoord;
out vec4 color;
void main() {
    vec2 at = v_TexCoord;
    if (u_Flip > 0.5) at.x = 1.0 - at.x;
    vec3 rgb = texture(u_Frame, at).rgb;
    // The hue turns about the grey axis.
    vec3 axis = vec3(0.57735);
    float c = cos(u_Hue);
    rgb = rgb * c + cross(axis, rgb) * sin(u_Hue) + axis * dot(axis, rgb) * (1.0 - c);
    float grey = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    rgb = mix(vec3(grey), rgb, u_Saturation);
    rgb = (rgb - 0.5) * u_Contrast + 0.5;
    color = vec4(clamp(rgb * u_Brightness, 0.0, 1.0), 1.0);
}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_nobody_touched_needs_no_pass() {
        assert!(Look::default().plain_picture());
        // Speed and loudness are not the picture's.
        let look = Look { rate: 0.5, volume: 0.2, ..Look::default() };
        assert!(look.plain_picture());
        assert!(!Look { flip: true, ..Look::default() }.plain_picture());
        assert!(!Look { hue: 30.0, ..Look::default() }.plain_picture());
        // The zoom and the shifts frame the wallpaper itself: no pass for them either.
        assert!(Look { zoom: 2.0, shift: [0.3, -0.1], ..Look::default() }.plain_picture());
    }

    #[test]
    fn the_frame_is_the_zoom_and_the_shifts_and_follows_the_mirror() {
        let look = Look { zoom: 1.5, shift: [0.25, -0.5], ..Look::default() };
        assert_eq!(look.frame(), wepaper_engine::Frame { zoom: 1.5, shift: [0.25, -0.5] });
        assert_eq!(Look { flip: true, ..look }.frame().shift, [-0.25, -0.5]);
        assert_eq!(Look::default().frame(), wepaper_engine::Frame::default());
        let mut wide = Look::default();
        assert!(wide.set("zoom", &Value::from(0.01)).is_ok() && wide.zoom == 0.05);
        assert!(wide.set("shift_x", &Value::from(50.0)).is_ok() && wide.shift[0] == 20.0);
    }

    #[test]
    fn values_are_set_by_name_kept_in_range_and_put_back_by_null() {
        let mut look = Look::default();
        assert!(look.set("rate", &Value::from(2.0)).is_ok() && look.rate == 2.0);
        assert!(look.set("volume", &Value::from(7.0)).is_ok() && look.volume == 1.0);
        assert!(look.set("flip", &Value::from(true)).is_ok() && look.flip);
        assert!(look.set("shift_x", &Value::from(-0.25)).is_ok() && look.shift == [-0.25, 0.0]);
        assert!(look.set("flip", &Value::from(3.0)).is_err());
        assert!(look.set("zoom", &Value::from("big")).is_err());
        assert!(look.set("tint", &Value::from(1.0)).unwrap_err().contains("saturation"));
        for key in KEYS {
            look.set(key, &Value::Null).unwrap();
        }
        assert_eq!(look, Look::default());
    }

    #[test]
    fn a_look_comes_back_from_what_was_written_of_it() {
        let look =
            Look { rate: 1.5, flip: true, zoom: 2.0, shift: [0.1, -0.2], hue: 90.0, ..Look::default() };
        assert_eq!(Look::from_value(&look.to_value()), look);
        // A state file from before there were looks.
        assert_eq!(Look::from_value(&Value::Null), Look::default());
    }
}
