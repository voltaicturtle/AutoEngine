# Networking

A game runs on a headless server. Clients hold no rules: they send what the
player does and draw what the server says. Everything goes over TCP as one
line of JSON per message, using only the Rust standard library.

## Hosting

```
autoengine-server                                  # the sample game, local only, port 7777
cargo run -- serve                                 # from a game crate: hosts that game
autoengine-server --bind 0.0.0.0 --port 7777       # reachable from other machines
autoengine-server --stats-secs 10                  # print traffic counters every 10 s
```

From Rust:

```rust
use autoengine::net::Server;
use autoengine::prelude::*;

let content = autoengine::sample::content();          // or your game's content()
let session = Session::new(content, SessionConfig::default());
let server = Server::bind("0.0.0.0:7777")?.spawn(session);   // runs in the background
println!("{:?}", server.stats());                           // connections, drops, bytes
let session = server.stop()?;                               // hand back the game
```

`Server::run(session, &stop)` does the same on the calling thread. Add
round types and hooks to a hosted game with `Session::on_start`.

## Joining

`GameClient` is everything a player's program needs:

```rust
use autoengine::net::GameClient;
use autoengine::game::Command;

let mut client = GameClient::join("127.0.0.1:7777", "Ana");   // or GameClient::spectate(addr)
loop {
    client.update();                       // never blocks; call it every frame
    if let Some(me) = client.state.me() {  // everything known about the game
        if me.gold >= 4 {
            client.command(Command::BuyUnit { slot: 0 });
        }
    }
    // client.state.combats holds fights ready to draw
}
```

It connects in the background, joins, answers the server's pings, and if
the connection drops it keeps trying (half a second, doubling up to five)
and takes the same seat back. A bot plays the seat meanwhile.
`client.status()` says whether it is connecting, connected, or retrying and
why. `examples/network_game.rs` is a server and two players in one program.

For full control, `Connection` is the bare socket: send `ClientMsg`, receive
`ServerMsg`, and fold them into a `ClientState` yourself.

## The server decides everything

Clients send intents and draw what they are told. Nothing a client works
out is ever used as a result.

- Every fight runs on the server, which applies the outcome (damage,
  health, gold, eliminations) before telling anyone. A client that replays
  a fight from its seed shows it only if a checksum of every event matches
  the server's; otherwise it downloads the server's log.
- Figures that follow from the rules (active traits and their breakpoints,
  how many units fit on the board, experience to the next level) are worked
  out by the server and sent in `GameView::status`. The client only looks
  up static definitions, such as names and costs, from the content the
  server sent on joining.
- A `Command` names no player: it always acts on the sender's seat. The game
  checks it against its own state (gold, shop, bench, board half and size,
  item slots, the round's offers) and refuses it with a reason, changing
  nothing, if it is not allowed. Commands are refused outside the planning
  phase and from spectators.
- A disconnected seat can only be reclaimed with the secret token the server
  handed out. Cheats need a server started with `--dev`. Players can fetch
  only their own fights; spectators can fetch all of them.
- Timers run on the server's clock. The countdown a client shows is the
  server's remaining time.

`tests/authority.rs` checks each of these. It also lists every message a
client can send with the reason it cannot set a result, and stops compiling
when a new message is added until that reason is written down.

## What is sent

| Message | When |
| --- | --- |
| `Welcome` | once on joining: your seat, a token to reclaim it, and the content |
| `State` | once on joining: the whole game as you may see it |
| `Update` | after anything changes: only the players that changed, plus the round header |
| `Events` | what happened, for the log (other players' shops are left out) |
| `CombatSeed` | a fight as its teams, seed and event checksum; the client fights it again |
| `Ping` / `Pong` | keep-alive, handled by the library |

Fights are deterministic, so the server sends the teams and the seed
(about 700 bytes) instead of the event log (about 170 KB on average for the
sample content) and the client replays it with the content it received on
joining. If any event differs from the server's (the checksum covers them
all), because the client runs a different engine version for instance, it
asks for the whole log with `ResendCombat` and gets it. A client that cannot run the engine sends
`Options { full_fights: true }` once to always get whole logs.

Measured over one eight-player game with the sample content, one player
receives about 0.6 MB, down from 8.5 MB when every message carried the
whole state and every fight its whole log.

## What the server guards against

| Problem | What happens | Setting (`ServerConfig`) | Default |
| --- | --- | --- | --- |
| a client that stops reading | dropped once its queue is full or a write blocks | `send_queue`, `write_timeout` | 512 messages, 10 s |
| a client that vanishes | pinged when quiet, dropped when silent | `ping_interval`, `idle_timeout` | 5 s, 30 s |
| a flood of messages | dropped once over its allowance | `messages_per_second`, `burst` | 40, 120 |
| an enormous line | dropped | `max_line_bytes` | 64 KB |
| too many connections | turned away with "the server is full" | `max_connections` | 64 |
| text that is not a message | an error reply; the connection stays | | |

Each connection has its own reader and writer thread, so one slow or silent
client never holds up the game or anyone else. A dropped client is told why
before the socket closes. `--max-connections` and `--idle-secs` set two of
these from the command line.

On the client side, a server that goes quiet is pinged and the connection
counts as lost after 30 seconds of silence (`Connection::set_quiet_time`).
Connecting gives up after 5 seconds, and a server that accepts the
connection but never welcomes it is retried after 10.

## Limits

There is no encryption and there are no accounts. A seat token stops
someone else taking your seat by name, but anyone who can reach the port can
join or watch. Host games for people you trust, or put the server behind a
VPN, SSH tunnel or TLS proxy. The server is authoritative, so a modified
client can only send commands a normal client could.

The protocol version is checked on joining. This change raised it to 2, so
clients and servers built before it cannot play together.
