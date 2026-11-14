//! The server end: a [`Session`] behind a listening socket.

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::read_line_limited;
use crate::protocol::{ClientMsg, ServerMsg, decode, encode};
use crate::session::{ConnId, Session};

/// Limits that keep one bad connection from hurting the game. The defaults
/// suit a game among friends; every field can be changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    /// Connections allowed at once, players and spectators together. More
    /// are refused with a message.
    pub max_connections: usize,
    /// A connection that has sent nothing for this long is dropped.
    pub idle_timeout: Duration,
    /// A connection that has been quiet for this long is sent a ping, which
    /// clients answer by themselves.
    pub ping_interval: Duration,
    /// The longest line accepted from a client, in bytes.
    pub max_line_bytes: usize,
    /// Messages that may wait to be written to one client. A client that
    /// falls further behind is dropped.
    pub send_queue: usize,
    /// How long one write to a client may block before it is dropped.
    pub write_timeout: Duration,
    /// Messages a client may send per second, on average.
    pub messages_per_second: u32,
    /// Messages a client may send at once before the average applies.
    pub burst: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            max_connections: 64,
            idle_timeout: Duration::from_secs(30),
            ping_interval: Duration::from_secs(5),
            max_line_bytes: 64 * 1024,
            send_queue: 512,
            write_timeout: Duration::from_secs(10),
            messages_per_second: 40,
            burst: 120,
        }
    }
}

/// Counters for what a server has done so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetStats {
    /// Connections open right now.
    pub connections: usize,
    pub accepted: u64,
    /// Turned away because the server was full.
    pub refused: u64,
    /// Dropped for not keeping up with what they were sent.
    pub dropped_slow: u64,
    /// Dropped for going silent.
    pub dropped_idle: u64,
    /// Dropped for sending too much or a line that was too long.
    pub dropped_flood: u64,
    pub messages_in: u64,
    pub messages_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// A session waiting to be hosted on a socket.
///
/// ```no_run
/// use autoengine::prelude::*;
/// use autoengine::net::Server;
///
/// let content = autoengine::sample::content();
/// let session = Session::new(content, SessionConfig::default());
/// let server = Server::bind("127.0.0.1:7777").unwrap().spawn(session);
/// println!("listening on {}", server.addr());
/// // ... later
/// let session = server.stop().unwrap();
/// ```
pub struct Server {
    listener: TcpListener,
    config: ServerConfig,
    stats: Arc<Mutex<NetStats>>,
}

impl Server {
    /// Listens on an address such as `"127.0.0.1:7777"`. Port 0 picks a
    /// free port; see [`Server::addr`].
    pub fn bind(addr: impl ToSocketAddrs) -> std::io::Result<Server> {
        Ok(Server::from_listener(TcpListener::bind(addr)?))
    }

    pub fn from_listener(listener: TcpListener) -> Server {
        Server { listener, config: ServerConfig::default(), stats: Arc::default() }
    }

    pub fn with_config(mut self, config: ServerConfig) -> Server {
        self.config = config;
        self
    }

    /// The address being listened on.
    pub fn addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("a bound listener has an address")
    }

    /// Hosts the session on the calling thread until `stop` is set, then
    /// returns it so the final state can be inspected.
    pub fn run(self, session: Session, stop: &AtomicBool) -> std::io::Result<Session> {
        run(self.listener, session, stop, self.config, self.stats)
    }

    /// Hosts the session on a background thread.
    pub fn spawn(self, session: Session) -> ServerHandle {
        let addr = self.addr();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = self.stats.clone();
        let thread = {
            let stop = stop.clone();
            std::thread::spawn(move || self.run(session, &stop))
        };
        ServerHandle { addr, stop, stats, thread }
    }
}

/// A server running in the background (see [`Server::spawn`]).
pub struct ServerHandle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<NetStats>>,
    thread: std::thread::JoinHandle<std::io::Result<Session>>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stats(&self) -> NetStats {
        *self.stats.lock().unwrap()
    }

    /// Stops the server, closes every connection and returns the session.
    pub fn stop(self) -> std::io::Result<Session> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap_or_else(|_| Err(std::io::Error::other("the server thread panicked")))
    }
}

/// Runs a session on a listener with the default [`ServerConfig`] until
/// `stop` is set. The same as `Server::from_listener(listener).run(..)`.
pub fn serve(listener: TcpListener, session: Session, stop: &AtomicBool) -> std::io::Result<Session> {
    Server::from_listener(listener).run(session, stop)
}

enum Incoming {
    Message(ConnId, ClientMsg, usize),
    Malformed(ConnId, String, usize),
    TooLong(ConnId),
    Disconnected(ConnId),
}

