//! TCP transport: one line of JSON per message.
//!
//! The two ends, from simplest to most control:
//!
//! * [`Server`] hosts a [`Session`](crate::session::Session) on a socket.
//!   `Server::bind(addr)?.spawn(session)` runs it in the background and
//!   `.run(session, &stop)` runs it on the calling thread.
//! * [`GameClient`] is everything a player's program needs: it connects in
//!   the background, joins, keeps a [`ClientState`](crate::client::ClientState)
//!   up to date, answers the server's pings, and reconnects into the same
//!   seat when the connection drops.
//! * [`Connection`] is the bare client socket, for programs that want to
//!   handle messages themselves.
//!
//! Only the standard library is used. The server keeps the session on one
//! thread and gives every connection a reader and a writer thread, so a
//! slow or silent client can never hold up the game or the other players.
//!
//! # What the server guards against
//!
//! | Problem | What happens | [`ServerConfig`] field |
//! | --- | --- | --- |
//! | a client that stops reading | its queue fills and it is dropped | `send_queue`, `write_timeout` |
//! | a client that vanishes without closing | pinged, then dropped when silent too long | `ping_interval`, `idle_timeout` |
//! | a flood of messages | dropped once over the allowance | `messages_per_second`, `burst` |
//! | an enormous line | dropped | `max_line_bytes` |
//! | too many connections | refused with a message | `max_connections` |
//! | text that is not a message | an error reply; the connection stays | |
//!
//! A dropped player's seat is played by a bot until they reconnect.
//!
//! There is no encryption and there are no accounts: host games for people
//! you trust, or put the server behind a tunnel or proxy that adds them.

mod client;
mod server;

pub use client::{Connection, GameClient, Status};
pub use server::{NetStats, Server, ServerConfig, ServerHandle, serve};

use std::io::{BufRead, Read};

/// Reads one line of at most `limit` bytes. `Ok(None)` is the end of the
/// stream, and a line that is too long is an error.
fn read_line_limited<R: BufRead>(reader: &mut R, line: &mut String, limit: usize) -> std::io::Result<Option<usize>> {
    line.clear();
    let mut limited = reader.take(limit as u64 + 1);
    let read = limited.read_line(line)?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') && read > limit {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the line is too long"));
    }
    Ok(Some(read))
}
