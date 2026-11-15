//! Many games on one server.

use std::sync::Arc;
use std::time::Duration;

use autoengine::content::ContentSet;
use autoengine::game::PlayerId;
use autoengine::hub::{Hub, HubConfig};
use autoengine::net::{GameClient, Host, Server};
use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, RoomInfo, ServerMsg};
use autoengine::session::{ConnId, SessionConfig};

fn content() -> Arc<ContentSet> {
    Arc::new(autoengine::sample::content())
}

fn hub(seats: usize) -> Hub {
    let room = SessionConfig { seats, seed: 3, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 };
    Hub::new(content(), HubConfig { room, close_after_ms: 1_000, ..HubConfig::default() })
}

fn join(name: &str) -> ClientMsg {
    ClientMsg::Join { name: name.into(), version: PROTOCOL_VERSION, token: None }
}

fn inbox(all: &[(ConnId, ServerMsg)], conn: ConnId) -> Vec<ServerMsg> {
    all.iter().filter(|(c, _)| *c == conn).map(|(_, m)| m.clone()).collect()
}

fn seat(messages: &[ServerMsg]) -> Option<Option<PlayerId>> {
    messages.iter().find_map(|m| match m {
        ServerMsg::Welcome { seat, .. } => Some(*seat),
        _ => None,
    })
}

fn room_of(messages: &[ServerMsg]) -> Option<Option<u32>> {
    messages.iter().rev().find_map(|m| match m {
        ServerMsg::InRoom { room } => Some(*room),
        _ => None,
    })
}

fn rooms(h: &mut Hub, conn: ConnId) -> Vec<RoomInfo> {
    h.handle(conn, ClientMsg::ListRooms, 0);
    inbox(&h.drain_outbox(), conn)
        .into_iter()
        .find_map(|m| match m {
            ServerMsg::Rooms(r) => Some(r),
            _ => None,
        })
        .unwrap()
}

#[test]
fn joining_fills_open_games_then_opens_new_ones() {
    let mut h = hub(2);
    for (conn, name) in [(1, "Ana"), (2, "Bo"), (3, "Cy")] {
        h.handle(conn, join(name), 0);
    }
    let out = h.drain_outbox();
    assert_eq!(room_of(&inbox(&out, 1)), Some(Some(1)));
    assert_eq!(room_of(&inbox(&out, 2)), Some(Some(1)));
    assert_eq!(room_of(&inbox(&out, 3)), Some(Some(2)), "the first game was full");
    assert_eq!(seat(&inbox(&out, 3)), Some(Some(PlayerId(0))));
    let list = rooms(&mut h, 9);
    assert_eq!(list.len(), 2);
    assert!(list[0].started, "a full game starts by itself");
    assert_eq!((list[1].players, list[1].started), (1, false));
}

