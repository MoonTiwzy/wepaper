//! The control socket: clients, their lines, their answers.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use wepaper_ipc::Request;
use wepaper_json::Value;

/// A client that leaves this much unread is dropped: a watcher that stopped reading
/// must not make the daemon hold its events forever.
const MAX_UNSENT: usize = 1 << 20;
/// A line longer than this is not a request.
const MAX_LINE: usize = 1 << 20;

struct Client {
    stream: UnixStream,
    received: Vec<u8>,
    unsent: Vec<u8>,
    watching: bool,
    gone: bool,
}

/// A request from a client, to be answered with [`Server::answer`].
pub struct Incoming {
    /// Which client asked.
    pub client: usize,
    /// What it asked, or why its line is not a request.
    pub request: Result<Request, String>,
}

/// The listening socket and everyone connected to it.
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    clients: Vec<Option<Client>>,
}

impl Server {
    /// Listens on `path`. A socket file nobody answers on is replaced; one somebody
    /// answers on means a daemon is running already.
    pub fn bind(path: &Path) -> Result<Self, String> {
        if UnixStream::connect(path).is_ok() {
            return Err(format!("a daemon is already listening on {}", path.display()));
        }
        let _ = std::fs::remove_file(path);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Self { listener, path: path.to_owned(), clients: Vec::new() })
    }

    /// The descriptors to wait on: the listener and every client.
    pub fn fds(&self) -> Vec<RawFd> {
        std::iter::once(self.listener.as_raw_fd())
            .chain(self.clients.iter().flatten().map(|c| c.stream.as_raw_fd()))
            .collect()
    }

    /// Accepts new clients, reads what the connected ones sent, and returns the
    /// complete requests among it.
    pub fn receive(&mut self) -> Vec<Incoming> {
        while let Ok((stream, _)) = self.listener.accept() {
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let client =
                Client { stream, received: Vec::new(), unsent: Vec::new(), watching: false, gone: false };
            match self.clients.iter_mut().position(|slot| slot.is_none()) {
                Some(free) => self.clients[free] = Some(client),
                None => self.clients.push(Some(client)),
            }
        }
        let mut incoming = Vec::new();
        for (index, slot) in self.clients.iter_mut().enumerate() {
            let Some(client) = slot else { continue };
            let mut buffer = [0u8; 4096];
            loop {
                match client.stream.read(&mut buffer) {
                    Ok(0) => {
                        client.gone = true;
                        break;
                    }
                    Ok(count) => client.received.extend_from_slice(&buffer[..count]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => {
                        client.gone = true;
                        break;
                    }
                }
            }
            while let Some(end) = client.received.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = client.received.drain(..=end).collect();
                let text = String::from_utf8_lossy(&line);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                // Once a connection watches, it only listens.
                if client.watching {
                    continue;
                }
                let request = wepaper_json::parse(text)
                    .map_err(|e| format!("not JSON: {e}"))
                    .and_then(|message| Request::parse(&message));
                incoming.push(Incoming { client: index, request });
            }
            if client.received.len() > MAX_LINE {
                client.gone = true;
            }
        }
        self.sweep();
        incoming
    }

    /// Sends `message` to one client as a line.
    pub fn answer(&mut self, client: usize, message: &Value) {
        if let Some(Some(client)) = self.clients.get_mut(client) {
            client.unsent.extend_from_slice(message.to_json().as_bytes());
            client.unsent.push(b'\n');
        }
    }

    /// From now on `client` gets events and its requests are ignored.
    pub fn watch(&mut self, client: usize) {
        if let Some(Some(client)) = self.clients.get_mut(client) {
            client.watching = true;
        }
    }

    /// Whether anybody watches: events nobody hears need not be built.
    pub fn has_watchers(&self) -> bool {
        self.clients.iter().flatten().any(|c| c.watching)
    }

    /// Sends an event to every watching client.
    pub fn broadcast(&mut self, event: &Value) {
        let mut line = event.to_json().into_bytes();
        line.push(b'\n');
        for client in self.clients.iter_mut().flatten().filter(|c| c.watching) {
            client.unsent.extend_from_slice(&line);
        }
    }

    /// Writes as much of the pending answers as the sockets take. Never blocks.
    pub fn flush(&mut self) {
        for client in self.clients.iter_mut().flatten() {
            while !client.unsent.is_empty() {
                match client.stream.write(&client.unsent) {
                    Ok(0) => {
                        client.gone = true;
                        break;
                    }
                    Ok(count) => {
                        client.unsent.drain(..count);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => {
                        client.gone = true;
                        break;
                    }
                }
            }
            if client.unsent.len() > MAX_UNSENT {
                log::warn!("a client stopped reading and is disconnected");
                client.gone = true;
            }
        }
        self.sweep();
    }

    fn sweep(&mut self) {
        for slot in &mut self.clients {
            // A client that left is kept until what it is owed is written, or cannot be.
            if slot.as_ref().is_some_and(|c| c.gone) {
                *slot = None;
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn server(name: &str) -> (Server, PathBuf) {
        let path = std::env::temp_dir().join(format!("wepaper-test-{}-{name}.sock", std::process::id()));
        (Server::bind(&path).unwrap(), path)
    }

    /// Receives until a request shows up (the kernel hands data over asynchronously).
    fn receive(server: &mut Server) -> Vec<Incoming> {
        for _ in 0..200 {
            let incoming = server.receive();
            if !incoming.is_empty() {
                return incoming;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Vec::new()
    }

    #[test]
    fn requests_are_parsed_and_answered_in_order() {
        let (mut server, path) = server("order");
        let mut client = UnixStream::connect(&path).unwrap();
        client
            .write_all(b"{\"cmd\":\"version\"}\n\n{\"cmd\":\"get\",\"path\":\"config/fps\"}\n{\"cmd\":\"se")
            .unwrap();
        let incoming = receive(&mut server);
        assert_eq!(incoming.len(), 2, "two complete lines; the third is still coming");
        assert_eq!(incoming[0].request, Ok(Request::Version));
        assert_eq!(incoming[1].request, Ok(Request::Get { path: "config/fps".into() }));
        server.answer(incoming[0].client, &wepaper_ipc::ok(Value::from("a")));
        server.answer(incoming[1].client, &wepaper_ipc::ok(Value::from(60)));
        server.flush();
        let mut lines = BufReader::new(client.try_clone().unwrap()).lines();
        assert_eq!(lines.next().unwrap().unwrap(), r#"{"ok":true,"value":"a"}"#);
        assert_eq!(lines.next().unwrap().unwrap(), r#"{"ok":true,"value":60}"#);
        client.write_all(b"t\",\"path\":\"config/fps\",\"value\":30}\n").unwrap();
        assert_eq!(
            receive(&mut server)[0].request,
            Ok(Request::Set { path: "config/fps".into(), value: Value::from(30) })
        );
    }

    #[test]
    fn garbage_line_is_answered_with_error() {
        let (mut server, path) = server("garbage");
        let mut client = UnixStream::connect(&path).unwrap();
        client.write_all(b"this is not json\n{\"cmd\":\"fly\"}\n\xff\xfe\n").unwrap();
        let incoming = receive(&mut server);
        assert_eq!(incoming.len(), 3);
        assert!(incoming[0].request.as_ref().unwrap_err().starts_with("not JSON"));
        assert_eq!(incoming[1].request, Err("unknown command \"fly\"".to_owned()));
        assert!(incoming[2].request.is_err(), "bytes that are not text are an error, not a crash");
    }

    #[test]
    fn slow_subscriber_is_dropped() {
        let (mut server, path) = server("slow");
        let mut client = UnixStream::connect(&path).unwrap();
        client.write_all(b"{\"cmd\":\"watch\"}\n").unwrap();
        let incoming = receive(&mut server);
        server.watch(incoming[0].client);
        assert!(server.has_watchers());
        // The client never reads. Events pile up past the limit; it is cut off, the server lives.
        let event = wepaper_json::obj! { "event" => "changed", "padding" => "x".repeat(8192) };
        for _ in 0..400 {
            server.broadcast(&event);
            server.flush();
        }
        assert!(!server.has_watchers(), "the client that stopped reading is gone");
        // And a new client is served as if nothing happened.
        let mut other = UnixStream::connect(&path).unwrap();
        other.write_all(b"{\"cmd\":\"version\"}\n").unwrap();
        assert_eq!(receive(&mut server)[0].request, Ok(Request::Version));
    }

    #[test]
    fn a_second_daemon_is_refused_and_a_dead_socket_is_replaced() {
        let (server, path) = server("second");
        assert!(Server::bind(&path).err().is_some_and(|why| why.contains("already listening")));
        std::mem::forget(server); // The socket file stays, nobody answers: a crashed daemon.
        let _ = std::fs::remove_file(&path);
        std::os::unix::net::UnixListener::bind(&path).map(drop).unwrap();
        assert!(Server::bind(&path).is_ok(), "a socket nobody answers on is taken over");
    }
}