/// One connected client as the server loop sees it.
struct Peer {
    /// Lines waiting for the writer thread. Dropping this ends that thread
    /// once it has written what is queued, and closes the socket.
    queue: SyncSender<Arc<str>>,
    last_heard: Instant,
    last_ping: Instant,
    /// Messages the client may still send right now.
    allowance: f64,
    allowance_at: Instant,
}

/// Why a connection is being dropped by the server.
enum Drop {
    Slow,
    Idle,
    Flood,
}

struct Peers {
    peers: BTreeMap<ConnId, Peer>,
    stats: Arc<Mutex<NetStats>>,
}

impl Peers {
    /// Queues a line for a client. Returns false if the client has fallen
    /// too far behind (or has gone).
    fn queue(&mut self, conn: ConnId, line: Arc<str>) -> bool {
        let Some(peer) = self.peers.get(&conn) else { return true };
        let bytes = line.len() as u64 + 1;
        match peer.queue.try_send(line) {
            Ok(()) => {
                let mut stats = self.stats.lock().unwrap();
                stats.messages_out += 1;
                stats.bytes_out += bytes;
                true
            }
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => true, // the reader will report it
        }
    }

    fn send(&mut self, conn: ConnId, message: &ServerMsg) -> bool {
        self.queue(conn, encode(message).into())
    }

    /// Drops a client, telling it why if it is still listening.
    fn drop(&mut self, conn: ConnId, why: Drop, session: &mut Session, now: u64) {
        let reason = match why {
            Drop::Slow => "disconnected: this connection is not keeping up",
            Drop::Idle => "disconnected: nothing was heard from this connection for too long",
            Drop::Flood => "disconnected: too many messages, or one that was too long",
        };
        self.send(conn, &ServerMsg::Error { message: reason.into() });
        if self.peers.remove(&conn).is_some() {
            let mut stats = self.stats.lock().unwrap();
            stats.connections = self.peers.len();
            match why {
                Drop::Slow => stats.dropped_slow += 1,
                Drop::Idle => stats.dropped_idle += 1,
                Drop::Flood => stats.dropped_flood += 1,
            }
        }
        session.disconnect(conn, now);
    }
}

/// Reads a client's lines and forwards them to the server loop.
fn reader_thread(conn: ConnId, stream: TcpStream, limit: usize, tx: SyncSender<Incoming>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        let incoming = match read_line_limited(&mut reader, &mut line, limit) {
            Ok(Some(bytes)) => {
                let text = line.trim();
                if text.is_empty() {
                    continue;
                }
                match decode::<ClientMsg>(text) {
                    Ok(message) => Incoming::Message(conn, message, bytes),
                    Err(e) => Incoming::Malformed(conn, e, bytes),
                }
            }
            Ok(None) => break,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                tx.send(Incoming::TooLong(conn)).ok();
                return;
            }
            Err(_) => break,
        };
        // The channel is bounded, so a flood waits here instead of piling up.
        if tx.send(incoming).is_err() {
            return;
        }
    }
    tx.send(Incoming::Disconnected(conn)).ok();
}

/// Writes queued lines to a client. Ends, closing the socket, when a write
/// fails or the server lets go of the queue.
fn writer_thread(mut stream: TcpStream, lines: mpsc::Receiver<Arc<str>>) {
    for line in lines {
        if stream.write_all(line.as_bytes()).and_then(|_| stream.write_all(b"\n")).is_err() {
            break;
        }
    }
    stream.shutdown(std::net::Shutdown::Both).ok();
}

