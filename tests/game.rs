use std::collections::BTreeMap;
use std::sync::Arc;

use autoengine::bot;
use autoengine::content::{BoardUnit, ChoiceOffer, ContentSet, RoundDef, RoundKindDef, Schedule};
use autoengine::game::{Command, CommandError, Game, GameEvent, Opponent, PlayerId, Slot};
use autoengine::rng::Rng;
use autoengine::round::RoundType;
use autoengine::stats::Stat;

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);
const P2: PlayerId = PlayerId(2);

fn sample() -> ContentSet {
    ContentSet::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/content")).expect("sample content loads")
}

/// The sample content with a schedule that repeats the given kinds.
fn with_rounds(kinds: Vec<RoundKindDef>) -> Arc<ContentSet> {
    let rounds = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| RoundDef { name: format!("r{i}"), stage: 1, kind })
        .collect();
    Arc::new(sample().with_schedule(Schedule { rounds, repeat_from: 0 }).unwrap())
}

fn game(kinds: Vec<RoundKindDef>, players: usize) -> Game {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let mut g = Game::new(with_rounds(kinds), 42, &names[..players]);
    g.begin_round().unwrap();
    g
}

/// Gives a player a unit straight onto the board.
fn field(g: &mut Game, id: PlayerId, unit: &str, pos: (i32, i32)) -> u32 {
    g.add_unit(id, unit, vec![]);
    let uid = g.player(id).bench.iter().flatten().last().unwrap().uid;
    g.apply(id, Command::MoveUnit { uid, to: Slot::Board(pos.0, pos.1) }).unwrap();
    uid
}

#[test]
fn buying_and_selling_moves_gold_and_pool_copies() {
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    assert_eq!(g.player(P0).gold, 4);
    assert_eq!(g.player(P0).shop.len(), 5);
    let slot = g.player(P0).shop.iter().position(Option::is_some).unwrap();
    let unit = g.player(P0).shop[slot].clone().unwrap();
    let cost = g.content().unit(&unit).unwrap().cost as i64;
    let pool_before = g.pool_count(&unit);

    g.apply(P0, Command::BuyUnit { slot }).unwrap();
    assert_eq!(g.player(P0).gold, 4 - cost);
    assert_eq!(g.pool_count(&unit), pool_before - 1);
    assert_eq!(g.player(P0).shop[slot], None);
    assert_eq!(g.apply(P0, Command::BuyUnit { slot }), Err(CommandError::EmptyShopSlot));
    let uid = g.player(P0).bench[0].as_ref().unwrap().uid;

    g.apply(P0, Command::SellUnit { uid }).unwrap();
    assert_eq!(g.player(P0).gold, 4);
    assert_eq!(g.pool_count(&unit), pool_before);
    assert_eq!(g.apply(P0, Command::SellUnit { uid }), Err(CommandError::UnknownUnit));

    // Not enough gold changes nothing.
    g.add_gold(P1, -4);
    let slot = g.player(P1).shop.iter().position(Option::is_some).unwrap();
    assert!(matches!(g.apply(P1, Command::BuyUnit { slot }), Err(CommandError::NotEnoughGold { .. })));
    assert!(g.player(P1).shop[slot].is_some());
}

#[test]
fn reroll_xp_and_levels() {
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    g.add_gold(P0, 96);
    g.apply(P0, Command::Reroll).unwrap();
    assert_eq!(g.player(P0).gold, 98);
    assert_eq!(g.board_limit(P0), 1);

    // 4 xp per purchase: level 1 needs 2, level 2 needs 2, so one purchase reaches level 3.
    g.apply(P0, Command::BuyXp).unwrap();
    assert_eq!((g.player(P0).level, g.player(P0).xp, g.player(P0).gold), (3, 0, 94));
    assert_eq!(g.board_limit(P0), 3);

    g.add_gold(P0, 500);
    while g.apply(P0, Command::BuyXp).is_ok() {}
    assert_eq!(g.player(P0).level, 8);
    assert_eq!(g.apply(P0, Command::BuyXp), Err(CommandError::MaxLevel));

    g.add_gold(P1, -4);
    assert!(matches!(g.apply(P1, Command::Reroll), Err(CommandError::NotEnoughGold { need: 2, have: 0 })));
}

