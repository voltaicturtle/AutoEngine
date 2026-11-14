//! The server decides everything. These tests are the checklist: a client
//! can only ask for things a player is allowed to do, every request is
//! checked against the server's own state, and nothing a client works out
//! is ever taken as a result.

use std::sync::Arc;

use autoengine::game::{Command, Player, PlayerId, Slot};
use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use autoengine::session::{ConnId, Session, SessionConfig};

fn session(seats: usize) -> Session {
    let content = Arc::new(autoengine::sample::content());
    Session::new(content, SessionConfig { seats, seed: 7, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 })
}

fn join(name: &str, token: Option<String>) -> ClientMsg {
    ClientMsg::Join { name: name.into(), version: PROTOCOL_VERSION, token }
}

fn inbox(s: &mut Session, conn: ConnId) -> Vec<ServerMsg> {
    s.drain_outbox().into_iter().filter(|(c, _)| *c == conn).map(|(_, m)| m).collect()
}

fn errors(messages: &[ServerMsg]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            ServerMsg::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn players(s: &Session) -> Vec<Player> {
    s.game().unwrap().players().to_vec()
}

/// Every message a client can send, and why none of them can set a result.
/// This match has no wildcard: adding a message type stops it compiling
/// until someone has decided, here, that the new message is safe.
#[test]
fn no_client_message_carries_a_result() {
    let every: Vec<ClientMsg> = vec![
        join("Ana", None),
        ClientMsg::Spectate { version: PROTOCOL_VERSION },
        ClientMsg::Start,
        ClientMsg::Command(Command::Ready),
        ClientMsg::Dev(autoengine::protocol::DevCommand::Skip),
        ClientMsg::Options { full_fights: true },
        ClientMsg::ResendCombat { id: 0 },
        ClientMsg::Ping,
        ClientMsg::Pong,
    ];
    for message in every {
        let why = match message {
            // Asks for a seat; the server picks which, and a taken seat
            // needs the secret token the server handed out.
            ClientMsg::Join { .. } => "a request for a seat",
            ClientMsg::Spectate { .. } => "a request to watch",
            // Only before the game, only from a seated player.
            ClientMsg::Start => "a request to start",
            // A player's intent (buy, sell, move, pick, ready). It names no
            // player, so it always acts on the sender's own seat, and the
            // game checks it against its own state.
            ClientMsg::Command(_) => "an intent, checked by the game",
            // Refused unless the server was started with --dev.
            ClientMsg::Dev(_) => "a cheat, off by default",
            ClientMsg::Options { .. } => "how to send, not what happened",
            ClientMsg::ResendCombat { .. } => "a request for the server's own record",
            ClientMsg::Ping | ClientMsg::Pong => "keep-alive",
        };
        assert!(!why.is_empty());
    }
}

#[test]
fn a_player_can_only_act_for_their_own_seat() {
    let mut s = session(2);
    s.handle(1, join("Ana", None), 0);
    s.handle(2, join("Ben", None), 0);
    inbox(&mut s, 1);
    let before = players(&s);
    // Ana buys and rerolls: only her gold and shop change.
    s.handle(1, ClientMsg::Command(Command::BuyUnit { slot: 0 }), 1);
    s.handle(1, ClientMsg::Command(Command::Reroll), 1);
    let after = players(&s);
    assert_ne!(after[0], before[0]);
    assert_eq!(after[1], before[1], "Ben is untouched");
    // Ben readying up readies Ben, not Ana.
    s.handle(2, ClientMsg::Command(Command::Ready), 2);
    assert!(players(&s)[1].ready && !players(&s)[0].ready);
}

#[test]
fn requests_the_rules_do_not_allow_are_refused_and_change_nothing() {
    let mut s = session(2);
    s.handle(1, join("Ana", None), 0);
    s.handle(1, ClientMsg::Start, 0);
    inbox(&mut s, 1);
    let rows = s.game().unwrap().content().rules().board_rows as i32;
    let cols = s.game().unwrap().content().rules().board_cols as i32;
    let refused = [
        Command::BuyUnit { slot: 99 },
        Command::SellUnit { uid: 999_999 },
        Command::MoveUnit { uid: 999_999, to: Slot::Bench(0) },
        Command::EquipItem { item: 99, uid: 1 },
        Command::Pick { option: 99 },
    ];
    for command in refused {
        let before = players(&s);
        s.handle(1, ClientMsg::Command(command.clone()), 1);
        assert_eq!(errors(&inbox(&mut s, 1)).len(), 1, "{command:?} was refused");
        assert_eq!(players(&s), before, "{command:?} changed nothing");
    }
    // A unit cannot be put on the opponent's half or off the board.
    s.handle(1, ClientMsg::Command(Command::BuyUnit { slot: 0 }), 1);
    inbox(&mut s, 1);
    let uid = players(&s)[0].units().next().expect("bought a unit").uid;
    for to in [Slot::Board(0, 0), Slot::Board(cols, rows - 1), Slot::Board(-1, rows - 1), Slot::Bench(999)] {
        let before = players(&s);
        s.handle(1, ClientMsg::Command(Command::MoveUnit { uid, to }), 1);
        assert_eq!(errors(&inbox(&mut s, 1)).len(), 1, "{to:?} was refused");
        assert_eq!(players(&s), before);
    }
    // Spending more gold than the server says you have is refused.
    while players(&s)[0].gold >= 2 {
        s.handle(1, ClientMsg::Command(Command::Reroll), 1);
    }
    inbox(&mut s, 1);
    let before = players(&s);
    s.handle(1, ClientMsg::Command(Command::Reroll), 1);
    assert!(errors(&inbox(&mut s, 1))[0].contains("gold"));
    assert_eq!(players(&s), before);
}

#[test]
fn nothing_can_be_done_outside_the_planning_phase_or_without_a_seat() {
    let mut s = session(2);
    s.handle(1, join("Ana", None), 0);
    s.handle(9, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    // Before the game.
    s.handle(1, ClientMsg::Command(Command::Reroll), 0);
    assert_eq!(errors(&inbox(&mut s, 1)), ["no round is in progress"]);
    s.handle(1, ClientMsg::Start, 0);
    inbox(&mut s, 1);
    // A spectator.
    let before = players(&s);
    s.handle(9, ClientMsg::Command(Command::Ready), 1);
    s.handle(9, ClientMsg::Start, 1);
    s.handle(9, ClientMsg::Dev(autoengine::protocol::DevCommand::Gold(100)), 1);
    assert_eq!(errors(&inbox(&mut s, 9)).len(), 3);
    assert_eq!(players(&s), before);
    // Cheats are off unless the server says otherwise.
    s.handle(1, ClientMsg::Dev(autoengine::protocol::DevCommand::Gold(100)), 1);
    assert!(errors(&inbox(&mut s, 1))[0].contains("--dev"));
    assert_eq!(players(&s), before);
    // While fights are being shown.
    let mut s = Session::new(
        s.game().unwrap().content().clone(),
        SessionConfig { seats: 2, seed: 7, planning_ms: 30_000, wait_for_playback: true, after_combat_ms: 2_000 },
    );
    s.handle(1, join("Ana", None), 0);
    s.handle(1, ClientMsg::Start, 0);
    s.handle(1, ClientMsg::Command(Command::Ready), 0);
    inbox(&mut s, 1);
    let before = players(&s);
    s.handle(1, ClientMsg::Command(Command::Reroll), 1);
    assert_eq!(errors(&inbox(&mut s, 1)), ["no round is in progress"]);
    assert_eq!(players(&s), before);
}

#[test]
fn a_seat_cannot_be_taken_over_by_name_alone() {
    let mut s = session(2);
    s.handle(1, join("Ana", None), 0);
    s.handle(1, ClientMsg::Start, 0);
    s.disconnect(1, 1);
    inbox(&mut s, 1);
    // Someone else claiming to be Ana, with no token or a wrong one, only
    // gets to watch.
    for token in [None, Some("0123456789abcdef0123456789abcdef".to_string())] {
        s.handle(2, join("Ana", token), 2);
        let welcome = inbox(&mut s, 2).into_iter().find_map(|m| match m {
            ServerMsg::Welcome { seat, .. } => Some(seat),
            _ => None,
        });
        assert_eq!(welcome, Some(None), "watching, not seated");
        assert!(s.seat_is_bot(0));
        s.disconnect(2, 2);
    }
}

#[test]
fn players_can_only_fetch_their_own_fights() {
    let mut s = session(4);
    s.handle(1, join("Ana", None), 0);
    s.handle(9, ClientMsg::Spectate { version: PROTOCOL_VERSION }, 0);
    s.handle(1, ClientMsg::Start, 0);
    s.handle(1, ClientMsg::Command(Command::Ready), 1);
    s.drain_outbox();
    let fights = s.game().unwrap().combats().len() as u32;
    assert_eq!(fights, 4, "one neutral fight each");
    let (mut mine, mut refused) = (0, 0);
    for index in 0..fights {
        s.handle(1, ClientMsg::ResendCombat { id: index }, 2);
        match &inbox(&mut s, 1)[..] {
            [ServerMsg::Combat(record)] => {
                assert_eq!(record.home, PlayerId(0));
                mine += 1;
            }
            [ServerMsg::Error { .. }] => refused += 1,
            other => panic!("unexpected {other:?}"),
        }
        // A spectator may see them all.
        s.handle(9, ClientMsg::ResendCombat { id: index }, 2);
        assert!(matches!(&inbox(&mut s, 9)[..], [ServerMsg::Combat(_)]));
    }
    assert_eq!((mine, refused), (1, 3));
}

#[test]
fn figures_the_client_shows_come_from_the_server() {
    let mut s = session(2);
    s.handle(1, join("Ana", None), 0);
    s.handle(1, ClientMsg::Start, 0);
    let view = inbox(&mut s, 1)
        .into_iter()
        .rev()
        .find_map(|m| match m {
            ServerMsg::State(v) => Some(v),
            _ => None,
        })
        .unwrap();
    let game = s.game().unwrap();
    for player in game.players() {
        let status = view.status_of(player.id).expect("every player has a status");
        assert_eq!(status.board_limit, game.board_limit(player.id));
        let traits = game.content().active_traits(&game.team_setup(player.id).units).unwrap();
        assert_eq!(status.traits, traits);
    }
}
