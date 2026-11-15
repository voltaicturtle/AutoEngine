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
    autoengine::sample::content()
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
    // The shop already held this copy out of the pool, so buying it leaves
    // the pool as it was; selling puts it back.
    assert_eq!(g.pool_count(&unit), pool_before);
    assert_eq!(g.player(P0).shop[slot], None);
    assert_eq!(g.apply(P0, Command::BuyUnit { slot }), Err(CommandError::EmptyShopSlot));
    let uid = g.player(P0).bench[0].as_ref().unwrap().uid;

    g.apply(P0, Command::SellUnit { uid }).unwrap();
    assert_eq!(g.player(P0).gold, 4);
    assert_eq!(g.pool_count(&unit), pool_before + 1);
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
    // Copies in the pool plus copies shops are holding.
    let pool_total = |g: &Game| {
        g.content().units().map(|u| g.pool_count(&u.id)).sum::<u32>()
            + g.players().iter().map(|p| p.shop.iter().flatten().count() as u32).sum::<u32>()
    };
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
    // Eliminated players' shops let go of their copies too.
    let in_shops = |g: &Game, ids: [PlayerId; 2]| {
        ids.iter().map(|id| g.player(*id).shop.iter().flatten().filter(|u| *u == "ashen_hound").count() as u32).sum::<u32>()
    };
    let pool = g.pool_count("ashen_hound") + in_shops(&g, [P1, P2]);
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

#[test]
fn shops_only_offer_copies_that_exist() {
    // Two copies of each unit and eight shops of five: most slots must stay
    // empty, and no unit may be shown more often than it has copies.
    let base = autoengine::sample::content();
    let mut rules = base.rules().clone();
    rules.pool_sizes.clear();
    rules.default_pool_size = 2;
    let content = ContentSet::new(base.units().cloned().collect(), base.traits().cloned().collect(), base.items().cloned().collect(), rules)
        .unwrap()
        .with_schedule(Schedule::new([RoundDef::new("fight", 1, RoundKindDef::pvp())]))
        .unwrap();
    let names: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut g = Game::new(content, 4, &names);

    let check = |g: &Game| {
        for unit in g.content().units().filter(|u| u.in_shop) {
            let shown: u32 = g.players().iter().map(|p| p.shop.iter().flatten().filter(|o| **o == unit.id).count() as u32).sum();
            let owned: u32 = g.players().iter().map(|p| p.units().filter(|u| u.unit == unit.id).map(|u| 3u32.pow(u.star as u32 - 1)).sum::<u32>()).sum();
            assert_eq!(shown + owned + g.pool_count(&unit.id), 2, "{}: shown {shown}, owned {owned}", unit.id);
        }
    };
    check(&g);
    for round in 0..3 {
        g.begin_round().unwrap();
        check(&g);
        for p in 0..8u8 {
            let id = PlayerId(p);
            // Everything a shop shows can be bought (gold permitting).
            g.add_gold(id, 100);
            for slot in 0..g.player(id).shop.len() {
                if g.player(id).shop[slot].is_some() && g.player(id).bench.iter().any(Option::is_none) {
                    assert_eq!(g.apply(id, Command::BuyUnit { slot }), Ok(()), "round {round}, player {p}, slot {slot}");
                }
            }
            let _ = g.apply(id, Command::Reroll);
            check(&g);
        }
        g.finish_round().unwrap();
    }
}