#[test]
fn moving_units_respects_the_board_limit_and_swaps() {
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    g.add_unit(P0, "ashen_hound", vec![]);
    g.add_unit(P0, "reed_stalker", vec![]);
    let hound = g.player(P0).bench[0].as_ref().unwrap().uid;
    let reed = g.player(P0).bench[1].as_ref().unwrap().uid;

    // Only the player's own half of the board is allowed.
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Board(3, 3) }), Err(CommandError::InvalidSlot));
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Board(7, 4) }), Err(CommandError::InvalidSlot));
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Bench(9) }), Err(CommandError::InvalidSlot));
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: 999, to: Slot::Bench(0) }), Err(CommandError::UnknownUnit));

    g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Board(3, 4) }).unwrap();
    assert_eq!(g.player(P0).locate(hound), Some(Slot::Board(3, 4)));
    // Level 1 fields one unit.
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: reed, to: Slot::Board(2, 4) }), Err(CommandError::BoardFull));
    // Swapping with the fielded unit is fine.
    g.apply(P0, Command::MoveUnit { uid: reed, to: Slot::Board(3, 4) }).unwrap();
    assert_eq!(g.player(P0).locate(reed), Some(Slot::Board(3, 4)));
    assert_eq!(g.player(P0).locate(hound), Some(Slot::Bench(1)));
    assert_eq!(g.player(P0).board.len(), 1);
}

#[test]
fn three_copies_merge_and_carry_items() {
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    g.add_unit(P0, "ashen_hound", vec!["whetstone".into()]);
    g.add_unit(P0, "ashen_hound", vec![]);
    assert_eq!(g.player(P0).units().count(), 2);
    g.add_unit(P0, "ashen_hound", vec!["oak_charm".into()]);
    let units: Vec<_> = g.player(P0).units().cloned().collect();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].star, 2);
    // The two components combined into the finished item on merge.
    assert_eq!(units[0].items, ["grove_sigil"]);

    // Six more copies make two more 2-stars, which merge into a 3-star.
    for _ in 0..6 {
        g.add_unit(P0, "ashen_hound", vec![]);
    }
    let units: Vec<_> = g.player(P0).units().cloned().collect();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].star, 3);
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::UnitUpgraded { star: 3, .. })));

    // Selling a 3-star returns nine copies and nine times the cost.
    let (gold, pool) = (g.player(P0).gold, g.pool_count("ashen_hound"));
    g.apply(P0, Command::SellUnit { uid: units[0].uid }).unwrap();
    assert_eq!(g.player(P0).gold, gold + 9);
    assert_eq!(g.pool_count("ashen_hound"), pool + 9);
    assert_eq!(g.player(P0).items, ["grove_sigil"]);
}

#[test]
fn a_full_bench_refunds_extra_units_unless_they_complete_an_upgrade() {
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    let kinds = ["ashen_hound", "reed_stalker", "dawnpetal_mender", "ironbark_warden", "emberlash_duelist"];
    // Nine units, never three of a kind.
    for i in 0..9 {
        g.add_unit(P0, kinds[i % 5], vec![]);
    }
    assert!(g.player(P0).bench.iter().all(Option::is_some));
    let gold = g.player(P0).gold;
    g.add_unit(P0, "tidewater_stormcaller", vec![]);
    assert_eq!(g.player(P0).bench.len(), 9);
    assert_eq!(g.player(P0).gold, gold + 2, "no room, so it is refunded");
    // A third hound merges, so it fits.
    g.add_unit(P0, "ashen_hound", vec![]);
    assert_eq!(g.player(P0).bench.len(), 9);
    assert!(g.player(P0).units().any(|u| u.unit == "ashen_hound" && u.star == 2));
}

