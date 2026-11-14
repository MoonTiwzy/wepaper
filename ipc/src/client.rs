//! A blocking client.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use wepaper_json::Value;

use crate::Request;

/// Why a call did not return a value.
#[derive(Debug)]
pub enum Error {
    /// The daemon is not there, or the connection broke.
    Connection(std::io::Error),
    /// The daemon answered with something that is not a response.
    Protocol(String),
    /// The daemon refused the request; this is its message.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Connection(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.kind() == std::io::ErrorKind::ConnectionRefused =>
            {
                f.write_str("the daemon is not running (start it: wepaper daemon)")
            }
            Error::Connection(error) => write!(f, "connection to the daemon: {error}"),
            Error::Protocol(what) => write!(f, "unexpected answer from the daemon: {what}"),
            Error::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Connection(error)
    }
}

/// Where the daemon listens unless told otherwise: `$XDG_RUNTIME_DIR/wepaper.sock`.
pub fn default_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
        .join("wepaper.sock")
}

/// A connection to the daemon.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    /// Connects to the daemon listening at `socket`.
    pub fn connect(socket: &Path) -> Result<Self, Error> {
        let stream = UnixStream::connect(socket)?;
        Ok(Self { reader: BufReader::new(stream.try_clone()?), writer: stream })
    }

    /// How long a call waits for the daemon; `None`, as a new client has it, waits for
    /// as long as the daemon takes. A call that ran out of time fails with
    /// [`Error::Connection`] of the kind `WouldBlock` or `TimedOut`.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.writer.set_read_timeout(timeout)?;
        self.writer.set_write_timeout(timeout)
    }

    /// Waits for the daemon to close the connection, as it does when it goes, for as
    /// long as the timeout allows. Whether it did.
    pub fn closed(&mut self) -> bool {
        let mut rest = String::new();
        loop {
            match self.reader.read_line(&mut rest) {
                Ok(0) => return true,
                Ok(_) => rest.clear(),
                Err(_) => return false,
            }
        }
    }

    fn read(&mut self) -> Result<Value, Error> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(Error::Connection(std::io::ErrorKind::UnexpectedEof.into()));
        }
        wepaper_json::parse(&line).map_err(|e| Error::Protocol(e.to_string()))
    }

    /// Sends a request and waits for its answer: the value, or the daemon's refusal.
    pub fn call(&mut self, request: &Request) -> Result<Value, Error> {
        let mut line = request.to_value().to_json();
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        let response = self.read()?;
        match response.get("ok").as_bool() {
            Some(true) => Ok(response.get("value").clone()),
            Some(false) => {
                Err(Error::Refused(response.get("error").as_str().unwrap_or("the daemon refused").to_owned()))
            }
            None => Err(Error::Protocol(line)),
        }
    }

    /// Asks for events and returns them as they come, until the daemon goes away.
    pub fn watch(mut self) -> Result<impl Iterator<Item = Value>, Error> {
        self.call(&Request::Watch)?;
        Ok(std::iter::from_fn(move || self.read().ok()))
    }
}

impl AsRawFd for Client {
    fn as_raw_fd(&self) -> RawFd {
        self.writer.as_raw_fd()
    }
}
