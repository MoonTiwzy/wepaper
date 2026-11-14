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
{}
  wepaper <command> --help                     what a command takes
  wepaper --version
",
        cli::client::USAGE,
        cli::tools::USAGE
    )
}

fn main() {
    let mut args = lexopt::Parser::from_env();
    let mut verbosity = log::LevelFilter::Warn;
    let command = loop {
        match args.next() {
            Ok(Some(lexopt::Arg::Short('v') | lexopt::Arg::Long("verbose"))) => {
                verbosity = if verbosity == log::LevelFilter::Warn {
                    log::LevelFilter::Info
                } else {
                    log::LevelFilter::Debug
                };
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
    logger::init(verbosity, command == "daemon");
    if command != "daemon" {
        // Output piped into `head` ends with the pipe, quietly, as a command line
        // tool's should. Not the daemon: a plugin that died must cost it a failed
        // write, not its life.
        // SAFETY: restores a signal's default action; nothing else runs yet.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }
    let result = match command.as_str() {
        "daemon" => daemon::run(args),
        "render" => cli::render::run(args),
        "inspect" => cli::tools::inspect(args),
        "check" => cli::tools::check(args),
        "unpack" => cli::tools::unpack(args),
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