#[test]
fn items_equip_combine_and_respect_the_limit() {
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    let uid = field(&mut g, P0, "ironbark_warden", (3, 4));
    for item in ["oak_charm", "oak_charm", "ember_core", "ember_core", "ember_core", "whetstone"] {
        g.give_item(P0, item);
    }
    assert_eq!(g.apply(P0, Command::EquipItem { item: 9, uid }), Err(CommandError::UnknownItem));
    assert_eq!(g.apply(P0, Command::EquipItem { item: 0, uid: 999 }), Err(CommandError::UnknownUnit));

    g.apply(P0, Command::EquipItem { item: 0, uid }).unwrap();
    g.apply(P0, Command::EquipItem { item: 0, uid }).unwrap();
    assert_eq!(g.player(P0).unit(uid).unwrap().items, ["barkplate"]);
    g.apply(P0, Command::EquipItem { item: 0, uid }).unwrap();
    g.apply(P0, Command::EquipItem { item: 0, uid }).unwrap();
    assert_eq!(g.player(P0).unit(uid).unwrap().items, ["barkplate", "ember_core", "ember_core"]);
    // Full, and a third ember core combines with nothing.
    assert_eq!(g.apply(P0, Command::EquipItem { item: 0, uid }), Err(CommandError::ItemSlotsFull));
    // A whetstone combines with an ember core, so it is allowed.
    g.apply(P0, Command::EquipItem { item: 1, uid }).unwrap();
    assert_eq!(g.player(P0).unit(uid).unwrap().items, ["barkplate", "cinder_blade", "ember_core"]);
    assert_eq!(g.player(P0).items, ["ember_core"]);
}

#[test]
fn commands_are_only_accepted_during_a_round() {
    let mut g = Game::new(with_rounds(vec![RoundKindDef::Pvp]), 1, &["a", "b"]);
    assert_eq!(g.apply(P0, Command::Reroll), Err(CommandError::NotPlanning));
    assert_eq!(g.finish_round(), Err(CommandError::NotPlanning));
    g.begin_round().unwrap();
    assert_eq!(g.apply(PlayerId(7), Command::Ready), Err(CommandError::UnknownPlayer));
    assert_eq!(g.apply(P0, Command::Pick { option: 0 }), Err(CommandError::NothingToPick));
    assert!(!g.all_ready());
    g.apply(P0, Command::Ready).unwrap();
    g.apply(P1, Command::Ready).unwrap();
    assert!(g.all_ready());
    g.finish_round().unwrap();
    assert_eq!(g.apply(P0, Command::Ready), Err(CommandError::NotPlanning));
}

#[test]
fn pvp_round_damages_the_loser_and_pays_income_next_round() {
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    field(&mut g, P0, "ironbark_warden", (3, 4));
    g.add_gold(P0, 26); // 30 gold: 3 interest
    g.finish_round().unwrap();

    assert_eq!(g.combats().len(), 1);
    let record = &g.combats()[0];
    assert!(!record.events.is_empty());
    // P0 has a unit and P1 has none, so P0 wins whichever side it was on.
    assert_eq!(g.player(P0).health, 100);
    assert_eq!(g.player(P1).health, 100 - 2 - 1, "stage 1 base damage plus one survivor");
    assert_eq!((g.player(P0).streak, g.player(P1).streak), (1, -1));
    assert_eq!(g.player(P0).gold, 31, "win bonus");

    g.begin_round().unwrap();
    assert_eq!(g.player(P0).gold, 31 + 5 + 3);
    assert_eq!(g.player(P1).gold, 4 + 5);
    assert_eq!(g.player(P1).xp, 0);
    assert_eq!(g.player(P1).level, 2, "2 xp per round reaches level 2");
    assert_eq!(g.round_index(), 1);
    assert_eq!(g.stage(), 2, "repeating the schedule raises the stage");
}

#[test]
fn odd_lobbies_fight_a_ghost_that_is_not_affected() {
    let mut g = game(vec![RoundKindDef::Pvp], 3);
    for id in [P0, P1, P2] {
        field(&mut g, id, "ashen_hound", (3, 4));
    }
    g.finish_round().unwrap();
    assert_eq!(g.combats().len(), 2);
    let ghosts: Vec<_> = g.combats().iter().filter(|c| matches!(c.away, Opponent::Ghost(_))).collect();
    assert_eq!(ghosts.len(), 1);
    // Exactly two fights' worth of results: at most two players took damage.
    let damaged = g.players().iter().filter(|p| p.health < 100).count();
    assert!((1..=2).contains(&damaged));
}

