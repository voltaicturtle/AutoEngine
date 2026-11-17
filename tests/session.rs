use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use autoengine::content::ContentSet;
use autoengine::game::{Command, GameEvent, Opponent, PlayerId, Slot};
use autoengine::net::{Connection, serve};
use autoengine::protocol::{ClientMsg, GameView, PROTOCOL_VERSION, ServerMsg, ViewPhase, decode, encode};
use autoengine::session::{ConnId, Session, SessionConfig};

fn content() -> Arc<ContentSet> {
    Arc::new(autoengine::sample::content())
}

fn session(seats: usize) -> Session {
    Session::new(content(), SessionConfig { seats, seed: 7, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 })
}

fn join(name: &str) -> ClientMsg {
    ClientMsg::Join { name: name.into(), version: PROTOCOL_VERSION, token: None }
}

/// Messages queued for one connection.
fn inbox(s: &mut Session, conn: ConnId) -> Vec<ServerMsg> {
    s.drain_outbox().into_iter().filter(|(c, _)| *c == conn).map(|(_, m)| m).collect()
}

fn inbox_of(all: &[(ConnId, ServerMsg)], conn: ConnId) -> Vec<ServerMsg> {
    all.iter().filter(|(c, _)| *c == conn).map(|(_, m)| m.clone()).collect()
}

/// The view a client would hold after these messages: the last whole
/// state with every later update applied.
fn last_state(messages: &[ServerMsg]) -> Option<GameView> {
    let mut view: Option<GameView> = None;
    for m in messages {
        match m {
            ServerMsg::State(v) => view = Some(v.clone()),
            ServerMsg::Update(u) => {
                // Tests that threw the first state away still get the header.
                let base = view.get_or_insert_with(|| GameView {
                    phase: u.phase,
                    round_index: 0,
                    round_name: String::new(),
                    round_kind: String::new(),
                    stage: 0,
                    you: None,
                    players: Vec::new(),
                    awaiting: Vec::new(),
                    options: Vec::new(),
                    status: Vec::new(),
                    paused: false,
                    pool: Default::default(),
                    round_tiles: Vec::new(),
                });
                base.patch(u.clone());
            }
            _ => {}
        }
    }
    view
}

