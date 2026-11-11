use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use autoengine::content::ContentSet;
use autoengine::game::{Command, GameEvent, PlayerId, Slot};
use autoengine::net::{Connection, serve};
use autoengine::protocol::{ClientMsg, GameView, PROTOCOL_VERSION, ServerMsg, ViewPhase, decode, encode};
use autoengine::session::{ConnId, Session, SessionConfig};

fn content() -> Arc<ContentSet> {
    Arc::new(ContentSet::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/content")).unwrap())
}

fn session(seats: usize) -> Session {
    Session::new(content(), SessionConfig { seats, seed: 7, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 })
}

fn join(name: &str) -> ClientMsg {
    ClientMsg::Join { name: name.into(), version: PROTOCOL_VERSION }
}

/// Messages queued for one connection.
fn inbox(s: &mut Session, conn: ConnId) -> Vec<ServerMsg> {
    s.drain_outbox().into_iter().filter(|(c, _)| *c == conn).map(|(_, m)| m).collect()
}

fn last_state(messages: &[ServerMsg]) -> Option<&GameView> {
    messages.iter().rev().find_map(|m| match m {
        ServerMsg::State(v) => Some(v),
        _ => None,
    })
}

fn errors(messages: &[ServerMsg]) -> Vec<&str> {
    messages
        .iter()
        .filter_map(|m| match m {
            ServerMsg::Error { message } => Some(message.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn messages_survive_the_wire_format() {
    let messages = vec![
        join("Ana"),
        ClientMsg::Start,
        ClientMsg::Command(Command::MoveUnit { uid: 3, to: Slot::Board(2, 5) }),
        ClientMsg::Command(Command::Pick { option: 1 }),
    ];
    for m in messages {
        let line = encode(&m);
        assert!(!line.contains('\n'));
        assert_eq!(decode::<ClientMsg>(&line).unwrap(), m);
    }
    assert!(decode::<ClientMsg>("{\"Nonsense\":1}").is_err());

    // A whole server-side conversation, including state with a board and fights.
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    s.handle(1, ClientMsg::Start, 0);
    let mut all = inbox(&mut s, 1);
    for now in 1..200 {
        s.handle(1, ClientMsg::Command(Command::Ready), now);
        s.tick(now);
        all.extend(inbox(&mut s, 1));
        if s.is_over() {
            break;
        }
    }
    assert!(all.iter().any(|m| matches!(m, ServerMsg::Combat(_))));
    for m in &all {
        assert_eq!(&decode::<ServerMsg>(&encode(m)).unwrap(), m);
    }
}

#[test]
fn lobby_fills_and_starts_when_every_seat_is_taken() {
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    let msgs = inbox(&mut s, 1);
    assert!(matches!(&msgs[0], ServerMsg::Welcome { seat: Some(PlayerId(0)), content, .. } if content.units.len() == 9));
    assert!(matches!(&msgs[1], ServerMsg::Lobby { seats, started: false } if seats == &[Some("Ana".to_string()), None]));
    assert!(!s.has_started());

    // The content sent to clients can be rebuilt for lookups.
    let ServerMsg::Welcome { content, .. } = &msgs[0] else { unreachable!() };
    let rebuilt = content.clone().into_content().unwrap();
    assert_eq!(rebuilt.unit("ashen_hound").unwrap().name, "Ashen Hound");
    assert!(rebuilt.augment("war_chest").is_some());

    s.handle(2, join("Ben"), 0);
    assert!(s.has_started());
    let msgs = inbox(&mut s, 1);
    let view = last_state(&msgs).unwrap();
    assert_eq!(view.you, Some(PlayerId(0)));
    assert_eq!(view.round_name, "1-1");
    assert_eq!(view.round_kind, "pve");
    assert!(matches!(view.phase, ViewPhase::Planning { remaining_ms: 30_000 }));
    assert_eq!(view.players[0].shop.len(), 5, "you see your own shop");
    assert!(view.players[1].shop.is_empty(), "but not anyone else's");
    // Private events stay private.
    let shop_rolls: Vec<_> = msgs
        .iter()
        .filter_map(|m| if let ServerMsg::Events(e) = m { Some(e) } else { None })
        .flatten()
        .filter_map(|e| if let GameEvent::ShopRolled { player, .. } = e { Some(*player) } else { None })
        .collect();
    assert_eq!(shop_rolls, [PlayerId(0)]);
}

#[test]
fn start_fills_empty_seats_with_bots_and_refuses_bad_requests() {
    let mut s = session(4);
    s.handle(9, ClientMsg::Start, 0);
    assert_eq!(errors(&inbox(&mut s, 9)), ["only seated players can start the game"]);
    s.handle(9, ClientMsg::Join { name: "Old".into(), version: 0 }, 0);
    assert!(errors(&inbox(&mut s, 9))[0].contains("protocol version 0"));
    s.handle(9, join("   "), 0);
    assert_eq!(errors(&inbox(&mut s, 9)), ["a name is required"]);

    s.handle(1, join("Ana"), 0);
    s.handle(1, ClientMsg::Command(Command::Reroll), 0);
    assert_eq!(errors(&inbox(&mut s, 1)), ["no round is in progress"]);
    s.handle(1, ClientMsg::Start, 0);
    let msgs = inbox(&mut s, 1);
    let view = last_state(&msgs).unwrap();
    let names: Vec<&str> = view.players.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Ana", "Bot 2", "Bot 3", "Bot 4"]);
    assert!(s.seat_is_bot(1) && !s.seat_is_bot(0));
    // Bots are ready at once; the round waits only for the person.
    assert!(view.players[1..].iter().all(|p| p.ready));
    assert!(!view.players[0].ready);

    s.handle(1, ClientMsg::Start, 0);
    assert_eq!(errors(&inbox(&mut s, 1)), ["the game has already started"]);
    // A rejected command reports why and changes nothing.
    s.handle(1, ClientMsg::Command(Command::SellUnit { uid: 12345 }), 0);
    assert_eq!(errors(&inbox(&mut s, 1)), ["the player owns no such unit"]);
}

#[test]
fn rounds_advance_on_ready_or_when_time_runs_out() {
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    s.handle(1, ClientMsg::Start, 0);
    inbox(&mut s, 1);

    // Everyone ready: the round resolves immediately.
    s.handle(1, ClientMsg::Command(Command::Ready), 100);
    let msgs = inbox(&mut s, 1);
    assert_eq!(msgs.iter().filter(|m| matches!(m, ServerMsg::Combat(_))).count(), 1, "your own fight only");
    assert!(matches!(last_state(&msgs).unwrap().phase, ViewPhase::Showing { .. }));
    assert_eq!(s.game().unwrap().round_index(), 1);

    s.tick(101);
    let msgs = inbox(&mut s, 1);
    let view = last_state(&msgs).unwrap();
    assert!(matches!(view.phase, ViewPhase::Planning { remaining_ms: 30_000 }));
    assert_eq!(view.round_name, "1-2");

    // Nobody readies up: the deadline resolves the round.
    s.tick(30_100);
    assert_eq!(s.game().unwrap().round_index(), 1);
    s.tick(30_101);
    assert_eq!(s.game().unwrap().round_index(), 2);
}

#[test]
fn playback_time_is_reserved_after_fights() {
    let config = SessionConfig { seats: 2, seed: 7, planning_ms: 30_000, wait_for_playback: true, after_combat_ms: 2_000 };
    let mut s = Session::new(content(), config);
    s.handle(1, join("Ana"), 0);
    s.handle(1, ClientMsg::Start, 0);
    s.handle(1, ClientMsg::Command(Command::Ready), 0);
    let msgs = inbox(&mut s, 1);
    // The wait covers the longest fight of the round, not just your own.
    let longest = s.game().unwrap().combats().iter().map(|c| c.duration_ms as u64).max().unwrap();
    let ViewPhase::Showing { remaining_ms } = last_state(&msgs).unwrap().phase else { panic!("expected showing") };
    assert_eq!(remaining_ms, longest + 2_000);
    s.tick(remaining_ms - 1);
    assert_eq!(s.game().unwrap().round_name(), "1-1");
    s.tick(remaining_ms);
    assert_eq!(s.game().unwrap().round_name(), "1-2");
}

#[test]
fn a_disconnected_seat_is_played_by_a_bot_until_the_player_returns() {
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    s.handle(2, join("Ben"), 0);
    s.handle(1, ClientMsg::Command(Command::Ready), 0);
    assert_eq!(s.game().unwrap().round_index(), 0, "still waiting for Ben");

    s.disconnect(2, 10);
    assert!(s.seat_is_bot(1));
    assert_eq!(s.game().unwrap().round_index(), 1, "the bot readied up for Ben");
    s.tick(11);
    inbox(&mut s, 1);

    // A stranger cannot take the seat, but can watch.
    s.handle(3, join("Cy"), 20);
    let msgs = inbox(&mut s, 3);
    assert!(matches!(&msgs[0], ServerMsg::Welcome { seat: None, .. }));
    assert_eq!(last_state(&msgs).unwrap().you, None);
    s.handle(3, ClientMsg::Command(Command::Reroll), 20);
    assert_eq!(errors(&inbox(&mut s, 3)), ["spectators cannot act"]);

    // Ben comes back under the same name and gets the seat and the state.
    s.handle(4, join("Ben"), 30);
    let msgs = inbox(&mut s, 4);
    assert!(matches!(&msgs[0], ServerMsg::Welcome { seat: Some(PlayerId(1)), .. }));
    let view = last_state(&msgs).unwrap();
    assert_eq!(view.you, Some(PlayerId(1)));
    assert_eq!(view.players[1].shop.len(), 5);
    assert!(!s.seat_is_bot(1));
}

#[test]
fn spectators_see_every_fight_but_no_private_information() {
    let mut s = session(4);
    s.handle(1, join("Ana"), 0);
    s.handle(5, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    s.handle(1, ClientMsg::Start, 0);
    s.handle(1, ClientMsg::Command(Command::Ready), 0);
    let all = s.drain_outbox();
    let watch: Vec<&ServerMsg> = all.iter().filter(|(c, _)| *c == 5).map(|(_, m)| m).collect();
    assert_eq!(watch.iter().filter(|m| matches!(m, ServerMsg::Combat(_))).count(), 4, "one neutral fight per player");
    for m in &watch {
        if let ServerMsg::State(v) = m {
            assert!(v.players.iter().all(|p| p.shop.is_empty()));
        }
        if let ServerMsg::Events(events) = m {
            assert!(!events.iter().any(|e| matches!(e, GameEvent::ShopRolled { .. })));
        }
    }
}

#[test]
fn an_all_bot_session_plays_itself_to_the_end() {
    let mut s = session(4);
    s.handle(1, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    s.handle(2, join("Ana"), 0);
    s.handle(2, ClientMsg::Start, 0);
    s.disconnect(2, 0);
    let mut now = 0;
    while !s.is_over() {
        now += 1;
        assert!(now < 2_000, "the game should finish");
        s.tick(now);
    }
    let msgs = inbox(&mut s, 1);
    let view = last_state(&msgs).unwrap();
    let ViewPhase::Over { winner: Some(winner) } = view.phase else { panic!("expected a winner") };
    assert_eq!(view.players[winner.0 as usize].placement, Some(1));
}

#[test]
fn a_client_can_play_over_tcp() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server = {
        let stop = stop.clone();
        let session = session(2);
        std::thread::spawn(move || serve(listener, session, &stop).unwrap())
    };

    let wait = Duration::from_secs(10);
    let mut ana = Connection::connect(addr).unwrap();
    ana.send(&join("Ana")).unwrap();
    assert!(matches!(ana.recv_timeout(wait), Some(ServerMsg::Welcome { seat: Some(PlayerId(0)), .. })));
    assert!(matches!(ana.recv_timeout(wait), Some(ServerMsg::Lobby { started: false, .. })));

    // Garbage gets an error back instead of killing the connection.
    {
        use std::io::Write;
        let mut raw = std::net::TcpStream::connect(addr).unwrap();
        raw.write_all(b"this is not json\n").unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(&raw), &mut line).unwrap();
        assert!(line.contains("could not read that message"));
    }

    ana.send(&ClientMsg::Start).unwrap();
    // Wait for the first planning state, then play a few rounds by readying up.
    let mut rounds_seen = std::collections::BTreeSet::new();
    let mut fights = 0;
    let mut readied_for = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while rounds_seen.len() < 4 {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for rounds");
        match ana.recv_timeout(wait).expect("the server keeps talking") {
            ServerMsg::State(view) => {
                if matches!(view.phase, ViewPhase::Planning { .. }) {
                    rounds_seen.insert(view.round_index);
                    if readied_for != Some(view.round_index) {
                        readied_for = Some(view.round_index);
                        ana.send(&ClientMsg::Command(Command::Ready)).unwrap();
                    }
                }
            }
            ServerMsg::Combat(record) => {
                assert_eq!(record.home, PlayerId(0));
                fights += 1;
            }
            _ => {}
        }
    }
    assert!(fights >= 3);

    drop(ana);
    stop.store(true, Ordering::Relaxed);
    let session = server.join().unwrap();
    assert!(session.has_started());
    assert!(session.seat_is_bot(0), "the server noticed the client leaving");
}