#[test]
fn a_player_who_forfeits_goes_out_last_when_the_round_ends() {
    use autoengine::game::{Command, CommandError, GameEvent, PlayerId};
    let content = std::sync::Arc::new(autoengine::sample::content());
    let mut g = autoengine::game::Game::new(content, 3, &["a", "b", "c"]);
    // Every copy is in the pool, in a shop or owned (all one-star here).
    let copies = |g: &autoengine::game::Game| -> u32 {
        let pool: u32 = g.content().units().map(|u| g.pool_count(&u.id)).sum();
        let shown = g.players().iter().flat_map(|p| p.shop.iter().flatten()).count() as u32;
        let owned = g.players().iter().map(|p| p.units().count() as u32).sum::<u32>();
        pool + shown + owned
    };
    g.begin_round().unwrap();
    let total = copies(&g);
    g.apply(PlayerId(1), Command::BuyUnit { slot: 0 }).unwrap();
    g.apply(PlayerId(1), Command::Forfeit).unwrap();
    assert!(g.player(PlayerId(1)).ready && g.player(PlayerId(1)).alive);
    assert_eq!(g.apply(PlayerId(1), Command::Reroll), Err(CommandError::Forfeited));
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::PlayerForfeited { player } if *player == PlayerId(1))));
    g.finish_round().unwrap();
    let p = g.player(PlayerId(1));
    assert!(!p.alive);
    assert_eq!(p.placement, Some(3));
    assert!(p.health > 0, "out despite health left");
    // Their unit and shop went back to the pool.
    assert_eq!(p.units().count(), 0);
    assert!(p.shop.iter().all(Option::is_none));
    assert_eq!(copies(&g), total);
    assert!(!g.is_over());

    // The last one standing wins when the other gives up.
    g.begin_round().unwrap();
    g.apply(PlayerId(0), Command::Forfeit).unwrap();
    g.finish_round().unwrap();
    assert!(g.is_over());
    assert_eq!(g.winner(), Some(PlayerId(2)));
    assert_eq!(g.player(PlayerId(0)).placement, Some(2));
}

#[test]
fn forfeiting_during_a_draft_makes_the_pick_so_nobody_waits() {
    use autoengine::content::{RoundDef, RoundKindDef, Schedule};
    use autoengine::game::Command;
    let draft = RoundDef { name: "draft".into(), stage: 1, kind: RoundKindDef::Draft { extra: 0, max_cost: None } };
    let content = autoengine::sample::content().with_schedule(Schedule { rounds: vec![draft], repeat_from: 0 }).unwrap();
    let mut g = autoengine::game::Game::new(content, 1, &["a", "b", "c"]);
    g.begin_round().unwrap();
    let first = g.awaiting()[0];
    g.apply(first, Command::Forfeit).unwrap();
    assert!(!g.awaiting().contains(&first));
    assert!(!g.awaiting().is_empty(), "the next player's turn");
    assert_eq!(g.player(first).units().count(), 1);
}

#[test]
fn player_stats_add_up() {
    let content = std::sync::Arc::new(autoengine::sample::content());
    let start = content.rules().economy.starting_gold;
    let health = content.rules().starting_health;
    let mut g = autoengine::game::Game::new(content, 8, &["a", "b", "c", "d"]);
    let mut rng = autoengine::rng::Rng::new(2);
    while !g.is_over() {
        autoengine::bot::play_round(&mut g, &mut rng).unwrap();
    }
    for p in g.players() {
        let s = &p.stats;
        assert_eq!(start + s.gold_earned - s.gold_spent, p.gold, "{}: {s:?}", p.name);
        assert_eq!(health - s.damage_taken, p.health, "{}", p.name);
        assert!(s.units_bought > 0 && s.rounds > 0 && s.best_star >= 1);
        assert!(s.fights_won + s.fights_lost + s.fights_drawn >= s.rounds / 2);
    }
    let winner = g.player(g.winner().unwrap());
    assert!(winner.stats.rounds >= g.players().iter().map(|p| p.stats.rounds).max().unwrap());
    assert!(g.players().iter().any(|p| p.stats.best_streak >= 2));
    assert!(g.players().iter().map(|p| p.stats.kills).sum::<u32>() > 0);
}

/// A schedule of player fights only.
fn pvp_only(players: &[&str], rules: impl FnOnce(&mut autoengine::content::GameRules)) -> autoengine::game::Game {
    use autoengine::content::{RoundDef, RoundKindDef, Schedule};
    let pvp = RoundDef { name: "fight".into(), stage: 1, kind: RoundKindDef::Pvp };
    let mut content = autoengine::sample::content().with_schedule(Schedule { rounds: vec![pvp], repeat_from: 0 }).unwrap();
    let mut r = content.rules().clone();
    rules(&mut r);
    content = content.with_game_rules(r).unwrap();
    autoengine::game::Game::new(content, 5, players)
}

