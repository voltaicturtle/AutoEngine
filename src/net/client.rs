//! The client end: a bare [`Connection`], and [`GameClient`] on top of it.

use std::io::{BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::read_line_limited;
use crate::client::ClientState;
use crate::game::Command;
use crate::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg, decode, encode};

/// The longest line a client will read from a server.
const MAX_LINE: usize = 32 * 1024 * 1024;
/// How long one connection attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn write_line(stream: &Mutex<TcpStream>, line: &str) -> std::io::Result<()> {
    let mut stream = stream.lock().unwrap();
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")
}

/// A socket to a server: send [`ClientMsg`]s, receive [`ServerMsg`]s.
///
/// Pings are handled here and never reach the caller: the server's are
/// answered, and when the server has been quiet for a while it is pinged
/// in turn, so a dead connection is noticed ([`Connection::is_closed`])
/// even when nothing is being sent. Most programs want [`GameClient`],
/// which also joins and reconnects.
pub struct Connection {
    stream: Arc<Mutex<TcpStream>>,
    incoming: Receiver<Result<ServerMsg, String>>,
    closed: Arc<AtomicBool>,
    last_heard: Instant,
    last_ping: Instant,
    /// Silence after which the server is pinged; three times this and the
    /// connection counts as lost.
    quiet: Duration,
}

impl Connection {
    /// Connects, waiting a few seconds at most.
    pub fn connect(addr: impl ToSocketAddrs) -> std::io::Result<Self> {
        let mut last_error = std::io::Error::new(std::io::ErrorKind::InvalidInput, "the address did not resolve");
        let mut found = None;
        for addr in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    found = Some(stream);
                    break;
                }
                Err(e) => last_error = e,
            }
        }
        let stream = found.ok_or(last_error)?;
        stream.set_nodelay(true).ok();
        stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
        let reader = stream.try_clone()?;
        let stream = Arc::new(Mutex::new(stream));
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, incoming) = mpsc::channel();
        {
            let (stream, closed) = (stream.clone(), closed.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(reader);
                let mut line = String::new();
                while let Ok(Some(_)) = read_line_limited(&mut reader, &mut line, MAX_LINE) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let message = decode::<ServerMsg>(line.trim());
                    // Answer pings here, so they are answered even while the
                    // program is busy doing something else.
                    if message == Ok(ServerMsg::Ping) && write_line(&stream, &encode(&ClientMsg::Pong)).is_err() {
                        break;
                    }
                    if tx.send(message).is_err() {
                        break;
                    }
                }
                closed.store(true, Ordering::Relaxed);
            });
        }
        let now = Instant::now();
        Ok(Connection { stream, incoming, closed, last_heard: now, last_ping: now, quiet: Duration::from_secs(10) })
    }

    /// Changes how long the server may be silent before it is pinged. The
    /// connection counts as lost after three times this (30 seconds unless
    /// changed).
    pub fn set_quiet_time(&mut self, quiet: Duration) {
        self.quiet = quiet;
    }

    pub fn send(&mut self, message: &ClientMsg) -> std::io::Result<()> {
        let sent = write_line(&self.stream, &encode(message));
        if sent.is_err() {
            self.closed.store(true, Ordering::Relaxed);
        }
        sent
    }

    /// Notes a message from the server; returns it unless it is a ping or
    /// pong, which callers never see.
    fn heard(&mut self, message: Result<ServerMsg, String>) -> Option<ServerMsg> {
        self.last_heard = Instant::now();
        match message {
            Ok(ServerMsg::Ping | ServerMsg::Pong) => None,
            Ok(message) => Some(message),
            Err(_) => None, // a line this client cannot read; skip it
        }
    }

    /// Pings a quiet server and gives up on a silent one.
    fn keep_alive(&mut self) {
        let now = Instant::now();
        let silence = now.duration_since(self.last_heard);
        if silence >= self.quiet * 3 {
            self.closed.store(true, Ordering::Relaxed);
        } else if silence >= self.quiet && now.duration_since(self.last_ping) >= self.quiet {
            self.last_ping = now;
            let _ = self.send(&ClientMsg::Ping);
        }
    }

    /// Messages that have arrived since the last call. Never blocks.
    pub fn poll(&mut self) -> Vec<ServerMsg> {
        let mut out = Vec::new();
        loop {
            match self.incoming.try_recv() {
                Ok(message) => out.extend(self.heard(message)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.closed.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        self.keep_alive();
        out
    }

    /// Waits up to `timeout` for the next message.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<ServerMsg> {
        let deadline = Instant::now() + timeout;
        loop {
            self.keep_alive();
            let left = deadline.checked_duration_since(Instant::now())?;
            match self.incoming.recv_timeout(left.min(self.quiet)) {
                Ok(message) => {
                    if let Some(message) = self.heard(message) {
                        return Some(message);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.closed.store(true, Ordering::Relaxed);
                    return None;
                }
            }
        }
    }

    /// True once the connection is lost: closed by either end, or silent
    /// for too long.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stream.lock().unwrap().shutdown(std::net::Shutdown::Both).ok();
    }
}

/// Where a [`GameClient`] stands with its server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The first attempt is under way.
    Connecting,
    /// Joined: the server has welcomed this client.
    Connected,
    /// The connection failed or was lost and another attempt is coming.
    Reconnecting { reason: String, attempts: u32 },
}

