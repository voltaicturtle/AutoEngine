//! TCP transport: one line of JSON per message.
//!
//! [`serve`] runs a [`Session`] behind a listening socket, and
//! [`Connection`] is the matching client end. Both use only the standard
//! library: one thread per connection reads lines and forwards them over a
//! channel, so the session itself stays single-threaded.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use crate::protocol::{ClientMsg, ServerMsg, decode, encode};
use crate::session::{ConnId, Session};

/// Longest line the server will read from a client.
const MAX_LINE: usize = 64 * 1024;

enum Incoming {
    Connected(ConnId, TcpStream),
    Message(ConnId, ClientMsg),
    Malformed(ConnId, String),
    Disconnected(ConnId),
}

fn write_line(stream: &mut TcpStream, line: &str) -> std::io::Result<()> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")
}

/// Runs a session on a listener until `stop` is set. Returns the session so
/// the caller can inspect the final state.
pub fn serve(listener: TcpListener, mut session: Session, stop: &AtomicBool) -> std::io::Result<Session> {
    listener.set_nonblocking(true)?;
    let (tx, rx) = mpsc::channel::<Incoming>();
    let started = Instant::now();
    let mut streams: BTreeMap<ConnId, TcpStream> = BTreeMap::new();
    let mut next_conn: ConnId = 1;

    while !stop.load(Ordering::Relaxed) {
        // Accept anyone waiting.
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let conn = next_conn;
                    next_conn += 1;
                    stream.set_nonblocking(false)?;
                    stream.set_nodelay(true).ok();
                    let reader = stream.try_clone()?;
                    let tx = tx.clone();
                    tx.send(Incoming::Connected(conn, stream)).ok();
                    std::thread::spawn(move || {
                        let mut lines = BufReader::new(reader).take(u64::MAX);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            lines.set_limit(MAX_LINE as u64);
                            match lines.read_line(&mut line) {
                                Ok(0) | Err(_) => break,
                                Ok(_) if !line.ends_with('\n') => break, // over-long line
                                Ok(_) => {
                                    let text = line.trim();
                                    if text.is_empty() {
                                        continue;
                                    }
                                    let incoming = match decode::<ClientMsg>(text) {
                                        Ok(message) => Incoming::Message(conn, message),
                                        Err(e) => Incoming::Malformed(conn, e),
                                    };
                                    if tx.send(incoming).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        tx.send(Incoming::Disconnected(conn)).ok();
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }

        let now = started.elapsed().as_millis() as u64;
        let handle = |incoming: Incoming, session: &mut Session, streams: &mut BTreeMap<ConnId, TcpStream>| {
            match incoming {
                Incoming::Connected(conn, stream) => {
                    streams.insert(conn, stream);
                }
                Incoming::Message(conn, message) => session.handle(conn, message, now),
                Incoming::Malformed(conn, error) => {
                    if let Some(stream) = streams.get_mut(&conn) {
                        let reply = ServerMsg::Error { message: format!("could not read that message: {error}") };
                        write_line(stream, &encode(&reply)).ok();
                    }
                }
                Incoming::Disconnected(conn) => {
                    streams.remove(&conn);
                    session.disconnect(conn, now);
                }
            }
        };
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(incoming) => {
                handle(incoming, &mut session, &mut streams);
                while let Ok(more) = rx.try_recv() {
                    handle(more, &mut session, &mut streams);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        session.tick(now);

        let mut dropped = Vec::new();
        for (conn, message) in session.drain_outbox() {
            if let Some(stream) = streams.get_mut(&conn)
                && write_line(stream, &encode(&message)).is_err()
            {
                dropped.push(conn);
            }
        }
        for conn in dropped {
            if let Some(stream) = streams.remove(&conn) {
                stream.shutdown(std::net::Shutdown::Both).ok();
            }
        }
    }
    for stream in streams.values() {
        stream.shutdown(std::net::Shutdown::Both).ok();
    }
    Ok(session)
}

/// The client end of a connection to a server.
pub struct Connection {
    stream: TcpStream,
    incoming: Receiver<Result<ServerMsg, String>>,
    closed: bool,
}

impl Connection {
    pub fn connect(addr: impl ToSocketAddrs) -> std::io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true).ok();
        let reader = stream.try_clone()?;
        let (tx, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                if tx.send(decode::<ServerMsg>(&line)).is_err() {
                    break;
                }
            }
        });
        Ok(Connection { stream, incoming, closed: false })
    }

    pub fn send(&mut self, message: &ClientMsg) -> std::io::Result<()> {
        write_line(&mut self.stream, &encode(message))
    }

    /// Messages that have arrived since the last call. Never blocks.
    pub fn poll(&mut self) -> Vec<ServerMsg> {
        let mut out = Vec::new();
        loop {
            match self.incoming.try_recv() {
                Ok(Ok(message)) => out.push(message),
                Ok(Err(_)) => {} // a line this client cannot read; skip it
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    break;
                }
            }
        }
        out
    }

    /// Waits up to `timeout` for the next message.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<ServerMsg> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            match self.incoming.recv_timeout(left) {
                Ok(Ok(message)) => return Some(message),
                Ok(Err(_)) => {}
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => {
                    self.closed = true;
                    return None;
                }
            }
        }
    }

    /// True once the server has closed the connection.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stream.shutdown(std::net::Shutdown::Both).ok();
    }
}