#[test]
fn players_do_not_meet_the_same_opponent_again_soon() {
    use autoengine::game::{GameEvent, Opponent};
    let mut g = pvp_only(&["a", "b", "c", "d"], |_| {});
    let mut rng = autoengine::rng::Rng::new(1);
    let mut meetings: Vec<Vec<(u8, u8)>> = Vec::new();
    for _ in 0..6 {
        autoengine::bot::play_round(&mut g, &mut rng).unwrap();
        let round: Vec<(u8, u8)> = g
            .drain_events()
            .iter()
            .filter_map(|e| match e {
                GameEvent::CombatFinished { home, away: Opponent::Player(away), .. } => {
                    Some((home.0.min(away.0), home.0.max(away.0)))
                }
                _ => None,
            })
            .collect();
        meetings.push(round);
        if g.alive().len() < 4 {
            break;
        }
    }
    assert!(meetings.len() >= 3);
    // With four players and two rounds remembered, everyone meets each of
    // the other three in turn.
    for window in meetings.windows(3) {
        for pair in &window[2] {
            assert!(!window[0].contains(pair) && !window[1].contains(pair), "{meetings:?}");
        }
    }
    assert_eq!(g.recent_opponents(autoengine::game::PlayerId(0)).len(), 2);

    // Turned off, players are paired in the shuffled order with no memory.
    let g = pvp_only(&["a", "b"], |r| r.avoid_rematch_rounds = 0);
    assert!(g.recent_opponents(autoengine::game::PlayerId(0)).is_empty());
}

#[test]
fn pairing_leaves_the_fewest_rematches() {
    use autoengine::game::PlayerId;
    let mut g = pvp_only(&["a", "b", "c", "d", "e"], |r| r.avoid_rematch_rounds = 3);
    // Fight a few rounds so there is history, then check the chosen
    // pairing is the cheapest of all.
    let mut rng = autoengine::rng::Rng::new(9);
    for _ in 0..3 {
        autoengine::bot::play_round(&mut g, &mut rng).unwrap();
    }
    let alive = g.alive();
    let (pairs, odd) = autoengine::round::pair_up(&g, &alive);
    assert_eq!(pairs.len(), alive.len() / 2);
    assert_eq!(odd.is_some(), alive.len() % 2 == 1);
    let cost = |pairs: &[(PlayerId, PlayerId)]| -> u32 { pairs.iter().map(|(a, b)| g.rematch_cost(*a, *b) + g.rematch_cost(*b, *a)).sum() };
    let chosen = cost(&pairs);
    // Brute force over every ordering's sequential pairing.
    fn permutations(items: Vec<PlayerId>) -> Vec<Vec<PlayerId>> {
        if items.len() <= 1 {
            return vec![items];
        }
        let mut out = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.clone();
            let x = rest.remove(i);
            for mut perm in permutations(rest) {
                perm.insert(0, x);
                out.push(perm);
            }
        }
        out
    }
    let cheapest = permutations(alive.clone())
        .into_iter()
        .map(|order| cost(&order.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect::<Vec<_>>()))
        .min()
        .unwrap();
    assert_eq!(chosen, cheapest);
}

#[test]
fn empty_board_places_are_filled_from_the_bench_before_fights() {
    use autoengine::game::{Command, PlayerId, Slot};
    let mut g = pvp_only(&["a", "b"], |_| {});
    g.begin_round().unwrap();
    let a = PlayerId(0);
    g.grant(a, autoengine::game::Grant::Unit("ashen_hound".into())).unwrap();
    // Take everything off the board.
    let on_board: Vec<u32> = g.player(a).board.values().map(|u| u.uid).collect();
    for (i, uid) in on_board.into_iter().enumerate() {
        g.apply(a, Command::MoveUnit { uid, to: Slot::Bench(i + 3) }).unwrap();
    }
    assert!(g.player(a).board.is_empty());
    g.finish_round().unwrap();
    assert_eq!(g.player(a).board.len(), g.board_limit(a).min(g.player(a).units().count()));

    let mut g = pvp_only(&["a", "b"], |r| r.auto_fill_board = false);
    g.begin_round().unwrap();
    g.grant(a, autoengine::game::Grant::Unit("ashen_hound".into())).unwrap();
    g.finish_round().unwrap();
    assert!(g.player(a).board.is_empty());
}