#[test]
fn rooms_are_separate_games() {
    let mut h = hub(4);
    h.handle(1, ClientMsg::CreateRoom { name: "  Friday\n night ".into(), seats: Some(3) }, 0);
    h.handle(2, ClientMsg::CreateRoom { name: "".into(), seats: Some(99) }, 0);
    let out = h.drain_outbox();
    assert_eq!(room_of(&inbox(&out, 1)), Some(Some(1)));
    let list = rooms(&mut h, 9);
    assert_eq!(list[0].name, "Friday night");
    assert_eq!(list[0].seats, 3);
    assert_eq!(list[1].name, "Game 2");
    assert_eq!(list[1].seats, 8, "capped at the most seats allowed");

    // Chat and game state stay in their own room.
    h.handle(1, join("Ana"), 0);
    h.handle(2, join("Bo"), 0);
    h.handle(1, ClientMsg::Chat { text: "hello room one".into() }, 0);
    let out = h.drain_outbox();
    let chat = |m: &ServerMsg| matches!(m, ServerMsg::Chat { .. });
    assert!(inbox(&out, 1).iter().any(chat));
    assert!(!inbox(&out, 2).iter().any(chat));
    h.handle(1, ClientMsg::Start, 0);
    assert!(h.room(1).unwrap().has_started());
    assert!(!h.room(2).unwrap().has_started());

    // A spectator can enter a running game and watch it.
    h.handle(3, ClientMsg::EnterRoom { room: 1 }, 0);
    h.handle(3, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    let out = h.drain_outbox();
    assert_eq!(seat(&inbox(&out, 3)), Some(None));
    assert!(inbox(&out, 3).iter().any(|m| matches!(m, ServerMsg::State(_))));

    // Unknown rooms and messages outside a room are refused.
    h.handle(4, ClientMsg::EnterRoom { room: 77 }, 0);
    h.handle(4, ClientMsg::Start, 0);
    let errors = inbox(&h.drain_outbox(), 4).iter().filter(|m| matches!(m, ServerMsg::Error { .. })).count();
    assert_eq!(errors, 2);
}

#[test]
fn leaving_and_closing_rooms() {
    let mut h = hub(2);
    h.handle(1, ClientMsg::CreateRoom { name: "a".into(), seats: None }, 0);
    h.handle(1, join("Ana"), 0);
    h.handle(1, ClientMsg::LeaveRoom, 0);
    assert_eq!(room_of(&inbox(&h.drain_outbox(), 1)), Some(None));
    assert!(h.room(1).unwrap().has_open_seat(), "the seat opened again before the start");
    // Empty for long enough: closed.
    h.tick(10);
    h.tick(500);
    assert_eq!(h.rooms().len(), 1);
    h.tick(1_100);
    assert!(h.rooms().is_empty());

    // Rooms can be limited, or left to the server to open.
    let mut h = Hub::new(content(), HubConfig { max_rooms: 1, allow_create: false, ..HubConfig::default() });
    h.handle(1, ClientMsg::CreateRoom { name: "x".into(), seats: None }, 0);
    h.handle(1, join("Ana"), 0);
    h.handle(1, ClientMsg::LeaveRoom, 0);
    let mut h2 = Hub::new(content(), HubConfig { max_rooms: 1, ..HubConfig::default() });
    h2.handle(1, ClientMsg::CreateRoom { name: "x".into(), seats: Some(1) }, 0);
    h2.handle(1, join("Ana"), 0);
    h2.handle(2, join("Bo"), 0);
    let errors = |out: Vec<ServerMsg>| out.iter().filter(|m| matches!(m, ServerMsg::Error { .. })).count();
    assert_eq!(errors(inbox(&h.drain_outbox(), 1)), 1);
    assert_eq!(errors(inbox(&h2.drain_outbox(), 2)), 1, "no room left for a second game");
}

#[test]
fn rooms_get_their_own_setup_and_seed() {
    let mut h = hub(2);
    h.on_new_room(|id, session| {
        session.allow_dev(id == 2);
    });
    h.handle(1, ClientMsg::CreateRoom { name: "a".into(), seats: Some(1) }, 0);
    h.handle(2, ClientMsg::CreateRoom { name: "b".into(), seats: Some(1) }, 0);
    h.handle(1, join("Ana"), 0);
    h.handle(2, join("Bo"), 0);
    h.drain_outbox();
    for conn in [1, 2] {
        h.handle(conn, ClientMsg::Dev(autoengine::protocol::DevCommand::Gold(5)), 0);
    }
    let out = h.drain_outbox();
    assert!(inbox(&out, 1).iter().any(|m| matches!(m, ServerMsg::Error { .. })));
    assert!(!inbox(&out, 2).iter().any(|m| matches!(m, ServerMsg::Error { .. })));
    assert_ne!(h.room(1).unwrap().game().unwrap().seed(), h.room(2).unwrap().game().unwrap().seed());
}

#[test]
fn a_client_reconnects_into_its_own_room() {
    let server = Server::bind("127.0.0.1:0").unwrap().spawn(hub(3));
    let addr = server.addr().to_string();
    let wait = Duration::from_secs(20);
    // Fill a first room so the next player gets a room of their own.
    let mut filler = GameClient::join(addr.clone(), "Filler");
    assert!(filler.wait_until(wait, |s| s.welcomed));
    assert_eq!(filler.state.room, Some(1));
    let mut c = GameClient::join(addr.clone(), "Ana").in_room(1);
    assert!(c.wait_until(wait, |s| s.seat.is_some()));
    assert_eq!(c.state.room, Some(1));
    let seat = c.state.seat;
    c.reconnect(None);
    assert!(c.wait_until(wait, |s| s.welcomed && s.seat.is_some()));
    assert_eq!((c.state.room, c.state.seat), (Some(1), seat), "same room, same seat");
    c.state.requests.push(ClientMsg::ListRooms);
    assert!(c.wait_until(wait, |s| !s.rooms.is_empty()));
    assert_eq!(c.state.rooms[0].players, 2);
    let hub = server.stop().unwrap();
    assert_eq!(hub.rooms().len(), 1);
}

#[test]
fn admin_jobs_run_on_the_live_server_and_kicks_disconnect() {
    let server = Server::bind("127.0.0.1:0").unwrap().spawn(hub(3));
    let addr = server.addr().to_string();
    let wait = Duration::from_secs(20);
    let mut ana = GameClient::join(addr.clone(), "Ana");
    let mut troll = GameClient::join(addr, "Troll");
    assert!(ana.wait_until(wait, |s| s.welcomed) && troll.wait_until(wait, |s| s.welcomed));
    let rooms = server.with_host(|h: &mut Hub, _| h.rooms()).unwrap();
    assert_eq!(rooms[0].players, 2);
    server.with_host(|h: &mut Hub, _| h.announce("welcome"));
    assert!(ana.wait_until(wait, |s| s.chat.iter().any(|(from, text)| from == "server" && text == "welcome")));
    assert_eq!(server.with_host(|h: &mut Hub, now| h.kick("Troll", now)), Some(1));
    assert!(troll.wait_until(wait, |s| s.error.as_deref() == Some("you were removed from the game by the host")));
    let deadline = std::time::Instant::now() + wait;
    while server.with_host(|h: &mut Hub, _| h.rooms()[0].players).unwrap() != 1 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(troll);
    let hub = server.stop().unwrap();
    assert_eq!(hub.room(1).unwrap().seat_names()[1], None);
}

#[test]
fn a_hub_resumes_its_unfinished_games() {
    let dir = std::env::temp_dir().join(format!("autoengine-hub-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let room = SessionConfig { seats: 2, seed: 4, planning_ms: 1_000, wait_for_playback: false, after_combat_ms: 0 };
    let make = || Hub::new(content(), HubConfig { room: room.clone(), ..HubConfig::default() });
    let mut first = make();
    first.record_to(&dir);
    first.handle(1, ClientMsg::CreateRoom { name: "a".into(), seats: None }, 0);
    first.handle(1, join("Ana"), 0);
    first.handle(1, ClientMsg::Start, 0);
    first.handle(2, ClientMsg::CreateRoom { name: "b".into(), seats: None }, 0);
    first.handle(2, join("Bo"), 0);
    first.handle(2, ClientMsg::Start, 0);
    let token = first.drain_outbox().into_iter().find_map(|(c, m)| match m {
        ServerMsg::Welcome { token, .. } if c == 1 => token,
        _ => None,
    });
    for t in 1..=3 {
        first.tick(t * 1_000);
    }
    let checksums: Vec<u64> = [1, 2].iter().map(|id| first.room(*id).unwrap().game().unwrap().checksum()).collect();
    drop(first);

    let mut second = make();
    assert_eq!(second.resume(&dir, 0).unwrap(), 2);
    assert_eq!(second.rooms().iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2]);
    for (i, id) in [1, 2].iter().enumerate() {
        assert_eq!(second.room(*id).unwrap().game().unwrap().checksum(), checksums[i]);
    }
    // Ana comes back to her room and seat.
    second.handle(5, ClientMsg::EnterRoom { room: 1 }, 10);
    second.handle(5, ClientMsg::Join { name: "Ana".into(), version: PROTOCOL_VERSION, token }, 10);
    assert_eq!(seat(&inbox(&second.drain_outbox(), 5)), Some(Some(PlayerId(0))));
    // New rooms do not reuse old numbers.
    second.handle(6, ClientMsg::CreateRoom { name: "c".into(), seats: None }, 10);
    assert_eq!(room_of(&inbox(&second.drain_outbox(), 6)), Some(Some(3)));
    assert_eq!(make().resume(&dir.join("nowhere"), 0).unwrap(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}
