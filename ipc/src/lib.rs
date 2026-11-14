//! The control protocol of the wepaper daemon and a client for it.
//!
//! The daemon keeps everything it knows in one tree (`config/fps`,
//! `outputs/DP-1/wallpaper/properties/speed`, ...) and the protocol is a handful of
//! verbs over that tree instead of a command per setting: [`Request::Get`],
//! [`Request::Set`], [`Request::List`], [`Request::Reset`], [`Request::Watch`], and
//! the few actions that are not a value: showing and hiding a wallpaper, pausing,
//! taking a snapshot.
//!
//! On the wire it is one JSON object per line over a Unix socket: a request tagged by
//! `cmd`, answered by `{"ok": true, "value": ...}` or `{"ok": false, "error": "..."}`,
//! in order. After `watch` the connection carries events instead.
//!
//! ```no_run
//! use wepaper_ipc::{Client, Request};
//!
//! let mut daemon = Client::connect(&wepaper_ipc::default_socket())?;
//! daemon.call(&Request::Show { wallpaper: "3566329932".into(), output: None, properties: None })?;
//! daemon.call(&Request::Set { path: "config/fps".into(), value: 30.into() })?;
//! let fps = daemon.call(&Request::Get { path: "config/fps".into() })?;
//! # Ok::<(), wepaper_ipc::Error>(())
//! ```

mod client;
mod request;

pub use client::{Client, Error, default_socket};
pub use request::{PROTOCOL, Request, error, ok, split_path};