#[test]
fn single_use_items_do_their_thing_and_are_gone() {
    use autoengine::content::{Consumable, ItemDef};
    use autoengine::game::{Command, CommandError, GameEvent, Grant, PlayerId};
    let base = autoengine::sample::content();
    let mut builder = autoengine::content::ContentSet::builder().rules(base.rules().clone()).schedule(base.schedule().clone());
    for u in base.units() {
        builder = builder.unit(u.clone());
    }
    for t in base.traits() {
        builder = builder.trait_def(t.clone());
    }
    for i in base.items() {
        builder = builder.item(i.clone());
    }
    let content = builder
        .item(ItemDef::new("magnet", "Magnet").consumable(Consumable::RemoveItems))
        .item(ItemDef::new("mirror", "Mirror").consumable(Consumable::Duplicate))
        .item(ItemDef::new("anvil", "Anvil").consumable(Consumable::Reforge))
        .item(ItemDef::new("purse", "Purse").consumable(Consumable::Gold(7)))
        .build()
        .unwrap();
    assert!(!autoengine::game::Game::new(content.clone(), 1, &["a"]).component_items().contains(&"magnet".to_string()));

    let mut g = autoengine::game::Game::new(content, 1, &["a", "b"]);
    let a = PlayerId(0);
    g.begin_round().unwrap();
    g.grant(a, Grant::Unit("ashen_hound".into())).unwrap();
    let uid = g.player(a).units().find(|u| u.unit == "ashen_hound").unwrap().uid;
    let component = g.component_items()[0].clone();
    let index = |g: &autoengine::game::Game, id: &str| g.player(a).items.iter().position(|i| i == id).unwrap();

    // Nothing to remove yet: refused, and the magnet stays.
    g.grant(a, Grant::Item("magnet".into())).unwrap();
    let refused = g.apply(a, Command::EquipItem { item: index(&g, "magnet"), uid });
    assert!(matches!(refused, Err(CommandError::CannotUse(_))));
    g.grant(a, Grant::Item(component.clone())).unwrap();
    g.apply(a, Command::EquipItem { item: index(&g, &component), uid }).unwrap();
    g.apply(a, Command::EquipItem { item: index(&g, "magnet"), uid }).unwrap();
    assert!(g.player(a).unit(uid).unwrap().items.is_empty());
    assert_eq!(g.player(a).items, vec![component.clone()]);
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::ItemUsed { item, .. } if item == "magnet")));

    // A reforged component becomes a different component.
    g.apply(a, Command::EquipItem { item: 0, uid }).unwrap();
    g.grant(a, Grant::Item("anvil".into())).unwrap();
    g.apply(a, Command::EquipItem { item: index(&g, "anvil"), uid }).unwrap();
    let now = &g.player(a).unit(uid).unwrap().items;
    assert_eq!(now.len(), 1);
    assert_ne!(now[0], component);
    assert!(g.component_items().contains(&now[0]));

    // A copy from the pool.
    let pool = g.pool_count("ashen_hound");
    g.grant(a, Grant::Item("mirror".into())).unwrap();
    g.apply(a, Command::EquipItem { item: index(&g, "mirror"), uid }).unwrap();
    assert_eq!(g.pool_count("ashen_hound"), pool - 1);
    assert_eq!(g.player(a).units().filter(|u| u.unit == "ashen_hound").count(), 2);

    let gold = g.player(a).gold;
    g.grant(a, Grant::Item("purse".into())).unwrap();
    g.apply(a, Command::EquipItem { item: index(&g, "purse"), uid }).unwrap();
    assert_eq!(g.player(a).gold, gold + 7);
    assert!(g.player(a).items.is_empty());
}

#[test]
fn single_use_items_cannot_be_recipe_parts() {
    use autoengine::content::{Consumable, ItemDef};
    let base = autoengine::sample::content();
    let err = base
        .into_builder()
        .item(ItemDef::new("magnet", "Magnet").consumable(Consumable::RemoveItems))
        .item(ItemDef::new("rod", "Rod"))
        .item(ItemDef::new("odd", "Odd").recipe("magnet", "rod"))
        .build()
        .unwrap_err();
    assert!(err.to_string().contains("used up"), "{err}");
}