/// A player's (or spectator's) whole connection to a game.
///
/// It connects in the background, joins, folds everything the server sends
/// into [`GameClient::state`], and reconnects into the same seat when the
/// connection drops. Call [`GameClient::update`] regularly (every frame, or
/// in a loop) and read `state`.
///
/// ```no_run
/// use autoengine::net::GameClient;
/// use autoengine::game::Command;
///
/// let mut client = GameClient::join("127.0.0.1:7777", "Ana");
/// loop {
///     client.update();
///     if let Some(me) = client.state.me() {
///         println!("{} gold", me.gold);
///         client.command(Command::Ready);
///     }
///     std::thread::sleep(std::time::Duration::from_millis(50));
/// }
/// ```
pub struct GameClient {
    addr: String,
    /// `None` to spectate.
    name: Option<String>,
    full_fights: bool,
    /// Everything known about the game. Read it freely.
    pub state: ClientState,
    conn: Option<Connection>,
    /// A connection attempt running on another thread.
    attempt: Option<Receiver<std::io::Result<Connection>>>,
    status: Status,
    attempts: u32,
    retry_at: Instant,
    /// When the current connection was made, until the server welcomes us.
    connected_at: Option<Instant>,
}

/// How long a server has to welcome a new connection before it is dropped
/// and tried again.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);

impl GameClient {
    /// Starts connecting to take a seat under `name`. Returns at once.
    pub fn join(addr: impl Into<String>, name: impl Into<String>) -> Self {
        Self::new(addr.into(), Some(name.into()))
    }

    /// Starts connecting to watch. Returns at once.
    pub fn spectate(addr: impl Into<String>) -> Self {
        Self::new(addr.into(), None)
    }

    fn new(addr: String, name: Option<String>) -> Self {
        let mut client = GameClient {
            addr,
            name,
            full_fights: false,
            state: ClientState::new(),
            conn: None,
            attempt: None,
            status: Status::Connecting,
            attempts: 0,
            retry_at: Instant::now(),
            connected_at: None,
        };
        client.begin_attempt();
        client
    }

    /// A client with no server, which never connects. For programs that
    /// show something saved (a replay, say) through the same code that
    /// shows a live game.
    pub fn offline() -> Self {
        GameClient {
            addr: String::new(),
            name: None,
            full_fights: false,
            state: ClientState::new(),
            conn: None,
            attempt: None,
            status: Status::Connecting,
            attempts: 0,
            retry_at: Instant::now() + Duration::from_secs(60 * 60 * 24 * 365),
            connected_at: None,
        }
    }