fn is_fight(message: &ServerMsg) -> bool {
    matches!(message, ServerMsg::Combat(_) | ServerMsg::CombatSeed { .. })
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
    assert!(all.iter().any(is_fight));
    assert!(all.iter().any(|m| matches!(m, ServerMsg::Update(_))));
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
    s.handle(9, ClientMsg::Join { name: "Old".into(), version: 0, token: None }, 0);
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
    assert_eq!(msgs.iter().filter(|m| is_fight(m)).count(), 1, "your own fight only");
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
    let ben_token = inbox_of(&s.drain_outbox(), 2)
        .into_iter()
        .find_map(|m| if let ServerMsg::Welcome { token, .. } = m { token } else { None })
        .expect("seated players get a token");
    s.handle(1, ClientMsg::Command(Command::Ready), 0);
    assert_eq!(s.game().unwrap().round_index(), 0, "still waiting for Ben");

    s.disconnect(2, 10);
    assert!(s.seat_is_bot(1));
    assert_eq!(s.game().unwrap().round_index(), 1, "the bot readied up for Ben");
    s.tick(11);
    inbox(&mut s, 1);

    // Knowing the name is not enough to take the seat; strangers just watch.
    for (conn, token) in [(3, None), (5, Some("not-the-token".to_string()))] {
        s.handle(conn, ClientMsg::Join { name: "Ben".into(), version: PROTOCOL_VERSION, token }, 20);
        let msgs = inbox(&mut s, conn);
        assert!(matches!(&msgs[0], ServerMsg::Welcome { seat: None, token: None, .. }));
        assert_eq!(last_state(&msgs).unwrap().you, None);
    }
    s.handle(3, ClientMsg::Command(Command::Reroll), 20);
    assert_eq!(errors(&inbox(&mut s, 3)), ["spectators cannot act"]);
    assert!(s.seat_is_bot(1));

    // Ben comes back with his token and gets the seat and the state.
    s.handle(4, ClientMsg::Join { name: "Ben".into(), version: PROTOCOL_VERSION, token: Some(ben_token.clone()) }, 30);
    let msgs = inbox(&mut s, 4);
    assert!(matches!(&msgs[0], ServerMsg::Welcome { seat: Some(PlayerId(1)), token: Some(t), .. } if *t == ben_token));
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
    assert_eq!(watch.iter().filter(|m| is_fight(m)).count(), 4, "one neutral fight per player");
    for m in &watch {
        if let ServerMsg::State(v) = m {
            assert!(v.players.iter().all(|p| p.shop.is_empty()));
        }
        if let ServerMsg::Update(u) = m {
            assert!(u.players.iter().all(|p| p.shop.is_empty()));
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
        let message = ana.recv_timeout(wait).expect("the server keeps talking");
        let header = match &message {
            ServerMsg::State(view) => Some((view.phase, view.round_index)),
            ServerMsg::Update(update) => Some((update.phase, update.round_index)),
            _ => None,
        };
        if let Some((ViewPhase::Planning { .. }, round)) = header {
            rounds_seen.insert(round);
            if readied_for != Some(round) {
                readied_for = Some(round);
                ana.send(&ClientMsg::Command(Command::Ready)).unwrap();
            }
        }
        if let ServerMsg::CombatSeed { record, .. } = message {
            // Which of the two is at home is up to the dice.
            assert!(record.home == PlayerId(0) || record.away == Opponent::Player(PlayerId(0)));
            assert!(record.events.is_empty(), "the log is left out; the client fights it again");
            fights += 1;
        }
    }
    assert!(fights >= 3);

    drop(ana);
    // Give the server a moment to see the closed socket before stopping it.
    std::thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::Relaxed);
    let session = server.join().unwrap();
    assert!(session.has_started());
    assert!(session.seat_is_bot(0), "the server noticed the client leaving");
}