fn run(
    listener: TcpListener,
    mut session: Session,
    stop: &AtomicBool,
    config: ServerConfig,
    stats: Arc<Mutex<NetStats>>,
) -> std::io::Result<Session> {
    listener.set_nonblocking(true)?;
    let (tx, rx) = mpsc::sync_channel::<Incoming>(1024);
    let started = Instant::now();
    let mut peers = Peers { peers: BTreeMap::new(), stats };
    let mut next_conn: ConnId = 1;

    while !stop.load(Ordering::Relaxed) {
        // Accept anyone waiting.
        loop {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // One failed accept (a client that gave up, or too many open
                // files) must not take the server down. Try again next turn.
                Err(_) => break,
            };
            stream.set_nonblocking(false).ok();
            stream.set_nodelay(true).ok();
            stream.set_write_timeout(Some(config.write_timeout)).ok();
            if peers.peers.len() >= config.max_connections {
                let mut stream = stream;
                stream.set_write_timeout(Some(Duration::from_millis(200))).ok();
                let refusal = ServerMsg::Error { message: "the server is full".into() };
                let _ = stream.write_all(format!("{}\n", encode(&refusal)).as_bytes());
                peers.stats.lock().unwrap().refused += 1;
                continue;
            }
            let Ok(reader) = stream.try_clone() else { continue };
            let conn = next_conn;
            next_conn += 1;
            let (queue, lines) = mpsc::sync_channel::<Arc<str>>(config.send_queue.max(1));
            let now = Instant::now();
            peers.peers.insert(
                conn,
                Peer { queue, last_heard: now, last_ping: now, allowance: config.burst as f64, allowance_at: now },
            );
            {
                let mut stats = peers.stats.lock().unwrap();
                stats.accepted += 1;
                stats.connections = peers.peers.len();
            }
            let tx = tx.clone();
            let limit = config.max_line_bytes;
            std::thread::spawn(move || reader_thread(conn, reader, limit, tx));
            std::thread::spawn(move || writer_thread(stream, lines));
        }

        let now = started.elapsed().as_millis() as u64;
        let handle = |incoming: Incoming, session: &mut Session, peers: &mut Peers| {
            let (conn, bytes) = match &incoming {
                Incoming::Message(conn, _, bytes) | Incoming::Malformed(conn, _, bytes) => (*conn, *bytes),
                Incoming::TooLong(conn) => return peers.drop(*conn, Drop::Flood, session, now),
                Incoming::Disconnected(conn) => {
                    if peers.peers.remove(conn).is_some() {
                        peers.stats.lock().unwrap().connections = peers.peers.len();
                    }
                    return session.disconnect(*conn, now);
                }
            };
            // Ignore anything still in flight from a connection we dropped.
            let Some(peer) = peers.peers.get_mut(&conn) else { return };
            let at = Instant::now();
            peer.last_heard = at;
            let earned = at.duration_since(peer.allowance_at).as_secs_f64() * config.messages_per_second as f64;
            peer.allowance = (peer.allowance + earned).min(config.burst as f64);
            peer.allowance_at = at;
            if peer.allowance < 1.0 {
                return peers.drop(conn, Drop::Flood, session, now);
            }
            peer.allowance -= 1.0;
            {
                let mut stats = peers.stats.lock().unwrap();
                stats.messages_in += 1;
                stats.bytes_in += bytes as u64;
            }
            match incoming {
                Incoming::Message(_, ClientMsg::Pong, _) => {}
                Incoming::Message(_, ClientMsg::Ping, _) => {
                    peers.send(conn, &ServerMsg::Pong);
                }
                Incoming::Message(_, message, _) => session.handle(conn, message, now),
                Incoming::Malformed(_, error, _) => {
                    peers.send(conn, &ServerMsg::Error { message: format!("could not read that message: {error}") });
                }
                Incoming::TooLong(_) | Incoming::Disconnected(_) => unreachable!("handled above"),
            }
        };
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(incoming) => {
                handle(incoming, &mut session, &mut peers);
                // A bounded batch, so timers and writes still get their turn.
                for more in rx.try_iter().take(2048) {
                    handle(more, &mut session, &mut peers);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        session.tick(now);

        // Ping the quiet, drop the silent.
        let at = Instant::now();
        let mut silent = Vec::new();
        let mut quiet = Vec::new();
        for (conn, peer) in &mut peers.peers {
            let since = at.duration_since(peer.last_heard);
            if since >= config.idle_timeout {
                silent.push(*conn);
            } else if since >= config.ping_interval && at.duration_since(peer.last_ping) >= config.ping_interval {
                peer.last_ping = at;
                quiet.push(*conn);
            }
        }
        if !quiet.is_empty() {
            let ping: Arc<str> = encode(&ServerMsg::Ping).into();
            for conn in quiet {
                peers.queue(conn, ping.clone());
            }
        }
        for conn in silent {
            peers.drop(conn, Drop::Idle, &mut session, now);
        }

        // Deliver what the session has to say. Dropping a client can make
        // the session say more (a bot takes the seat), hence the loop.
        loop {
            let outbox = session.drain_outbox();
            if outbox.is_empty() {
                break;
            }
            let mut slow = Vec::new();
            for (conn, message) in outbox {
                if !slow.contains(&conn) && !peers.send(conn, &message) {
                    slow.push(conn);
                }
            }
            for conn in slow {
                peers.drop(conn, Drop::Slow, &mut session, now);
            }
        }
    }
    // Tell the session everyone has gone, so the returned session can be
    // hosted again and its players can take their seats back. Letting go of
    // the queues makes each writer finish and close its socket.
    let now = started.elapsed().as_millis() as u64;
    for conn in std::mem::take(&mut peers.peers).into_keys() {
        session.disconnect(conn, now);
    }
    session.drain_outbox();
    peers.stats.lock().unwrap().connections = 0;
    Ok(session)
}
