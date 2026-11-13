//! Finding Wallpaper Engine's files on this machine.

use std::path::{Path, PathBuf};

/// Wallpaper Engine's app id on Steam.
const APP_ID: &str = "431960";

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// The Steam libraries on this machine: the usual install places, and every library
/// they list.
pub fn steam_libraries() -> Vec<PathBuf> {
    let home = home();
    let roots = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
    ];
    let mut libraries: Vec<PathBuf> = Vec::new();
    let mut add = |path: PathBuf| {
        if let Ok(path) = path.canonicalize()
            && path.join("steamapps").is_dir()
            && !libraries.contains(&path)
        {
            libraries.push(path);
        }
    };
    for root in roots {
        // `"path"		"/mnt/games/SteamLibrary"`
        if let Ok(text) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            for line in text.lines() {
                let mut parts = line.split('"').filter(|part| !part.trim().is_empty());
                if parts.next() == Some("path")
                    && let Some(path) = parts.next()
                {
                    add(PathBuf::from(path.replace("\\\\", "/")));
                }
            }
        }
        add(root);
    }
    libraries
}

/// Wallpaper Engine's stock `assets` folder, when it is installed.
pub fn assets() -> Option<PathBuf> {
    steam_libraries().into_iter().map(|library| library.join("steamapps/common/wallpaper_engine/assets")).find(|path| path.is_dir())
}

/// The folder of a wallpaper named by a path or by a workshop id.
pub fn wallpaper(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_dir() {
        return path.canonicalize().ok();
    }
    // A file inside the folder (project.json, scene.pkg) names the folder too.
    if path.is_file() {
        return path.canonicalize().ok()?.parent().map(Path::to_owned);
    }
    if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
        return steam_libraries().into_iter().map(|library| library.join("steamapps/workshop/content").join(APP_ID).join(name)).find(|path| path.is_dir());
    }
    None
}