#[test]
fn choice_offers_can_be_rerolled_a_limited_number_of_times() {
    use autoengine::content::{ChoiceOffer, RoundDef, RoundKindDef, Schedule};
    use autoengine::game::{Command, CommandError, PlayerId};
    let choice = RoundDef { name: "pick".into(), stage: 1, kind: RoundKindDef::Choice { options: 2, offer: ChoiceOffer::Augments } };
    let content = autoengine::sample::content().with_schedule(Schedule { rounds: vec![choice], repeat_from: 0 }).unwrap();
    let mut g = autoengine::game::Game::new(content, 4, &["a", "b"]);
    g.begin_round().unwrap();
    let a = PlayerId(0);
    assert_eq!(g.rerolls_left(a), 1);
    let before = g.options(a);
    g.apply(a, Command::RerollPick).unwrap();
    let after = g.options(a);
    assert_eq!(after.len(), 2);
    assert!(after.iter().all(|o| !before.contains(o)), "{before:?} then {after:?}");
    assert_eq!(g.rerolls_left(a), 0);
    assert_eq!(g.apply(a, Command::RerollPick), Err(CommandError::NoRerollsLeft));
    // After picking there is nothing to reroll.
    g.apply(PlayerId(1), Command::Pick { option: 0 }).unwrap();
    assert_eq!(g.rerolls_left(PlayerId(1)), 0);
    assert_eq!(g.apply(PlayerId(1), Command::RerollPick), Err(CommandError::NothingToPick));
    let picked = after[1].clone().unwrap();
    g.apply(a, Command::Pick { option: 1 }).unwrap();
    assert!(g.player(a).augments.contains(&picked));
}

#[test]
fn augments_can_pay_out_every_round() {
    use autoengine::content::{AugmentDef, RoundGrant};
    use autoengine::game::{Command, Grant, PlayerId};
    let base = autoengine::sample::content();
    let mut builder = autoengine::content::ContentSet::builder().rules(base.rules().clone()).schedule(base.schedule().clone());
    for u in base.units() {
        builder = builder.unit(u.clone());
    }
    for t in base.traits() {
        builder = builder.trait_def(t.clone());
    }
    for i in base.items() {
        builder = builder.item(i.clone());
    }
    let content = builder
        .augment(AugmentDef::new("patron", "Patron").per_round(RoundGrant { gold: 3, xp: 1, free_rerolls: 2, components: 1 }))
        .build()
        .unwrap();
    let mut g = autoengine::game::Game::new(content, 3, &["a", "b"]);
    let (a, b) = (PlayerId(0), PlayerId(1));
    g.begin_round().unwrap();
    g.grant(a, Grant::Augment("patron".into())).unwrap();
    g.finish_round().unwrap();
    let (gold_a, gold_b) = (g.player(a).gold, g.player(b).gold);
    let (items_a, items_b) = (g.player(a).items.len(), g.player(b).items.len());
    g.begin_round().unwrap();
    // Same income otherwise, so the difference is the augment (plus interest
    // and streaks, which only add).
    assert!(g.player(a).gold - gold_a >= g.player(b).gold - gold_b + 3 - 1);
    assert_eq!(g.player(a).items.len(), items_a + 1 + (g.player(b).items.len() - items_b));
    assert_eq!(g.player(a).free_rerolls, 2);
    // Free rerolls go first and cost nothing.
    let gold = g.player(a).gold;
    g.apply(a, Command::Reroll).unwrap();
    g.apply(a, Command::Reroll).unwrap();
    assert_eq!((g.player(a).gold, g.player(a).free_rerolls), (gold, 0));
    g.apply(a, Command::Reroll).unwrap();
    assert_eq!(g.player(a).gold, gold - g.content().rules().economy.reroll_cost);
    assert_eq!(g.player(a).stats.rerolls, 3);
}

#[test]
fn a_lobby_modifier_gives_everyone_the_same_twist() {
    use autoengine::game::GameEvent;
    let base = autoengine::sample::content();
    let augments: Vec<String> = base.augments().map(|a| a.id.clone()).take(3).collect();
    let mut rules = base.rules().clone();
    rules.lobby_modifiers = augments.clone();
    let content = std::sync::Arc::new(base.clone().with_game_rules(rules.clone()).unwrap());
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..12 {
        let g = autoengine::game::Game::new(content.clone(), seed, &["a", "b", "c"]);
        let pick = g.lobby_modifier().unwrap().to_string();
        assert!(augments.contains(&pick));
        assert!(g.players().iter().all(|p| p.augments == vec![pick.clone()]));
        assert!(g.events().iter().any(|e| matches!(e, GameEvent::LobbyModifier { augment } if *augment == pick)));
        seen.insert(pick);
    }
    assert!(seen.len() > 1, "different games get different twists");
    assert!(autoengine::game::Game::new(base.clone(), 1, &["a"]).lobby_modifier().is_none());
    rules.lobby_modifiers = vec!["nothing_like_this".into()];
    assert!(base.with_game_rules(rules).unwrap_err().to_string().contains("lobby modifier"));
}
