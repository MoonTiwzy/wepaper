//! wepaper: Wallpaper Engine wallpapers as the Wayland desktop background.
//!
//! One binary: `wepaper daemon` shows the wallpapers, the other commands talk to it
//! or work on wallpapers offline.

mod cli;
mod logger;

const USAGE: &str = "\
wepaper: Wallpaper Engine wallpapers as the Wayland desktop background

Usage: wepaper <command> [options]

Offline tools:
  render     one frame of a wallpaper to a PNG, without a display

  wepaper <command> --help   what a command takes
  wepaper --version
";

fn main() {
    let mut args = lexopt::Parser::from_env();
    let mut verbosity = log::LevelFilter::Warn;
    let command = loop {
        match args.next() {
            Ok(Some(lexopt::Arg::Short('v') | lexopt::Arg::Long("verbose"))) => verbosity = log::LevelFilter::Debug,
            Ok(Some(lexopt::Arg::Short('V') | lexopt::Arg::Long("version"))) => {
                println!("wepaper {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            Ok(Some(lexopt::Arg::Value(command))) => break command.to_string_lossy().into_owned(),
            Ok(Some(lexopt::Arg::Short('h') | lexopt::Arg::Long("help"))) | Ok(None) => {
                print!("{USAGE}");
                return;
            }
            Ok(Some(other)) => fail(&other.unexpected().to_string()),
            Err(error) => fail(&error.to_string()),
        }
    };
    logger::init(verbosity);
    let result = match command.as_str() {
        "render" => cli::render::run(args),
        other => Err(format!("unknown command '{other}' (see wepaper --help)")),
    };
    if let Err(message) = result {
        fail(&message);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("wepaper: {message}");
    std::process::exit(1);
}
