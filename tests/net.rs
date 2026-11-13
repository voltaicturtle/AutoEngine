//! The network layer under good and bad conditions.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use autoengine::client::ClientState;
use autoengine::content::ContentSet;
use autoengine::game::{Command, PlayerId};
use autoengine::net::{Connection, GameClient, Server, ServerConfig, ServerHandle, Status};
use autoengine::protocol::{ClientMsg, GameView, PROTOCOL_VERSION, ServerMsg, ViewPhase, encode};
use autoengine::session::{Session, SessionConfig};

fn content() -> Arc<ContentSet> {
    Arc::new(ContentSet::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/content")).unwrap())
}

fn session(seats: usize) -> Session {
    Session::new(content(), SessionConfig { seats, seed: 7, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 })
}

fn host(seats: usize, config: ServerConfig) -> ServerHandle {
    Server::bind("127.0.0.1:0").unwrap().with_config(config).spawn(session(seats))
}

const WAIT: Duration = Duration::from_secs(20);

/// Waits until `done` holds for the server's counters.
fn eventually(server: &ServerHandle, what: &str, done: impl Fn(&autoengine::net::NetStats) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done(&server.stats()) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?}", server.stats());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Does what a patient player would: takes the first option when the
/// round offers a pick, and readies up once per round.
fn act(client: &mut GameClient, readied: &mut Option<usize>) {
    let (Some(view), Some(me)) = (client.state.view.clone(), client.state.seat) else { return };
    if !matches!(view.phase, ViewPhase::Planning { .. }) {
        return;
    }
    if view.awaiting.contains(&me) {
        client.command(Command::Pick { option: view.options.iter().position(|o| o.is_some()).unwrap_or(0) });
    } else if *readied != Some(view.round_index) {
        *readied = Some(view.round_index);
        client.command(Command::Ready);
    }
}

/// Plays until round number `rounds` has begun.
fn play_rounds(client: &mut GameClient, rounds: usize) {
    let mut readied = None;
    let deadline = Instant::now() + WAIT;
    loop {
        client.update();
        let round = client.state.view.as_ref().map(|v| v.round_index);
        if round.is_some_and(|r| r >= rounds) {
            return;
        }
        act(client, &mut readied);
        assert!(Instant::now() < deadline, "timed out in round {round:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_game_client_joins_plays_and_fights_locally_what_the_server_fought() {
    let server = host(2, ServerConfig::default());
    let mut ana = GameClient::join(server.addr().to_string(), "Ana");
    assert_eq!(*ana.status(), Status::Connecting);
    assert!(ana.wait_until(WAIT, |s| s.welcomed), "joined");
    assert!(ana.is_connected());
    assert_eq!(ana.state.seat, Some(PlayerId(0)));

    // Someone watching asks for whole fight logs, to compare with.
    let mut watcher = GameClient::spectate(server.addr().to_string()).with_full_fights();
    assert!(watcher.wait_until(WAIT, |s| s.welcomed));

    assert!(ana.start());
    assert!(ana.wait_until(WAIT, |s| s.view.is_some()));
    play_rounds(&mut ana, 3);
    watcher.wait_until(WAIT, |s| s.view.as_ref().is_some_and(|v| v.round_index >= 3));

    // Ana was sent seeds, fought them here, and has the same events the
    // server recorded.
    assert!(ana.state.combats.len() >= 3);
    for fight in &ana.state.combats {
        assert!(!fight.events.is_empty());
        let same = watcher.state.combats.iter().find(|w| w.seed == fight.seed).expect("the watcher saw it too");
        assert_eq!(same.events, fight.events);
    }
    // Stop the server, let both clients read everything it sent before it
    // went, and they agree on everything public though each folded its own
    // stream of updates.
    let session = server.stop().unwrap();
    assert!(session.game().unwrap().round_index() >= 3);
    for client in [&mut ana, &mut watcher] {
        let deadline = Instant::now() + WAIT;
        while client.is_connected() {
            client.update();
            assert!(Instant::now() < deadline, "the client noticed the server stopping");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let public = |state: &ClientState| -> Vec<(i64, i64, u32)> {
        state.view.as_ref().unwrap().players.iter().map(|p| (p.gold, p.health, p.level)).collect()
    };
    assert_eq!(public(&ana.state), public(&watcher.state));
    assert_eq!(ana.state.view.as_ref().unwrap().round_index, watcher.state.view.as_ref().unwrap().round_index);
}

#[test]
fn updates_and_seeds_are_much_smaller_than_whole_states_and_fights() {
    // Two watchers from the start: one folds updates, one asks for full fights.
    let mut s = session(4);
    let join = |name: &str| ClientMsg::Join { name: name.into(), version: PROTOCOL_VERSION, token: None };
    s.handle(1, join("Ana"), 0);
    s.handle(2, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    s.handle(3, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    s.handle(3, ClientMsg::Options { full_fights: true }, 0);
    s.handle(1, ClientMsg::Start, 0);

    let mut folded = ClientState::new();
    let (mut update_bytes, mut state_bytes, mut seed_bytes, mut fight_bytes) = (0usize, 0usize, 0usize, 0usize);
    let mut late_joiner = 100;
    for now in 1..400u64 {
        // Ana does a few things, then readies up.
        let action = match now % 4 {
            0 => Command::Ready,
            1 => Command::BuyUnit { slot: (now % 5) as usize },
            2 => Command::Reroll,
            _ => Command::BuyXp,
        };
        s.handle(1, ClientMsg::Command(action), now);
        s.tick(now);
        for (conn, message) in s.drain_outbox() {
            match (conn, &message) {
                (2, ServerMsg::Update(_)) => {
                    update_bytes += encode(&message).len();
                    folded.apply(message);
                    // What a whole state would have cost instead.
                    state_bytes += encode(&ServerMsg::State(folded.view.clone().unwrap())).len();
                }
                (2, ServerMsg::CombatSeed { .. }) => {
                    seed_bytes += encode(&message).len();
                    folded.apply(message);
                }
                (2, _) => folded.apply(message),
                (3, ServerMsg::Combat(_)) => fight_bytes += encode(&message).len(),
                _ => {}
            }
        }
        // A watcher who joins now is sent the whole state, and it is exactly
        // what the one who has been folding updates holds.
        if now % 50 == 0 && !s.is_over() {
            late_joiner += 1;
            s.handle(late_joiner, ClientMsg::Spectate { version: PROTOCOL_VERSION }, now);
            let fresh: Vec<GameView> = s
                .drain_outbox()
                .into_iter()
                .filter_map(|(c, m)| match m {
                    ServerMsg::State(v) if c == late_joiner => Some(v),
                    _ => None,
                })
                .collect();
            let (mut fresh, mut held) = (fresh[0].clone(), folded.view.clone().unwrap());
            // The countdown is the one thing updates do not chase.
            fresh.phase = ViewPhase::Lobby;
            held.phase = ViewPhase::Lobby;
            assert_eq!(fresh, held, "at {now}");
            s.disconnect(late_joiner, now);
        }
        if s.is_over() {
            break;
        }
    }
    assert!(update_bytes > 0 && seed_bytes > 0 && fight_bytes > 0);
    assert!(update_bytes * 2 < state_bytes, "updates {update_bytes} against states {state_bytes}");
    assert!(seed_bytes * 5 < fight_bytes, "seeds {seed_bytes} against fights {fight_bytes}");
    // The seeded fights all arrived as real fights.
    assert!(!folded.combats.is_empty() && folded.requests.is_empty());
}

#[test]
fn a_fight_that_plays_out_differently_is_asked_for_in_full() {
    let mut s = session(2);
    s.handle(1, ClientMsg::Join { name: "Ana".into(), version: PROTOCOL_VERSION, token: None }, 0);
    s.handle(1, ClientMsg::Start, 0);
    s.handle(1, ClientMsg::Command(Command::Ready), 1);
    let mut state = ClientState::new();
    for (_, message) in s.drain_outbox() {
        // A client whose engine disagrees: pretend the seed says something else.
        let message = match message {
            ServerMsg::CombatSeed { id, events, record } => ServerMsg::CombatSeed { id, events: events + 1, record },
            other => other,
        };
        state.apply(message);
    }
    assert!(state.combats.is_empty());
    let [ClientMsg::ResendCombat { id }] = state.requests[..] else { panic!("expected one request: {:?}", state.requests) };
    s.handle(1, ClientMsg::ResendCombat { id }, 2);
    let replies = s.drain_outbox();
    assert!(matches!(&replies[..], [(1, ServerMsg::Combat(record))] if !record.events.is_empty()));
    // Asking for something that is gone is refused, not ignored.
    s.handle(1, ClientMsg::ResendCombat { id: id + 1000 }, 3);
    assert!(matches!(&s.drain_outbox()[..], [(1, ServerMsg::Error { .. })]));
}

#[test]
fn a_client_gets_its_seat_back_when_the_server_comes_back() {
    let server = host(2, ServerConfig::default());
    let addr = server.addr();
    let mut ana = GameClient::join(addr.to_string(), "Ana");
    assert!(ana.wait_until(WAIT, |s| s.welcomed));
    ana.start();
    play_rounds(&mut ana, 2);

    // The server goes away mid-game...
    let session = server.stop().unwrap();
    let deadline = Instant::now() + WAIT;
    while !matches!(ana.status(), Status::Reconnecting { .. }) {
        ana.update();
        assert!(Instant::now() < deadline, "the client noticed the loss");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!ana.command(Command::Ready), "nothing can be sent while disconnected");

    // ...and comes back on the same address with the same game.
    let server = Server::bind(addr).unwrap().spawn(session);
    let deadline = Instant::now() + WAIT;
    while !ana.is_connected() {
        ana.update();
        assert!(Instant::now() < deadline, "the client reconnected: {:?}", ana.status());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(ana.state.seat, Some(PlayerId(0)), "the same seat");
    let round = ana.state.view.as_ref().unwrap().round_index;
    play_rounds(&mut ana, round + 1);
    // Stopping hands back a session with everyone marked gone, ready to be
    // hosted again.
    let session = server.stop().unwrap();
    assert!(session.seat_is_bot(0));
}

/// A raw socket that says hello and then does whatever the test wants.
fn raw(server: &ServerHandle, hello: &[ClientMsg]) -> TcpStream {
    let mut stream = TcpStream::connect(server.addr()).unwrap();
    for message in hello {
        stream.write_all(format!("{}\n", encode(message)).as_bytes()).unwrap();
    }
    stream
}

/// Everything the server sends until it closes the connection.
fn read_to_close(stream: TcpStream) -> Vec<String> {
    stream.set_read_timeout(Some(WAIT)).unwrap();
    BufReader::new(stream).lines().map_while(Result::ok).collect()
}

#[test]
fn a_client_that_stops_reading_is_dropped_and_the_game_goes_on() {
    let config = ServerConfig {
        send_queue: 8,
        write_timeout: Duration::from_millis(500),
        // Not reading also means not answering pings; rule that out here so
        // it is the backlog that gets the watcher dropped.
        idle_timeout: Duration::from_secs(600),
        ..ServerConfig::default()
    };
    let server = host(8, config);
    let mut ana = GameClient::join(server.addr().to_string(), "Ana");
    assert!(ana.wait_until(WAIT, |s| s.welcomed));
    // A watcher that asks for the biggest messages and never reads them.
    let stuck = raw(&server, &[ClientMsg::Spectate { version: PROTOCOL_VERSION }, ClientMsg::Options { full_fights: true }]);
    eventually(&server, "the watcher to connect", |s| s.connections == 2);
    ana.start();

    let mut readied = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    while server.stats().dropped_slow == 0 {
        ana.update();
        act(&mut ana, &mut readied);
        assert!(!matches!(ana.state.view.as_ref().map(|v| v.phase), Some(ViewPhase::Over { .. })), "the game ended first");
        assert!(Instant::now() < deadline, "the stuck watcher was never dropped: {:?}", server.stats());
        std::thread::sleep(Duration::from_millis(1));
    }
    // Ana never stalled: she is still connected and the rounds kept coming.
    let round = ana.state.view.as_ref().unwrap().round_index;
    play_rounds(&mut ana, round + 2);
    assert!(ana.is_connected());
    drop(stuck);
    eventually(&server, "the watcher to be gone", |s| s.connections == 1);
}

#[test]
fn silent_connections_are_dropped_but_idle_clients_are_not() {
    let config = ServerConfig {
        idle_timeout: Duration::from_millis(600),
        ping_interval: Duration::from_millis(150),
        ..ServerConfig::default()
    };
    let server = host(2, config);
    // A real client sitting in the lobby doing nothing answers pings.
    let mut ana = GameClient::join(server.addr().to_string(), "Ana");
    assert!(ana.wait_until(WAIT, |s| s.welcomed));
    // A socket that connects and never says anything.
    let silent = raw(&server, &[]);

    let lines = read_to_close(silent);
    assert!(lines.iter().any(|l| l.contains("Ping")), "it was pinged first: {lines:?}");
    assert!(lines.last().unwrap().contains("nothing was heard"), "{lines:?}");
    assert_eq!(server.stats().dropped_idle, 1);

    // Well past the timeout, the real client is still there.
    assert!(!ana.wait_until(Duration::from_millis(1500), |_| false));
    assert!(ana.is_connected());
    assert_eq!(server.stats().dropped_idle, 1);
    assert_eq!(server.stats().connections, 1);
}

#[test]
fn floods_and_oversized_lines_are_cut_off() {
    let config = ServerConfig { burst: 20, messages_per_second: 5, max_line_bytes: 1024, ..ServerConfig::default() };
    let server = host(2, config);

    let mut flood = raw(&server, &[ClientMsg::Spectate { version: PROTOCOL_VERSION }]);
    let many = format!("{}\n", encode(&ClientMsg::Start)).repeat(200);
    let _ = flood.write_all(many.as_bytes());
    let lines = read_to_close(flood);
    assert!(lines.last().unwrap().contains("too many messages"), "{:?}", lines.last());
    assert_eq!(server.stats().dropped_flood, 1);

    let mut big = raw(&server, &[]);
    let _ = big.write_all(format!("{}\n", "x".repeat(5000)).as_bytes());
    let lines = read_to_close(big);
    assert!(lines.last().unwrap().contains("too long"), "{lines:?}");
    assert_eq!(server.stats().dropped_flood, 2);

    // Nonsense that fits on a line gets an answer and keeps the connection.
    let mut odd = raw(&server, &[]);
    odd.write_all(b"this is not json\n").unwrap();
    let mut reader = BufReader::new(odd.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("could not read that message"));
    odd.write_all(format!("{}\n", encode(&ClientMsg::Ping)).as_bytes()).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("Pong"), "{line}");

    // The server is still serving.
    let mut ana = GameClient::join(server.addr().to_string(), "Ana");
    assert!(ana.wait_until(WAIT, |s| s.welcomed));
}

#[test]
fn a_full_server_turns_connections_away_politely() {
    let server = host(2, ServerConfig { max_connections: 1, ..ServerConfig::default() });
    let mut ana = GameClient::join(server.addr().to_string(), "Ana");
    assert!(ana.wait_until(WAIT, |s| s.welcomed));
    let lines = read_to_close(raw(&server, &[]));
    assert!(lines[0].contains("the server is full"), "{lines:?}");
    assert_eq!(server.stats().refused, 1);
    // A place opens up when someone leaves.
    drop(ana);
    eventually(&server, "Ana to leave", |s| s.connections == 0);
    let mut ben = GameClient::join(server.addr().to_string(), "Ben");
    assert!(ben.wait_until(WAIT, |s| s.welcomed));
}

#[test]
fn a_connection_notices_a_server_that_has_gone_silent() {
    // Something that accepts connections and then never says a word.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let held = std::thread::spawn(move || listener.accept().map(|(stream, _)| stream));
    let mut conn = Connection::connect(addr).unwrap();
    conn.set_quiet_time(Duration::from_millis(100));
    let start = Instant::now();
    while !conn.is_closed() {
        conn.poll();
        assert!(start.elapsed() < Duration::from_secs(5), "never gave up");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(start.elapsed() >= Duration::from_millis(250), "it waited through its pings first");
    drop(held.join());

    // And nothing listening at all is an error straight away, not a hang.
    let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    assert!(Connection::connect(free).is_err());
    let mut nobody = GameClient::join(free.to_string(), "Ana");
    let deadline = Instant::now() + WAIT;
    while !matches!(nobody.status(), Status::Reconnecting { attempts, .. } if *attempts >= 2) {
        nobody.update();
        assert!(Instant::now() < deadline, "{:?}", nobody.status());
        std::thread::sleep(Duration::from_millis(10));
    }
}