#[test]
fn recorded_fights_can_be_loaded_and_verified() {
    use autoengine::replay::{ReplayError, ReplayFile};
    let dir = std::env::temp_dir().join(format!("autoengine-replays-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = session(2);
    s.record_to(&dir);
    s.handle(1, ClientMsg::Join { name: "Ana".into(), version: PROTOCOL_VERSION, token: None }, 0);
    s.handle(1, ClientMsg::Start, 0);
    let mut now = 0;
    while !s.is_over() && now < 4_000_000 {
        now += 30_000;
        s.tick(now);
        s.drain_outbox();
    }
    // The whole match is saved next to the fights, and replays to the end.
    let saved = Session::load_match(&dir).unwrap();
    let game = saved.log.replay(content(), |_| {}).unwrap();
    assert!(game.is_over());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| *p != Session::match_file(&dir))
        .collect();
    files.sort();
    assert!(files.len() >= 2, "every fight is saved");
    for file in &files {
        let replay = ReplayFile::load(file).unwrap();
        assert!(!replay.title.is_empty());
        assert_eq!(replay.verify(), Ok(()), "{}", file.display());
    }
    // A tampered replay is caught.
    let mut replay = ReplayFile::load(&files[0]).unwrap();
    replay.record.events.pop();
    assert!(matches!(replay.verify(), Err(ReplayError::Diverged { .. })));
    replay.record.teams.clear();
    assert_eq!(replay.verify(), Err(ReplayError::NoTeams));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn developer_commands_need_the_server_to_allow_them() {
    use autoengine::protocol::DevCommand;
    assert_eq!(DevCommand::parse("gold 50"), Ok(DevCommand::Gold(50)));
    assert_eq!(DevCommand::parse("  unit ashen_hound "), Ok(DevCommand::Unit("ashen_hound".into())));
    assert_eq!(DevCommand::parse("skip"), Ok(DevCommand::Skip));
    assert!(DevCommand::parse("gold lots").is_err() && DevCommand::parse("fly").is_err() && DevCommand::parse("").is_err());

    let errors = |s: &mut Session| s.drain_outbox().into_iter().filter(|(_, m)| matches!(m, ServerMsg::Error { .. })).count();
    let mut s = session(2);
    s.handle(1, ClientMsg::Join { name: "Ana".into(), version: PROTOCOL_VERSION, token: None }, 0);
    s.handle(1, ClientMsg::Start, 0);
    s.drain_outbox();
    let gold = s.game().unwrap().players()[0].gold;
    s.handle(1, ClientMsg::Dev(DevCommand::Gold(50)), 1);
    assert_eq!(errors(&mut s), 1, "off by default");
    assert_eq!(s.game().unwrap().players()[0].gold, gold);

    s.allow_dev(true);
    s.handle(1, ClientMsg::Dev(DevCommand::Gold(50)), 2);
    s.handle(1, ClientMsg::Dev(DevCommand::Unit("ashen_hound".into())), 3);
    assert_eq!(errors(&mut s), 0);
    let me = &s.game().unwrap().players()[0];
    assert_eq!(me.gold, gold + 50);
    assert!(me.units().any(|u| u.unit == "ashen_hound"));
    s.handle(1, ClientMsg::Dev(DevCommand::Item("no_such_item".into())), 4);
    assert_eq!(errors(&mut s), 1);
    let round = s.game().unwrap().round_index();
    s.handle(1, ClientMsg::Dev(DevCommand::Skip), 5);
    assert!(s.game().unwrap().round_index() > round || !s.game().unwrap().in_round());
}

#[test]
fn chat_reaches_everyone_and_late_joiners_see_recent_lines() {
    let chats = |messages: &[ServerMsg]| -> Vec<(String, String)> {
        messages
            .iter()
            .filter_map(|m| match m {
                ServerMsg::Chat { from, text, .. } => Some((from.clone(), text.clone())),
                _ => None,
            })
            .collect()
    };
    let mut s = session(3);
    s.handle(1, join("Ana"), 0);
    s.handle(2, join("Bo"), 0);
    s.drain_outbox();
    // Strangers cannot talk; text is tidied and empty lines are dropped.
    s.handle(9, ClientMsg::Chat { text: "hi".into() }, 0);
    assert_eq!(errors(&inbox(&mut s, 9)).len(), 1);
    s.handle(1, ClientMsg::Chat { text: "  good\n\tluck  ".into() }, 0);
    s.handle(1, ClientMsg::Chat { text: " \n ".into() }, 0);
    s.handle(2, ClientMsg::Chat { text: "x".repeat(500) }, 0);
    let all = s.drain_outbox();
    let to_ana = chats(&inbox_of(&all, 1));
    assert_eq!(to_ana.len(), 2);
    assert_eq!(to_ana[0], ("Ana".to_string(), "good luck".to_string()));
    assert_eq!(to_ana[1].1.len(), autoengine::protocol::MAX_CHAT_CHARS);
    assert_eq!(chats(&inbox_of(&all, 2)), to_ana);

    // A spectator gets the history and can talk too; so can the server.
    s.handle(3, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    assert_eq!(chats(&inbox(&mut s, 3)), to_ana);
    s.handle(3, ClientMsg::Chat { text: "go Bo".into() }, 0);
    s.announce("restarting soon");
    let heard = chats(&inbox(&mut s, 1));
    assert_eq!(heard, vec![("spectator".into(), "go Bo".into()), ("server".into(), "restarting soon".into())]);

    s.allow_chat(false);
    s.handle(1, ClientMsg::Chat { text: "anyone?".into() }, 0);
    let messages = inbox(&mut s, 1);
    assert!(chats(&messages).is_empty() && errors(&messages)[0].contains("off"));

    // The client state shows chat in its log.
    let mut state = autoengine::client::ClientState::new();
    state.apply(ServerMsg::Chat { from: "Ana".into(), seat: Some(PlayerId(0)), text: "hello".into() });
    assert_eq!(state.chat.back(), Some(&("Ana".to_string(), "hello".to_string())));
    assert_eq!(state.log.back().map(String::as_str), Some("Ana: hello"));
    state.say("  ");
    state.say("hey");
    assert_eq!(state.requests, vec![ClientMsg::Chat { text: "hey".into() }]);
}

#[test]
fn spectators_can_be_shown_the_game_late() {
    let mut s = session(2);
    s.set_spectator_delay(10_000);
    s.handle(1, join("Ana"), 0);
    s.handle(2, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    s.drain_outbox();
    s.handle(1, ClientMsg::Start, 1_000);
    s.handle(1, ClientMsg::Chat { text: "hi".into() }, 1_000);
    let now = s.drain_outbox();
    assert!(last_state(&inbox_of(&now, 1)).is_some(), "the player sees the game at once");
    let watcher = inbox_of(&now, 2);
    assert!(last_state(&watcher).is_none(), "the spectator does not, yet");
    assert!(watcher.iter().any(|m| matches!(m, ServerMsg::Chat { .. })), "chat is not delayed");
    assert!(watcher.iter().any(|m| matches!(m, ServerMsg::Lobby { .. })));
    s.tick(10_999);
    assert!(last_state(&inbox(&mut s, 2)).is_none());
    s.tick(11_000);
    let late = inbox(&mut s, 2);
    let view = last_state(&late).expect("ten seconds later");
    assert_eq!(view.round_index, 0);
}

#[test]
fn rounds_of_a_kind_can_have_their_own_planning_time() {
    use autoengine::content::{RoundDef, RoundKindDef, Schedule};
    let pvp = RoundDef { name: "fight".into(), stage: 1, kind: RoundKindDef::Pvp, tiles: Vec::new() };
    let content = autoengine::sample::content().with_schedule(Schedule { rounds: vec![pvp], repeat_from: 0 }).unwrap();
    let mut rules = content.rules().clone();
    rules.planning_ms_by_kind.insert("pvp".into(), 5_000);
    let content = content.with_game_rules(rules).unwrap();
    let mut s = Session::new(content, SessionConfig { seats: 2, seed: 7, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 });
    s.handle(1, join("Ana"), 0);
    s.handle(1, ClientMsg::Start, 0);
    let view = last_state(&inbox(&mut s, 1)).unwrap();
    assert_eq!(view.phase, ViewPhase::Planning { remaining_ms: 5_000 });
    s.tick(4_999);
    assert_eq!(s.game().unwrap().round_index(), 0);
    s.tick(5_000);
    s.tick(5_001);
    assert_eq!(s.game().unwrap().round_index(), 1);
}

#[test]
fn the_host_can_kick_a_player() {
    let mut s = session(3);
    s.handle(1, join("Ana"), 0);
    s.handle(2, join("Troll"), 0);
    // Before the start the seat opens again, and the name stays out.
    assert!(s.kick("Troll", 0));
    assert!(!s.kick("Nobody", 0));
    assert_eq!(s.take_closed(), vec![2]);
    assert_eq!(s.seat_names()[1], None);
    s.drain_outbox();
    s.handle(3, join("Troll"), 0);
    assert_eq!(errors(&inbox(&mut s, 3)), vec!["you were removed from this game"]);

    // During the game a bot keeps the seat, and the token stops working.
    s.handle(4, join("Bo"), 0);
    let token = inbox(&mut s, 4).iter().find_map(|m| match m {
        ServerMsg::Welcome { token, .. } => token.clone(),
        _ => None,
    });
    s.handle(1, ClientMsg::Start, 0);
    assert!(s.kick("Bo", 10));
    assert!(s.seat_is_bot(1));
    s.drain_outbox();
    s.handle(5, ClientMsg::Join { name: "Bo".into(), version: PROTOCOL_VERSION, token }, 20);
    assert_eq!(errors(&inbox(&mut s, 5)), vec!["you were removed from this game"]);
}

#[test]
fn chat_is_rate_limited_filtered_and_mutable() {
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    s.handle(2, join("Bo"), 0);
    s.set_chat_filter(|text| if text.contains("spoiler") { None } else { Some(text.replace("darn", "d**n")) });
    s.drain_outbox();
    let said = |s: &mut Session, conn, text: &str, now| {
        s.handle(conn, ClientMsg::Chat { text: text.into() }, now);
        s.drain_outbox()
    };
    let heard = |out: &[(ConnId, ServerMsg)]| -> Vec<String> {
        inbox_of(out, 2).into_iter().filter_map(|m| match m { ServerMsg::Chat { text, .. } => Some(text), _ => None }).collect()
    };
    assert_eq!(heard(&said(&mut s, 1, "oh darn", 0)), vec!["oh d**n"]);
    assert!(heard(&said(&mut s, 1, "big spoiler here", 0)).is_empty());
    for i in 0..3 {
        assert_eq!(heard(&said(&mut s, 1, &format!("line {i}"), 100)).len(), 1);
    }
    // That is five lines in ten seconds; the sixth is refused.
    let out = said(&mut s, 1, "one more", 200);
    assert!(heard(&out).is_empty());
    assert!(errors(&inbox_of(&out, 1))[0].contains("too fast"));
    assert_eq!(heard(&said(&mut s, 1, "later", 10_001)).len(), 1);

    let mut state = autoengine::client::ClientState::new();
    state.muted.insert("Troll".into());
    state.apply(ServerMsg::Chat { from: "Troll".into(), seat: None, text: "spam".into() });
    state.apply(ServerMsg::Chat { from: "Ana".into(), seat: None, text: "hi".into() });
    assert_eq!(state.chat.len(), 1);
}

#[test]
fn the_host_can_pause_the_clock() {
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    assert!(!s.pause(0), "nothing to pause before the start");
    s.handle(1, ClientMsg::Start, 0);
    s.drain_outbox();
    assert!(s.pause(10_000));
    assert!(!s.pause(10_000));
    let view = last_state(&inbox(&mut s, 1)).unwrap();
    assert!(view.paused);
    assert_eq!(view.phase, ViewPhase::Planning { remaining_ms: 20_000 });
    // Long past the deadline, and everyone ready: still waiting.
    s.handle(1, ClientMsg::Command(Command::Ready), 50_000);
    s.tick(100_000);
    assert_eq!(s.game().unwrap().round_index(), 0);
    assert!(s.game().unwrap().in_round());
    // Players can still act while paused.
    assert!(errors(&inbox(&mut s, 1)).is_empty());
    // Unpausing with everyone ready ends the round at once.
    assert!(s.unpause(100_000));
    assert!(!s.game().unwrap().in_round() || s.game().unwrap().round_index() > 0);

    // Unpaused with time left, the deadline moves by the pause.
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    s.handle(1, ClientMsg::Start, 0);
    s.pause(10_000);
    s.unpause(70_000);
    s.tick(89_999);
    assert_eq!(s.game().unwrap().round_index(), 0);
    s.tick(90_000);
    s.tick(90_001);
    assert_eq!(s.game().unwrap().round_index(), 1);
}

#[test]
fn clients_see_how_many_copies_are_left() {
    let mut s = session(2);
    s.handle(1, join("Ana"), 0);
    s.handle(1, ClientMsg::Start, 0);
    let messages = inbox(&mut s, 1);
    let view = last_state(&messages).unwrap();
    let game = s.game().unwrap();
    assert_eq!(&view.pool, game.pool());
    assert!(!view.pool.is_empty());
    // Buying changes nothing in the pool (the shop held the copy), but a
    // reroll puts copies back and takes others: the update carries it.
    s.handle(1, ClientMsg::Command(Command::Reroll), 0);
    let mut messages = messages;
    messages.extend(inbox(&mut s, 1));
    let view = last_state(&messages).unwrap();
    assert_eq!(&view.pool, s.game().unwrap().pool());
}
