//! wepaper: Wallpaper Engine wallpapers as the Wayland desktop background.
//!
//! One binary: `wepaper daemon` shows the wallpapers, the other commands talk to it
//! or work on wallpapers offline.

mod cli;
mod daemon;
mod logger;

fn usage() -> String {
    format!(
        "\
wepaper: Wallpaper Engine wallpapers as the Wayland desktop background

Usage: wepaper [-v] <command> [options]

The daemon:
  daemon [wallpaper]                           show wallpapers and listen for commands

{}
Offline tools:
  render <wallpaper> -o FILE                   one frame of a wallpaper to a PNG, without a display

  wepaper <command> --help                     what a command takes
  wepaper --version
",
        cli::client::USAGE
    )
}

fn main() {
    let mut args = lexopt::Parser::from_env();
    let mut verbosity = log::LevelFilter::Warn;
    let command = loop {
        match args.next() {
            Ok(Some(lexopt::Arg::Short('v') | lexopt::Arg::Long("verbose"))) => {
                verbosity = if verbosity == log::LevelFilter::Warn { log::LevelFilter::Info } else { log::LevelFilter::Debug };
            }
            Ok(Some(lexopt::Arg::Short('V') | lexopt::Arg::Long("version"))) => {
                println!("wepaper {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            Ok(Some(lexopt::Arg::Value(command))) => break command.to_string_lossy().into_owned(),
            Ok(Some(lexopt::Arg::Short('h') | lexopt::Arg::Long("help"))) | Ok(None) => {
                print!("{}", usage());
                return;
            }
            Ok(Some(other)) => fail(&other.unexpected().to_string()),
            Err(error) => fail(&error.to_string()),
        }
    };
    logger::init(verbosity);
    let result = match command.as_str() {
        "daemon" => daemon::run(args),
        "render" => cli::render::run(args),
        other => cli::client::run(other, args),
    };
    if let Err(message) = result {
        fail(&message);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("wepaper: {message}");
    std::process::exit(1);
}