#[test]
fn pve_round_rewards_winners_and_damages_losers() {
    let enemies = vec![BoardUnit { unit: "moss_golem".into(), star: 1, items: vec![], pos: (3, 4) }];
    let mut g = game(vec![RoundKindDef::Pve { enemies, gold: 7, items: 2 }], 2);
    field(&mut g, P0, "ironbark_warden", (3, 4));
    g.finish_round().unwrap();
    assert_eq!(g.player(P0).gold, 11);
    assert_eq!(g.player(P0).items.len(), 2);
    assert_eq!(g.player(P0).health, 100);
    assert_eq!(g.player(P1).gold, 4);
    assert!(g.player(P1).items.is_empty());
    assert_eq!(g.player(P1).health, 97);
    assert!(g.combats().iter().all(|c| c.away == Opponent::Neutral));
    assert_eq!(g.player(P0).streak, 0, "neutral fights do not affect streaks");
}

#[test]
fn draft_round_picks_in_health_order_and_returns_leftovers() {
    let mut g = Game::new(with_rounds(vec![RoundKindDef::Draft { extra: 1, max_cost: Some(1) }]), 9, &["a", "b", "c"]);
    g.damage_player(P2, 30);
    g.damage_player(P0, 10);
    let pool_total = |g: &Game| g.content().units().map(|u| g.pool_count(&u.id)).sum::<u32>();
    let before = pool_total(&g);
    g.begin_round().unwrap();

    let options = g.options(P0);
    assert_eq!(options.len(), 4, "one per player plus one extra");
    assert_eq!(pool_total(&g), before - 4, "offered units are reserved");
    // Lowest health picks first.
    assert_eq!(g.awaiting(), [P2]);
    assert_eq!(g.apply(P0, Command::Pick { option: 0 }), Err(CommandError::NotYourTurn));
    g.apply(P2, Command::Pick { option: 1 }).unwrap();
    assert_eq!(g.awaiting(), [P0]);
    assert_eq!(g.apply(P0, Command::Pick { option: 1 }), Err(CommandError::InvalidOption));
    g.apply(P0, Command::Pick { option: 0 }).unwrap();
    assert_eq!(g.awaiting(), [P1]);
    assert!(!g.all_ready());

    // P1 never picks; finishing the round picks for them.
    g.finish_round().unwrap();
    for id in [P0, P1, P2] {
        let p = g.player(id);
        assert_eq!(p.units().count(), 1);
        let unit = p.units().next().unwrap();
        assert_eq!(g.content().unit(&unit.unit).unwrap().cost, 1);
        assert_eq!(unit.items.len(), 1, "drafted units carry an item");
    }
    assert_eq!(pool_total(&g), before - 3, "the unpicked offer went back to the pool");
}

#[test]
fn choice_round_grants_augments_that_change_the_team() {
    let mut g = game(vec![RoundKindDef::Choice { options: 3, offer: ChoiceOffer::Augments }], 2);
    field(&mut g, P0, "ironbark_warden", (3, 4));
    let options = g.options(P0);
    assert_eq!(options.len(), 3);
    assert_eq!(g.awaiting(), [P0, P1]);
    assert_eq!(g.apply(P0, Command::Pick { option: 3 }), Err(CommandError::InvalidOption));
    g.apply(P0, Command::Pick { option: 2 }).unwrap();
    assert_eq!(g.player(P0).augments, [options[2].clone().unwrap()]);
    assert_eq!(g.apply(P0, Command::Pick { option: 0 }), Err(CommandError::NothingToPick));
    assert_eq!(g.awaiting(), [P1]);

    g.finish_round().unwrap();
    assert_eq!(g.player(P1).augments.len(), 1, "undecided players get a random option");
    assert!(g.combats().is_empty(), "a choice round has no fights");
    assert_eq!(g.player(P1).health, 100);

    // The augment's bonuses reach combat through the team setup.
    let mut g = game(vec![RoundKindDef::Pvp], 2);
    field(&mut g, P0, "ironbark_warden", (3, 4));
    let armor = |g: &Game| {
        g.content().build_team(&g.team_setup(P0)).unwrap()[0].template.stats.get(Stat::Armor)
    };
    let before = armor(&g);
    g.give_augment(P0, "iron_discipline");
    assert_eq!(armor(&g), before + 15);
    let gold = g.player(P0).gold;
    g.give_augment(P0, "war_chest");
    assert_eq!(g.player(P0).gold, gold + 12);
}

