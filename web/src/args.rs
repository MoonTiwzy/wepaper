//! The command line the daemon starts this plugin with.

use std::ffi::OsString;
use std::path::PathBuf;

use wepaper_json::Value;

use crate::properties;
use crate::surface::Layer;

/// How to call the plugin, for `--help` and for a command line that does not parse.
pub const USAGE: &str = "\
usage: wepaper-web --dir <wallpaper folder> --output <output name> [options]

  --dir <folder>           the web wallpaper's folder
  --file <name>            the page inside it (default: index.html)
  --output <name>          the output to show it on, by connector name (DP-1)
  --layer <layer>          background (default) or bottom
  --namespace <name>       the layer surface's namespace (default: wepaper)
  --fps <n>                cap the frame rate (default: no cap)
  --volume <0..100>        loudness (default: 100)
  --muted                  start silent
  --property <name=json>   a user property's value; may be repeated
  --passthrough            the page gets no pointer input

Commands are read from stdin and events written to stdout, one JSON object per line.";

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    /// The wallpaper's folder.
    pub dir: PathBuf,
    /// The page to open, relative to the folder.
    pub file: String,
    /// The connector name of the output to cover.
    pub output: String,
    /// The layer the surface sits on.
    pub layer: Layer,
    /// The layer surface's namespace.
    pub namespace: String,
    /// The frame rate cap, if there is one.
    pub fps: Option<u32>,
    /// Loudness, 0 to 100.
    pub volume: u8,
    /// Whether the page starts silent.
    pub muted: bool,
    /// User property values that override the wallpaper's own, in the order given.
    pub properties: Vec<(String, Value)>,
    /// Whether pointer input goes past the page.
    pub passthrough: bool,
}

/// A command line that parsed.
#[derive(Debug, Clone, PartialEq)]
pub enum Invocation {
    /// Show a wallpaper.
    Run(Args),
    /// Print [`USAGE`] and leave.
    Help,
}

/// Parses the arguments after the program's name.
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    use lexopt::prelude::*;

    let mut dir = None;
    let mut file = "index.html".to_owned();
    let mut output = None;
    let mut layer = Layer::Background;
    let mut namespace = "wepaper".to_owned();
    let mut fps = None;
    let mut volume = 100;
    let mut muted = false;
    let mut overrides = Vec::new();
    let mut passthrough = false;

    let mut parser = lexopt::Parser::from_args(arguments);
    while let Some(argument) = parser.next().map_err(|error| error.to_string())? {
        match argument {
            Long("dir") => dir = Some(PathBuf::from(parser.value().map_err(|error| error.to_string())?)),
            Long("file") => file = text(&mut parser, "--file")?,
            Long("output") => output = Some(text(&mut parser, "--output")?),
            Long("layer") => {
                let name = text(&mut parser, "--layer")?;
                layer = Layer::from_name(&name)
                    .ok_or_else(|| format!("--layer is background or bottom, not \"{name}\""))?;
            }
            Long("namespace") => namespace = text(&mut parser, "--namespace")?,
            Long("fps") => {
                let value = text(&mut parser, "--fps")?;
                // 0 means the same on the command line as in the fps command: no cap.
                fps = match value.parse::<u32>() {
                    Ok(0) => None,
                    Ok(cap) => Some(cap),
                    Err(_) => return Err(format!("--fps wants a whole number, not \"{value}\"")),
                };
            }
            Long("volume") => {
                let value = text(&mut parser, "--volume")?;
                volume = match value.parse::<u8>() {
                    Ok(percent) if percent <= 100 => percent,
                    _ => return Err(format!("--volume wants 0 to 100, not \"{value}\"")),
                };
            }
            Long("muted") => muted = true,
            Long("property") => {
                overrides.push(properties::parse_override(&text(&mut parser, "--property")?)?)
            }
            Long("passthrough") => passthrough = true,
            Long("help") | Short('h') => return Ok(Invocation::Help),
            other => return Err(other.unexpected().to_string()),
        }
    }

    let dir = dir.ok_or("--dir is required")?;
    let output = output.ok_or("--output is required")?;
    if file.is_empty() || output.is_empty() {
        return Err("--file and --output cannot be empty".to_owned());
    }
    Ok(Invocation::Run(Args {
        dir,
        file,
        output,
        layer,
        namespace,
        fps,
        volume,
        muted,
        properties: overrides,
        passthrough,
    }))
}

