//! The commands that talk to the daemon.

use std::path::PathBuf;

use wepaper_ipc::{Client, Request};
use wepaper_json::Value;

use super::{Failure, parse_property, value};

pub const USAGE: &str = "\
Talking to the daemon:
  show <wallpaper> [--on OUTPUT] [-p K=V]...   show a wallpaper (a folder or a workshop id)
  hide [--on OUTPUT]                           take the wallpaper off
  pause | resume [--on OUTPUT]                 freeze on the current frame, play again
  status                                       what is shown where
  get [PATH]                                   a value of the control tree, or a whole branch
  set PATH VALUE                               change one (config/fps, outputs/DP-1/wallpaper/properties/...)
  list [PATH]                                  the names under a path
  reset PATH                                   back to the default
  snapshot FILE [--on OUTPUT]                  what an output shows now, as a PNG
  watch                                        events as they happen, one JSON object a line
  reload                                       read the config file again
  quit                                         stop the daemon
  version                                      the daemon's version and what it runs on

  --socket PATH    the daemon's socket (default $XDG_RUNTIME_DIR/wepaper.sock)
  --json           print answers as JSON
";

/// A value as a person reads it: text bare, everything else as JSON.
fn show(value: &Value, json: bool) -> String {
    match value {
        Value::String(text) if !json => text.clone(),
        Value::Array(items) if !json && items.iter().all(|item| item.as_str().is_some()) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        Value::Object(_) | Value::Array(_) => value.to_json_pretty(),
        other => other.to_json(),
    }
}

/// The `status` command's summary of the whole tree.
fn summary(tree: &Value) -> String {
    let megabytes = |bytes: &Value| bytes.as_f64().unwrap_or(0.0) / 1048576.0;
    let mut lines = Vec::new();
    for (name, output) in tree.get("outputs").members() {
        let wallpaper = output.get("wallpaper");
        let what = match wallpaper.get("title").as_str() {
            Some(title) => format!("{} ({}, {})", if title.is_empty() { "untitled" } else { title }, wallpaper.get("kind").as_str().unwrap_or("?"), wallpaper.get("path").as_str().unwrap_or("")),
            None => "nothing".to_owned(),
        };
        let size = output.get("size");
        let paused = if output.get("paused").as_bool() == Some(true) { ", paused" } else { "" };
        lines.push(format!("{name} {}x{} @{}: {what}, {} fps{paused}", show(size.at(0), false), show(size.at(1), false), show(output.get("scale"), false), show(output.get("fps"), false)));
        if let Some(error) = output.get("error").as_str() {
            lines.push(format!("  last error: {error}"));
        }
    }
    if lines.is_empty() {
        lines.push("no outputs".to_owned());
    }
    let memory = tree.get("memory");
    lines.push(format!(
        "video memory: textures {:.0} MB, buffers {:.0} MB, video {:.0} MB",
        megabytes(memory.get("textures")),
        megabytes(memory.get("buffers")),
        megabytes(memory.get("video"))
    ));
    lines.join("\n")
}

pub fn run(command: &str, mut args: lexopt::Parser) -> Result<(), Failure> {
    use lexopt::prelude::*;
    let (mut socket, mut json, mut output) = (None::<PathBuf>, false, None::<String>);
    let mut properties = Vec::new();
    let mut words: Vec<String> = Vec::new();
    while let Some(arg) = args.next().map_err(|e| e.to_string())? {
        match arg {
            Long("socket") => socket = Some(value(&mut args)?.into()),
            Long("json") => json = true,
            Long("on") | Long("output") => output = Some(value(&mut args)?),
            Short('p') | Long("property") => properties.push(parse_property(&value(&mut args)?)?),
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            Value(word) => words.push(word.to_string_lossy().into_owned()),
            other => return Err(other.unexpected().to_string()),
        }
    }
    let word = |index: usize, what: &str| words.get(index).cloned().ok_or_else(|| format!("{command}: {what}?"));
    let request = match command {
        "show" => Request::Show { wallpaper: word(0, "which wallpaper")?, output, properties },
        "hide" => Request::Hide { output },
        "pause" => Request::Pause { output },
        "resume" => Request::Resume { output },
        "status" => Request::Get { path: String::new() },
        "get" => Request::Get { path: words.first().cloned().unwrap_or_default() },
        "list" => Request::List { path: words.first().cloned().unwrap_or_default() },
        "set" => {
            let text = words.get(1..).filter(|rest| !rest.is_empty()).map(|rest| rest.join(" ")).ok_or("set: to what value?")?;
            Request::Set { path: word(0, "which path")?, value: wepaper_json::parse(&text).unwrap_or(wepaper_json::Value::from(text)) }
        }
        "reset" => Request::Reset { path: word(0, "which path")? },
        "snapshot" => {
            let file = std::path::absolute(word(0, "into which file")?).map_err(|e| e.to_string())?;
            Request::Snapshot { output, file: file.to_string_lossy().into_owned() }
        }
        "watch" => Request::Watch,
        "reload" => Request::Reload,
        "quit" => Request::Quit,
        "version" => Request::Version,
        other => return Err(format!("unknown command '{other}' (see wepaper --help)")),
    };
    let mut client = Client::connect(&socket.unwrap_or_else(wepaper_ipc::default_socket)).map_err(|e| e.to_string())?;
    if request == Request::Watch {
        for event in client.watch().map_err(|e| e.to_string())? {
            println!("{}", event.to_json());
        }
        return Ok(());
    }
    let value = client.call(&request).map_err(|e| e.to_string())?;
    if command == "status" && !json {
        println!("{}", summary(&value));
    } else if !value.is_null() {
        println!("{}", show(&value, json));
    }
    Ok(())
}