/// A round type the engine knows nothing about: everyone gets gold, and the
/// amount comes from the schedule.
struct Windfall(i64);

impl RoundType for Windfall {
    fn kind(&self) -> &str {
        "windfall"
    }
    fn finish(&mut self, game: &mut Game) {
        for id in game.alive() {
            game.add_gold(id, self.0);
        }
    }
}

#[test]
fn games_can_register_their_own_round_types() {
    let custom = RoundKindDef::Custom { kind: "windfall".into(), params: BTreeMap::from([("gold".to_string(), 25)]) };
    let content = with_rounds(vec![custom]);

    let mut g = Game::new(content.clone(), 1, &["a", "b"]);
    assert_eq!(g.begin_round(), Err(CommandError::UnknownRoundKind("windfall".into())));

    let mut g = Game::new(content, 1, &["a", "b"]);
    g.register_round_type("windfall", Box::new(|params| Box::new(Windfall(params["gold"]))));
    g.begin_round().unwrap();
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::RoundStarted { kind, .. } if kind == "windfall")));
    // Ordinary commands still work during a custom round.
    g.apply(P0, Command::Reroll).unwrap();
    g.finish_round().unwrap();
    assert_eq!(g.player(P0).gold, 4 - 2 + 25);
    assert_eq!(g.player(P1).gold, 4 + 25);
}

#[test]
fn players_without_health_are_eliminated_and_the_last_one_wins() {
    let mut g = game(vec![RoundKindDef::Pvp], 3);
    field(&mut g, P0, "ironbark_warden", (3, 4));
    g.damage_player(P1, 99);
    g.damage_player(P2, 150);
    let pool = g.pool_count("ashen_hound");
    assert!(g.take_from_pool("ashen_hound"));
    g.add_unit(P2, "ashen_hound", vec![]);
    g.finish_round().unwrap();

    // P2 was already below zero and places last; P1 falls the same round.
    assert_eq!(g.player(P2).placement, Some(3));
    assert_eq!(g.player(P1).placement, Some(2));
    assert_eq!(g.player(P0).placement, Some(1));
    assert!(g.is_over());
    assert_eq!(g.winner(), Some(P0));
    assert_eq!(g.pool_count("ashen_hound"), pool, "an eliminated player's units go back to the pool");
    // Scripts in the sample content may add their own events after it.
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::GameOver { winner } if *winner == P0)));
    assert_eq!(g.begin_round(), Err(CommandError::GameOver));
    assert_eq!(g.apply(P0, Command::Ready), Err(CommandError::GameOver));
}

fn bot_game(seed: u64, players: usize) -> Game {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let mut g = Game::new(sample(), seed, &names[..players]);
    let mut rng = Rng::new(seed + 1);
    while !g.is_over() {
        assert!(g.round_index() < 300, "the game should end");
        bot::play_round(&mut g, &mut rng).unwrap();
        for p in g.players().iter().filter(|p| p.alive) {
            assert!(p.board.len() <= g.board_limit(p.id));
            assert!(p.bench.len() == 9 && p.gold >= 0);
            assert!(p.units().all(|u| u.items.len() <= 3));
        }
    }
    g
}

#[test]
fn a_full_bot_lobby_plays_to_a_single_winner() {
    let g = bot_game(5, 8);
    let mut places: Vec<u32> = g.players().iter().map(|p| p.placement.unwrap()).collect();
    places.sort();
    assert_eq!(places, (1..=8).collect::<Vec<_>>());
    assert_eq!(g.players().iter().filter(|p| p.alive).count(), 1);
    // The sample schedule exercised every built-in round kind.
    for kind in ["pvp", "pve", "draft", "choice"] {
        assert!(
            g.events().iter().any(|e| matches!(e, GameEvent::RoundStarted { kind: k, .. } if k == kind)),
            "no {kind} round was played"
        );
    }
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::UnitUpgraded { .. })));
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::ItemCombined { .. })));
}

#[test]
fn games_are_deterministic() {
    let a = bot_game(21, 4);
    let b = bot_game(21, 4);
    assert_eq!(a.events(), b.events());
    assert_eq!(a.players(), b.players());
    let c = bot_game(22, 4);
    assert_ne!(a.events(), c.events());
}