    /// Asks the server for fights with their whole event log instead of
    /// fighting them again locally. Only useful for measuring or debugging.
    pub fn with_full_fights(mut self) -> Self {
        self.full_fights = true;
        self
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn is_connected(&self) -> bool {
        self.status == Status::Connected
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    fn begin_attempt(&mut self) {
        let (tx, rx) = mpsc::channel();
        let addr = self.addr.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Connection::connect(addr.as_str()));
        });
        self.attempt = Some(rx);
    }

    fn lost(&mut self, reason: String) {
        self.conn = None;
        self.connected_at = None;
        self.attempts += 1;
        // Half a second, then doubling, up to five seconds between tries.
        let wait = Duration::from_millis(500) * (1u32 << (self.attempts - 1).min(4));
        self.retry_at = Instant::now() + wait.min(Duration::from_secs(5));
        self.status = Status::Reconnecting { reason, attempts: self.attempts };
    }

    /// Does everything that is due: finishes or retries connecting, reads
    /// what the server sent into `state`, and sends what `state` asks for.
    /// Never blocks. Returns true if anything arrived.
    pub fn update(&mut self) -> bool {
        // A connection attempt that has finished.
        if let Some(attempt) = &self.attempt {
            match attempt.try_recv() {
                Ok(Ok(mut conn)) => {
                    self.attempt = None;
                    let hello = match &self.name {
                        Some(name) => ClientMsg::Join {
                            name: name.clone(),
                            version: PROTOCOL_VERSION,
                            token: self.state.token.clone(),
                        },
                        None => ClientMsg::Spectate { version: PROTOCOL_VERSION },
                    };
                    let mut sent = conn.send(&hello);
                    if self.full_fights && sent.is_ok() {
                        sent = conn.send(&ClientMsg::Options { full_fights: true });
                    }
                    match sent {
                        Ok(()) => {
                            self.state.welcomed = false;
                            self.conn = Some(conn);
                            self.connected_at = Some(Instant::now());
                        }
                        Err(e) => self.lost(format!("cannot reach the server at {}: {e}", self.addr)),
                    }
                }
                Ok(Err(e)) => {
                    self.attempt = None;
                    self.lost(format!("cannot reach the server at {}: {e}", self.addr));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.attempt = None;
                    self.lost("the connection attempt failed".into());
                }
            }
        }
        if self.conn.is_none() && self.attempt.is_none() && Instant::now() >= self.retry_at {
            self.begin_attempt();
        }

        let mut arrived = false;
        if let Some(conn) = &mut self.conn {
            for message in conn.poll() {
                arrived = true;
                self.state.apply(message);
            }
            for request in std::mem::take(&mut self.state.requests) {
                let _ = conn.send(&request);
            }
            if self.state.welcomed && self.status != Status::Connected {
                self.status = Status::Connected;
                self.attempts = 0;
                self.connected_at = None;
            }
            if conn.is_closed() {
                self.lost("lost the connection to the server; retrying".into());
            } else if self.connected_at.is_some_and(|at| at.elapsed() >= WELCOME_TIMEOUT) {
                // Connected but never let in: the server is not answering, or
                // refused us with an error (which is in `state.error`).
                let reason = self.state.error.clone().unwrap_or_else(|| "the server did not answer".into());
                self.lost(format!("{reason}; retrying"));
            }
        }
        arrived
    }

    /// Sends a message if connected. Returns whether it was sent.
    pub fn send(&mut self, message: ClientMsg) -> bool {
        match &mut self.conn {
            Some(conn) => conn.send(&message).is_ok(),
            None => false,
        }
    }

    /// Sends a game command for this client's seat.
    pub fn command(&mut self, command: Command) -> bool {
        self.send(ClientMsg::Command(command))
    }

    /// Asks the server to start the game, filling empty seats with bots.
    pub fn start(&mut self) -> bool {
        self.send(ClientMsg::Start)
    }

    /// Keeps updating until `done` says so or `timeout` passes. Returns
    /// whether it did. For programs with no loop of their own, such as
    /// tests and scripts.
    pub fn wait_until(&mut self, timeout: Duration, mut done: impl FnMut(&ClientState) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.update();
            if done(&self.state) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Drops the connection and connects again, to a new address if given.
    pub fn reconnect(&mut self, addr: Option<String>) {
        if let Some(addr) = addr {
            self.addr = addr;
        }
        self.conn = None;
        self.attempt = None;
        self.status = Status::Connecting;
        self.begin_attempt();
    }
}