/// The value of `option`, which has to be text.
fn text(parser: &mut lexopt::Parser, option: &str) -> Result<String, String> {
    let value = parser.value().map_err(|error| error.to_string())?;
    value.into_string().map_err(|_| format!("{option}: the value is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(arguments: &[&str]) -> Result<Invocation, String> {
        parse(arguments.iter().map(OsString::from))
    }

    fn run(arguments: &[&str]) -> Args {
        match parse_line(arguments) {
            Ok(Invocation::Run(args)) => args,
            other => panic!("expected a run, got {other:?}"),
        }
    }

    #[test]
    fn the_two_required_options_are_enough_and_the_rest_default() {
        let args = run(&["--dir", "/walls/1", "--output", "DP-1"]);
        assert_eq!(
            args,
            Args {
                dir: PathBuf::from("/walls/1"),
                file: "index.html".to_owned(),
                output: "DP-1".to_owned(),
                layer: Layer::Background,
                namespace: "wepaper".to_owned(),
                fps: None,
                volume: 100,
                muted: false,
                properties: Vec::new(),
                passthrough: false,
            }
        );
    }

    #[test]
    fn every_option_is_read() {
        let args = run(&[
            "--dir",
            "/walls/2",
            "--file",
            "HackerConsole.html",
            "--output",
            "HDMI-A-1",
            "--layer",
            "bottom",
            "--namespace",
            "wallpaper",
            "--fps",
            "30",
            "--volume",
            "40",
            "--muted",
            "--passthrough",
        ]);
        assert_eq!(args.file, "HackerConsole.html");
        assert_eq!(args.output, "HDMI-A-1");
        assert_eq!(args.layer, Layer::Bottom);
        assert_eq!(args.namespace, "wallpaper");
        assert_eq!(args.fps, Some(30));
        assert_eq!(args.volume, 40);
        assert!(args.muted);
        assert!(args.passthrough);
    }

    #[test]
    fn options_may_be_written_with_an_equals_sign() {
        let args = run(&["--dir=/walls/3", "--output=DP-2", "--fps=24"]);
        assert_eq!(args.dir, PathBuf::from("/walls/3"));
        assert_eq!(args.output, "DP-2");
        assert_eq!(args.fps, Some(24));
    }

    #[test]
    fn properties_repeat_and_keep_their_order() {
        let args = run(&[
            "--dir",
            "/w",
            "--output",
            "DP-1",
            "--property",
            "show_clock=false",
            "--property",
            r#"dayCycle="3""#,
            "--property",
            "speed=1.5",
            "--property",
            "show_clock=true",
        ]);
        assert_eq!(
            args.properties,
            [
                ("show_clock".to_owned(), Value::Bool(false)),
                ("dayCycle".to_owned(), Value::from("3")),
                ("speed".to_owned(), Value::Number(1.5)),
                ("show_clock".to_owned(), Value::Bool(true)),
            ]
        );
    }

    #[test]
    fn a_frame_rate_of_zero_is_no_cap() {
        assert_eq!(run(&["--dir", "/w", "--output", "DP-1", "--fps", "0"]).fps, None);
    }

    #[test]
    fn missing_required_options_are_named() {
        assert_eq!(parse_line(&["--output", "DP-1"]), Err("--dir is required".to_owned()));
        assert_eq!(parse_line(&["--dir", "/w"]), Err("--output is required".to_owned()));
    }

    #[test]
    fn bad_values_are_refused() {
        for arguments in [
            &["--dir", "/w", "--output", "DP-1", "--layer", "overlay"][..],
            &["--dir", "/w", "--output", "DP-1", "--fps", "fast"],
            &["--dir", "/w", "--output", "DP-1", "--fps", "-1"],
            &["--dir", "/w", "--output", "DP-1", "--volume", "101"],
            &["--dir", "/w", "--output", "DP-1", "--volume", "loud"],
            &["--dir", "/w", "--output", "DP-1", "--property", "novalue"],
            &["--dir", "/w", "--output", "DP-1", "--output"],
            &["--dir", "/w", "--output", ""],
            &["--dir", "/w", "--output", "DP-1", "--frobnicate"],
            &["--dir", "/w", "--output", "DP-1", "stray"],
        ] {
            assert!(parse_line(arguments).is_err(), "{arguments:?} parsed");
        }
    }

    #[test]
    fn help_wins_over_everything_else() {
        assert_eq!(parse_line(&["--help"]), Ok(Invocation::Help));
        assert_eq!(parse_line(&["--dir", "/w", "-h"]), Ok(Invocation::Help));
    }
}
