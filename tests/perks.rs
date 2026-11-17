//! Perks, rule changes, loot, picks of a player's own, and what units earn
//! in fights.

use std::sync::Arc;

use autoengine::content::{
    AugmentDef, BoardUnit, Bonus, ChoiceOffer, Consumable, ContentSet, CurrencyDef, GameBuilder, ItemDef, RoundDef, RoundKindDef, Schedule, TraitDef,
    UnitDef,
};
use autoengine::effect::{Effect, Passive, Selector, Trigger};
use autoengine::game::{Command, CommandError, Game, GameEvent, Grant, PlayerId, Slot};
use autoengine::perk::{LootTable, Perk, Rule, When};
use autoengine::stats::Stat;

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);

fn base() -> GameBuilder {
    autoengine::sample::content().into_builder()
}

fn start(content: ContentSet) -> Game {
    let mut g = Game::new(Arc::new(content), 7, &["a", "b"]);
    g.begin_round().unwrap();
    g
}

fn err(b: GameBuilder) -> String {
    b.build().err().expect("should not build").to_string()
}

/// One of a currency, used as a counter.
fn tick(id: &str) -> Grant {
    Grant::Currency { currency: id.into(), amount: 1 }
}

fn uid_of(g: &Game, id: PlayerId, unit: &str) -> u32 {
    g.player(id).units().find(|u| u.unit == unit).expect("the player has the unit").uid
}

#[test]
fn perks_pay_out_on_their_occasions() {
    let occasions = [
        ("gained", When::Gained),
        ("round", When::RoundStart),
        ("won", When::Won),
        ("lost", When::Lost),
        ("fought", When::Fought),
        ("level", When::LevelUp),
        ("reroll", When::Reroll),
        ("buyxp", When::BuyXp),
        ("sold", When::UnitSold),
        ("star", When::StarUp),
    ];
    let mut builder = base();
    let mut tally = AugmentDef::new("tally", "Tally");
    for (id, when) in occasions {
        builder = builder.currency(CurrencyDef::new(id, id));
        tally = tally.perk(Perk::on(when).gives(tick(id)));
    }
    let mut g = start(builder.augment(tally).build().unwrap());
    let count = |g: &Game, id: PlayerId, what: &str| g.currency(id, what);
    for id in [P0, P1] {
        g.give_augment(id, "tally");
        assert_eq!(count(&g, id, "gained"), 1);
    }
    g.add_gold(P0, 50);
    g.apply(P0, Command::Reroll).unwrap();
    g.apply(P0, Command::Reroll).unwrap();
    assert_eq!(count(&g, P0, "reroll"), 2);
    // Four experience is two levels at the start of the sample game.
    g.apply(P0, Command::BuyXp).unwrap();
    assert_eq!((count(&g, P0, "buyxp"), count(&g, P0, "level"), g.player(P0).level), (1, 2, 3));
    for _ in 0..3 {
        g.add_unit(P0, "ironbark_warden", vec![]);
    }
    assert_eq!(count(&g, P0, "star"), 1);
    g.add_unit(P0, "ashen_hound", vec![]);
    g.apply(P0, Command::SellUnit { uid: uid_of(&g, P0, "ashen_hound") }).unwrap();
    assert_eq!(count(&g, P0, "sold"), 1);
    // The first round is against a neutral board: a two-star warden wins
    // it, an empty board does not.
    g.finish_round().unwrap();
    assert_eq!((count(&g, P0, "won"), count(&g, P0, "lost"), count(&g, P0, "fought")), (1, 0, 1));
    assert_eq!((count(&g, P1, "won"), count(&g, P1, "lost"), count(&g, P1, "fought")), (0, 1, 1));
    assert_eq!(count(&g, P0, "round"), 0);
    g.begin_round().unwrap();
    assert_eq!((count(&g, P0, "round"), count(&g, P1, "round"), count(&g, P1, "gained")), (1, 1, 1));
    // Nothing happened that nobody did.
    assert_eq!((count(&g, P1, "reroll"), count(&g, P1, "sold"), count(&g, P1, "star")), (0, 0, 0));
}

#[test]
fn a_perk_can_be_a_matter_of_chance() {
    let content = base()
        .currency(CurrencyDef::new("luck", "Luck"))
        .augment(AugmentDef::new("dice", "Dice").perk(Perk::on(When::Reroll).gives(tick("luck")).chance(300)))
        .build()
        .unwrap();
    let mut g = start(content);
    g.give_augment(P0, "dice");
    g.add_gold(P0, 1000);
    for _ in 0..200 {
        g.apply(P0, Command::Reroll).unwrap();
    }
    let hits = g.currency(P0, "luck");
    assert!((30..=90).contains(&hits), "{hits} of 200 at 30%");
}

#[test]
fn traits_and_items_carry_perks_and_rules_while_they_are_in_play() {
    let kin = TraitDef::new("kin", "Kin")
        .breakpoint(2, [Bonus::members()])
        .perk(Perk::on(When::Reroll).gives(Grant::Xp(1)))
        .rule(Rule::TeamSize.add(1))
        .rule(Rule::RerollCost.add(-1));
    let banner = ItemDef::new("banner", "War Banner").rule(Rule::TeamSize.add(2)).perk(Perk::on(When::Reroll).gives(Grant::Heal(1)));
    let member = |id: &str| UnitDef::new(id, id).cost(1).traits(["kin"]);
    let mut g = start(base().trait_def(kin).unit(member("sprout")).unit(member("bud")).item(banner).build().unwrap());
    g.add_xp(P0, 2);
    assert_eq!((g.player(P0).level, g.board_limit(P0), g.rule(P0, Rule::RerollCost)), (2, 2, 2));
    g.add_unit(P0, "sprout", vec![]);
    g.add_unit(P0, "bud", vec![]);
    g.add_unit(P0, "ashen_hound", vec![]);
    let (mender, stalker, hound) = (uid_of(&g, P0, "sprout"), uid_of(&g, P0, "bud"), uid_of(&g, P0, "ashen_hound"));
    g.apply(P0, Command::MoveUnit { uid: mender, to: Slot::Board(2, 4) }).unwrap();
    g.apply(P0, Command::MoveUnit { uid: stalker, to: Slot::Board(3, 4) }).unwrap();
    // Two kin units: the tier is active, so there is room for a third.
    assert_eq!((g.board_limit(P0), g.rule(P0, Rule::RerollCost)), (3, 1));
    g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Board(4, 4) }).unwrap();
    let (gold, xp) = (g.player(P0).gold, g.player(P0).xp);
    g.apply(P0, Command::Reroll).unwrap();
    assert_eq!((g.player(P0).gold, g.player(P0).xp), (gold - 1, xp + 1));
    // Off the board the tier is gone, and so are its perk and rules.
    g.apply(P0, Command::MoveUnit { uid: stalker, to: Slot::Bench(5) }).unwrap();
    assert_eq!((g.board_limit(P0), g.rule(P0, Rule::RerollCost)), (2, 2));
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: stalker, to: Slot::Board(3, 4) }), Err(CommandError::BoardFull));

    // An item counts while a unit on the board holds it.
    g.give_item(P0, "banner");
    let slot = g.player(P0).items.iter().position(|i| i == "banner").unwrap();
    g.apply(P0, Command::EquipItem { item: slot, uid: stalker }).unwrap();
    assert_eq!(g.board_limit(P0), 2);
    g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Bench(6) }).unwrap();
    g.apply(P0, Command::MoveUnit { uid: stalker, to: Slot::Board(3, 4) }).unwrap();
    assert_eq!(g.board_limit(P0), 5);
    g.damage_player(P0, 10);
    let health = g.player(P0).health;
    g.apply(P0, Command::Reroll).unwrap();
    assert_eq!(g.player(P0).health, health + 1);
}

#[test]
fn rules_change_what_the_game_charges_and_pays() {
    let content = base()
        .augment(
            AugmentDef::new("ledger", "Ledger")
                .rule(Rule::XpCost.add(-3))
                .rule(Rule::XpPerPurchase.add(2))
                .rule(Rule::XpPerRound.add(1))
                .rule(Rule::RoundIncome.add(4))
                .rule(Rule::InterestCap.add(-4))
                .rule(Rule::WinBonus.add(5))
                .rule(Rule::DamageTaken.add(-2))
                .rule(Rule::RerollCost.add(-10)),
        )
        .augment(AugmentDef::new("insight", "Insight").rule(Rule::ShopLevel.add(20)))
        .build()
        .unwrap();
    let mut g = start(content);
    g.give_augment(P0, "ledger");
    let rules = [Rule::XpCost, Rule::XpPerPurchase, Rule::XpPerRound, Rule::RoundIncome, Rule::InterestCap, Rule::WinBonus, Rule::DamageTaken, Rule::RerollCost];
    assert_eq!(rules.map(|r| g.rule(P0, r)), [1, 6, 3, 9, 1, 6, -2, 0]);
    assert_eq!(rules.map(|r| g.rule(P1, r)), [4, 4, 2, 5, 5, 1, 0, 2]);

    // A price never goes below nothing, and damage never heals.
    let gold = g.player(P0).gold;
    g.apply(P0, Command::Reroll).unwrap();
    g.apply(P0, Command::BuyXp).unwrap();
    assert_eq!(g.player(P0).gold, gold - 1);
    assert_eq!(g.player(P0).stats.xp_bought, 6);
    g.damage_player(P0, 5);
    g.damage_player(P0, 1);
    g.damage_player(P1, 5);
    assert_eq!((g.player(P0).health, g.player(P1).health), (97, 95));
    g.record_result(P0, true);
    g.record_result(P1, true);
    assert_eq!(g.player(P0).gold, gold - 1 + 6);

    // Income and the interest cap at the start of the next round.
    for id in [P0, P1] {
        let have = g.player(id).gold;
        g.add_gold(id, 100 - have);
    }
    g.finish_round().unwrap();
    let before = [g.player(P0).gold, g.player(P1).gold];
    g.begin_round().unwrap();
    let streak = |id| if g.player(id).streak.abs() >= 2 { 1 } else { 0 };
    assert_eq!(g.player(P0).gold - before[0], 9 + 1 + streak(P0));
    assert_eq!(g.player(P1).gold - before[1], 5 + 5 + streak(P1));

    // A shop that rolls as if its owner were at the highest level shows
    // units a level 1 shop never does.
    let seen = |g: &mut Game, id: PlayerId| {
        let mut costly = 0;
        g.add_gold(id, 1000);
        for _ in 0..40 {
            g.apply(id, Command::Reroll).unwrap();
            costly += g.player(id).shop.iter().flatten().filter(|u| g.content().unit(u).unwrap().cost >= 3).count();
        }
        costly
    };
    let low = Arc::new(g.content().clone());
    let mut plain = Game::new(low.clone(), 3, &["a", "b"]);
    plain.begin_round().unwrap();
    assert_eq!(seen(&mut plain, P0), 0);
    plain.give_augment(P1, "insight");
    assert!(seen(&mut plain, P1) > 0);
}

#[test]
fn loot_tables_roll_and_grants_can_be_random() {
    let content = base()
        .loot(LootTable::new("spoils").entry(3, [Grant::Gold(1)]).entry(1, [Grant::Gold(1000), Grant::Loot("extras".into())]).nothing(1))
        .loot(LootTable::new("extras").entry(1, [Grant::Xp(1)]))
        .item(ItemDef::new("relic", "Relic").tag("artifact"))
        .item(ItemDef::new("idol", "Idol").tag("artifact"))
        .build()
        .unwrap();
    assert_eq!(content.loot("spoils").unwrap().entries.len(), 3);
    let mut g = start(content);
    let gold = g.player(P0).gold;
    for _ in 0..300 {
        g.give(P0, &Grant::Loot("spoils".into())).unwrap();
    }
    let gained = g.player(P0).gold - gold;
    let (big, small) = (gained / 1000, gained % 1000);
    // Three in five rolls give 1 gold, one in five gives 1000 and a roll
    // on the second table.
    assert!((140..=220).contains(&small) && (35..=90).contains(&big), "{small} small, {big} big");
    assert!(g.player(P0).level > 1 || g.player(P0).xp > 0);
    assert_eq!(g.give(P0, &Grant::Loot("spoil".into())), Err(CommandError::UnknownItem));

    g.give(P0, &Grant::Component).unwrap();
    g.give(P0, &Grant::ItemTagged("artifact".into())).unwrap();
    let items = &g.player(P0).items;
    assert_eq!(items.len(), 2);
    assert!(g.component_items().contains(&items[0]) && ["relic", "idol"].contains(&items[1].as_str()));
    // A random unit comes out of the pool.
    let pool: u32 = g.pool().values().sum();
    g.give(P0, &Grant::UnitOfCost(3)).unwrap();
    assert_eq!(g.player(P0).units().map(|u| u.unit.as_str()).collect::<Vec<_>>(), vec!["emberlash_duelist"]);
    assert_eq!(g.pool().values().sum::<u32>(), pool - 1);
    g.give(P0, &Grant::Rerolls(2)).unwrap();
    assert_eq!(g.player(P0).free_rerolls, 2);

    // The checker follows grants wherever they are.
    assert!(err(base().augment(AugmentDef::new("a", "A").perk(Perk::on(When::Won).gives(Grant::Loot("spoil".into()))))).contains("unknown loot table `spoil`"));
    assert!(err(base().loot(LootTable::new("empty").nothing(0))).contains("never gives anything"));
    assert!(err(base().loot(LootTable::new("bad").entry(1, [Grant::Item("nothing".into())]))).contains("loot table `bad` gives unknown item"));
    assert!(err(base().item(ItemDef::new("tome", "Tome").perk(Perk::on(When::Won).gives(Grant::ItemTagged("artefact".into()))))).contains("no item has that tag"));
    let cursed = TraitDef::new("cursed", "Cursed").breakpoint(2, [Bonus::members()]).perk(Perk::on(When::Lost).gives(Grant::Augment("nope".into())));
    assert!(err(base().trait_def(cursed)).contains("trait `cursed` gives unknown augment `nope`"));
}

#[test]
fn a_grant_can_open_a_pick_of_the_players_own() {
    let content = base()
        .augment(AugmentDef::new("small", "Small").tier(1))
        .augment(AugmentDef::new("large", "Large").tier(2))
        .augment(AugmentDef::new("huge", "Huge").tier(2))
        .item(ItemDef::new("relic", "Relic").tag("artifact"))
        .item(ItemDef::new("anvil", "Anvil").consumable(Consumable::Give(Grant::Choice { options: 2, offer: ChoiceOffer::Components })))
        .build()
        .unwrap();
    let mut g = start(content);
    assert!(g.awaiting().is_empty() && g.options(P0).is_empty());
    g.give(P0, &Grant::Choice { options: 3, offer: ChoiceOffer::AugmentsOfTier(2) }).unwrap();
    g.give(P0, &Grant::Choice { options: 3, offer: ChoiceOffer::ItemsTagged("artifact".into()) }).unwrap();
    assert_eq!(g.awaiting(), vec![P0]);
    assert!(!g.all_ready() && g.options(P1).is_empty());
    let mut offered: Vec<String> = g.options(P0).into_iter().flatten().collect();
    offered.sort();
    assert_eq!(offered, vec!["huge", "large"]);
    assert_eq!(g.apply(P0, Command::Pick { option: 2 }), Err(CommandError::InvalidOption));
    assert_eq!(g.apply(P1, Command::Pick { option: 0 }), Err(CommandError::NothingToPick));
    let pick = g.options(P0)[1].clone().unwrap();
    g.apply(P0, Command::Pick { option: 1 }).unwrap();
    assert_eq!(g.player(P0).augments, vec![pick.clone()]);
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::ChoiceMade { player, choice, .. } if *player == P0 && *choice == pick)));
    // The next pick is up; left alone, it is made when the round ends.
    assert_eq!(g.options(P0), vec![Some("relic".to_string())]);
    g.finish_round().unwrap();
    assert_eq!(g.player(P0).items, vec!["relic"]);
    assert!(g.player(P0).choices.is_empty() && g.awaiting().is_empty());

    // An item that opens a pick when it is used on a unit.
    g.begin_round().unwrap();
    g.add_unit(P0, "ashen_hound", vec![]);
    g.give_item(P0, "anvil");
    let slot = g.player(P0).items.iter().position(|i| i == "anvil").unwrap();
    g.apply(P0, Command::EquipItem { item: slot, uid: uid_of(&g, P0, "ashen_hound") }).unwrap();
    let options = g.options(P0);
    assert_eq!(options.len(), 2);
    assert!(options.iter().flatten().all(|o| g.component_items().contains(o)));
    g.apply(P0, Command::Pick { option: 0 }).unwrap();
    assert_eq!(g.player(P0).items.len(), 2);

    // A pick of units takes the one chosen out of the pool.
    g.give(P0, &Grant::Choice { options: 2, offer: ChoiceOffer::Units { cost: 2 } }).unwrap();
    let unit = g.options(P0)[0].clone().unwrap();
    let left = g.pool_count(&unit);
    g.apply(P0, Command::Pick { option: 0 }).unwrap();
    assert_eq!(g.pool_count(&unit), left - 1);
    assert!(g.player(P0).units().any(|u| u.unit == unit));
}

#[test]
fn a_choice_round_can_offer_one_tier_or_roll_for_it() {
    let tiers = |b: GameBuilder| {
        b.augment(AugmentDef::new("small", "Small").tier(1))
            .augment(AugmentDef::new("minor", "Minor").tier(1))
            .augment(AugmentDef::new("large", "Large").tier(2))
            .augment(AugmentDef::new("huge", "Huge").tier(2))
    };
    let with = |offer: ChoiceOffer| tiers(base()).schedule(Schedule::new([RoundDef::new("pick", 1, RoundKindDef::choice(2, offer))]));
    let mut g = start(with(ChoiceOffer::AugmentsOfTier(1)).build().unwrap());
    for id in [P0, P1] {
        let mut options: Vec<String> = g.options(id).into_iter().flatten().collect();
        options.sort();
        assert_eq!(options, vec!["minor", "small"]);
    }
    g.apply(P0, Command::Pick { option: 0 }).unwrap();
    assert_eq!(g.player(P0).augments.len(), 1);

    // Rolled once: both players are offered the same tier, and over many
    // games both tiers come up.
    let content = Arc::new(with(ChoiceOffer::AugmentTiers(vec![(1, 1), (2, 1)])).build().unwrap());
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..20 {
        let mut g = Game::new(content.clone(), seed, &["a", "b"]);
        g.begin_round().unwrap();
        let tier_of = |id| {
            let tiers: std::collections::BTreeSet<u32> =
                g.options(id).into_iter().flatten().map(|a| g.content().augment(&a).unwrap().tier).collect();
            assert_eq!(tiers.len(), 1);
            tiers.into_iter().next().unwrap()
        };
        assert_eq!(tier_of(P0), tier_of(P1));
        seen.insert(tier_of(P0));
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), vec![1, 2]);

    assert!(err(with(ChoiceOffer::AugmentsOfTier(7))).contains("round `pick` offers augments of tier 7"));
    assert!(err(with(ChoiceOffer::AugmentTiers(vec![(1, 1), (9, 2)]))).contains("can roll augment tier 9"));
    assert!(err(with(ChoiceOffer::AugmentTiers(vec![(1, 0)]))).contains("no tier has a weight above 0"));
    assert!(err(with(ChoiceOffer::ItemsTagged("artifact".into()))).contains("no item has that tag"));
    assert!(err(with(ChoiceOffer::Units { cost: 9 })).contains("no shop unit costs that"));
    let anvil = ItemDef::new("anvil", "Anvil").consumable(Consumable::Give(Grant::Choice { options: 0, offer: ChoiceOffer::Items }));
    assert!(err(base().item(anvil)).contains("opens a pick with no options"));
}

#[test]
fn a_big_unit_takes_more_than_one_team_slot() {
    let titan = UnitDef::new("titan", "Titan").cost(1).stat(Stat::MaxHealth, 2000).team_slots(2);
    let content = base().unit(titan).build().unwrap();
    assert!(err(base().unit(UnitDef::new("ghost", "Ghost").team_slots(0))).contains("takes 0 team slots"));
    let mut g = start(content);
    g.add_xp(P0, 2);
    assert_eq!(g.board_limit(P0), 2);
    g.add_unit(P0, "titan", vec![]);
    g.add_unit(P0, "ashen_hound", vec![]);
    g.add_unit(P0, "reed_stalker", vec![]);
    let (titan, hound, stalker) = (uid_of(&g, P0, "titan"), uid_of(&g, P0, "ashen_hound"), uid_of(&g, P0, "reed_stalker"));
    g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Board(3, 4) }).unwrap();
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: titan, to: Slot::Board(2, 4) }), Err(CommandError::BoardFull));
    // Trading places with a small unit does not make room for a big one.
    g.apply(P0, Command::MoveUnit { uid: stalker, to: Slot::Board(4, 4) }).unwrap();
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: titan, to: Slot::Board(3, 4) }), Err(CommandError::BoardFull));
    g.apply(P0, Command::MoveUnit { uid: stalker, to: Slot::Bench(7) }).unwrap();
    g.apply(P0, Command::MoveUnit { uid: titan, to: Slot::Board(3, 4) }).unwrap();
    assert_eq!((g.board_slots_used(P0), g.player(P0).board.len()), (2, 1));
    assert_eq!(g.apply(P0, Command::MoveUnit { uid: hound, to: Slot::Board(4, 4) }), Err(CommandError::BoardFull));
    // Filling the board at the end of planning counts slots too.
    g.finish_round().unwrap();
    assert_eq!(g.player(P0).board.len(), 1);
}

#[test]
fn a_tier_can_ask_for_an_exact_count_and_items_can_be_made_better() {
    let lone = TraitDef::new("loner", "Loner")
        .breakpoint_exactly(1, [Bonus::members().mods([Stat::AttackDamage.flat(50)])])
        .breakpoint(3, [Bonus::members().mods([Stat::AttackDamage.flat(10)])]);
    let content = base()
        .trait_def(lone)
        .units(["wolf", "crow", "moth"].map(|id| UnitDef::new(id, id).cost(1).traits(["loner"])))
        .item(ItemDef::new("hone", "Hone").mods([Stat::AttackDamage.flat(10)]).upgrades_to("keen_hone"))
        .item(ItemDef::new("keen_hone", "Keen Hone").mods([Stat::AttackDamage.flat(30)]))
        .item(ItemDef::new("polish", "Polish").consumable(Consumable::Upgrade))
        .build()
        .unwrap();
    let tier = |units: &[&str]| {
        let board: Vec<BoardUnit> = units.iter().enumerate().map(|(i, u)| BoardUnit::new(*u, (i as i32, 4))).collect();
        content.active_traits(&board).unwrap().into_iter().find(|t| t.id == "loner").and_then(|t| t.tier)
    };
    assert_eq!(tier(&["wolf"]), Some(0));
    assert_eq!(tier(&["wolf", "crow"]), None);
    assert_eq!(tier(&["wolf", "crow", "moth"]), Some(1));
    assert!(err(base().item(ItemDef::new("x", "X").upgrades_to("nothing"))).contains("upgrades to unknown item `nothing`"));

    let mut g = start(content);
    g.add_unit(P0, "ashen_hound", vec!["oak_charm".into(), "hone".into()]);
    let hound = uid_of(&g, P0, "ashen_hound");
    g.give_item(P0, "polish");
    g.give_item(P0, "polish");
    g.apply(P0, Command::EquipItem { item: 0, uid: hound }).unwrap();
    assert_eq!(g.player(P0).unit(hound).unwrap().items, vec!["oak_charm", "keen_hone"]);
    // Nothing left to improve: the second one is kept.
    assert!(matches!(g.apply(P0, Command::EquipItem { item: 0, uid: hound }), Err(CommandError::CannotUse(_))));
    assert_eq!(g.player(P0).items, vec!["polish"]);
}

/// A game of neutral rounds against one weak monster that drops loot.
fn hunt(reaper: UnitDef) -> ContentSet {
    let piggy = UnitDef::new("piggy", "Piggy")
        .stat(Stat::MaxHealth, 50)
        .stat(Stat::AttackDamage, 0)
        .summon_only()
        .passive(Passive::new("Hoard", Trigger::OnDeath).effect(Effect::reward(Grant::Loot("orb".into())).to(Selector::EventOther)));
    base()
        .unit(piggy)
        .unit(reaper)
        .currency(CurrencyDef::new("orbs", "Orbs"))
        .loot(LootTable::new("orb").entry(1, [Grant::Currency { currency: "orbs".into(), amount: 1 }]))
        .schedule(Schedule::new([RoundDef::new("hunt", 1, RoundKindDef::pve([BoardUnit::new("piggy", (3, 4)), BoardUnit::new("piggy", (4, 4))]))]))
        .build()
        .unwrap()
}

#[test]
fn units_earn_rewards_and_lasting_growth_in_fights() {
    let reaper = UnitDef::new("reaper", "Reaper").cost(1).stat(Stat::AttackDamage, 200).passive(
        Passive::new("Harvest", Trigger::OnKill)
            .effect(Effect::reward(Grant::Gold(1)).to(Selector::Source))
            .effect(Effect::grow([Stat::AttackDamage.flat(5), Stat::MaxHealth.flat(20)]).to(Selector::Source)),
    );
    let content = Arc::new(hunt(reaper));
    assert!(err(content.as_ref().clone().into_builder().unit(
        UnitDef::new("bad", "Bad").passive(Passive::new("P", Trigger::OnKill).effect(Effect::reward(Grant::Loot("orbz".into())).to(Selector::Source)))
    ))
    .contains("unknown loot table `orbz`"));

    let mut g = Game::new(content.clone(), 4, &["a", "b"]);
    g.begin_round().unwrap();
    g.add_unit(P0, "reaper", vec![]);
    let reaper = uid_of(&g, P0, "reaper");
    g.apply(P0, Command::MoveUnit { uid: reaper, to: Slot::Board(3, 5) }).unwrap();
    let gold = g.player(P0).gold;
    g.finish_round().unwrap();
    // Two kills: two gold, two orbs from the monsters, and growth that stays.
    assert_eq!(g.player(P0).gold, gold + 2);
    assert_eq!(g.currency(P0, "orbs"), 2);
    assert_eq!(g.player(P0).unit(reaper).unwrap().growth, vec![Stat::AttackDamage.flat(10), Stat::MaxHealth.flat(40)]);
    // The player with no units killed nothing and earned nothing.
    assert_eq!((g.currency(P1, "orbs"), g.player(P1).gold), (0, 4));
    let record = g.combats().iter().find(|c| c.home == P0).unwrap();
    let earned = record.events.iter().filter(|e| matches!(e.kind, autoengine::combat::EventKind::Rewarded { .. })).count();
    let grew = record.events.iter().filter(|e| matches!(e.kind, autoengine::combat::EventKind::Grew { .. })).count();
    assert_eq!((earned, grew), (4, 2));

    // The next fight starts with what it earned, and replays the same.
    g.begin_round().unwrap();
    let setup = g.team_setup(P0);
    assert_eq!(setup.units[0].growth.len(), 2);
    let plain = content.base_template("reaper", 1).unwrap().stats;
    let built = content.build_team(&setup).unwrap();
    assert_eq!(built[0].template.stats.get(Stat::AttackDamage), plain.get(Stat::AttackDamage) + 10);
    assert_eq!(built[0].template.stats.get(Stat::MaxHealth), plain.get(Stat::MaxHealth) + 40);
    g.finish_round().unwrap();
    assert_eq!(g.player(P0).unit(reaper).unwrap().growth, vec![Stat::AttackDamage.flat(20), Stat::MaxHealth.flat(80)]);
    let record = g.combats().iter().find(|c| c.home == P0).unwrap().clone();
    assert_eq!(autoengine::replay::fight_again(&content, &record, record.events.len()).unwrap(), record.events);

    // Copies that merge keep what they had earned.
    g.begin_round().unwrap();
    g.add_unit(P0, "reaper", vec![]);
    g.add_unit(P0, "reaper", vec![]);
    let merged = g.player(P0).unit(reaper).unwrap();
    assert_eq!((merged.star, merged.growth.clone()), (2, vec![Stat::AttackDamage.flat(20), Stat::MaxHealth.flat(80)]));
    // And a unit can be given growth or a trait from outside a fight.
    g.grow_unit(P0, reaper, &[Stat::AttackDamage.flat(1)]);
    g.add_unit_trait(P0, reaper, "cinder");
    g.add_unit_trait(P0, reaper, "cinder");
    let unit = g.player(P0).unit(reaper).unwrap();
    assert_eq!((unit.growth[0], unit.traits.clone()), (Stat::AttackDamage.flat(21), vec!["cinder".to_string()]));
    let traits = content.active_traits(&g.team_setup(P0).units).unwrap();
    assert_eq!(traits.iter().map(|t| (t.id.as_str(), t.count)).collect::<Vec<_>>(), vec![("cinder", 1)]);
    assert!(content.active_traits(&[BoardUnit::new("reaper", (3, 4)).with_trait("cindr")]).unwrap_err().to_string().contains("unknown trait `cindr`"));
}

#[test]
fn a_copy_of_a_board_earns_nothing_for_its_owner() {
    let reaper = UnitDef::new("reaper", "Reaper").cost(1).stat(Stat::AttackDamage, 400).stat(Stat::MaxHealth, 3000).passive(
        Passive::new("Harvest", Trigger::OnKill)
            .effect(Effect::reward(Grant::Gold(10)).to(Selector::Source))
            .effect(Effect::grow([Stat::AttackDamage.flat(5)]).to(Selector::Source)),
    );
    let content = hunt(reaper).into_builder().schedule(Schedule::new([RoundDef::new("duel", 1, RoundKindDef::pvp())])).build().unwrap();
    let mut g = Game::new(Arc::new(content), 2, &["a", "b", "c"]);
    g.begin_round().unwrap();
    for id in [P0, P1, PlayerId(2)] {
        g.add_unit(id, if id == P0 { "reaper" } else { "ashen_hound" }, vec![]);
    }
    let gold: Vec<i64> = g.players().iter().map(|p| p.gold).collect();
    g.finish_round().unwrap();
    // With three players one of them fights a copy of someone's board.
    let ghost = g.combats().iter().find(|c| matches!(c.away, autoengine::game::Opponent::Ghost(_))).expect("an odd player out").clone();
    let reaper_kills: usize = g
        .combats()
        .iter()
        .filter(|c| c.home == P0 || c.away == autoengine::game::Opponent::Player(P0))
        .map(|c| c.events.iter().filter(|e| matches!(e.kind, autoengine::combat::EventKind::Grew { .. })).count())
        .sum();
    let p0 = g.player(P0);
    let streak_and_win = p0.gold - gold[0] - 10 * reaper_kills as i64;
    assert!((0..=1).contains(&streak_and_win), "only real kills paid: {streak_and_win}");
    assert_eq!(p0.units().next().unwrap().growth.first().map_or(0, |m| m.flat), 5 * reaper_kills as i64);
    // Whoever was copied got nothing from the copy's fight.
    if ghost.away == autoengine::game::Opponent::Ghost(P0) {
        assert!(ghost.events.iter().any(|e| matches!(e.kind, autoengine::combat::EventKind::Rewarded { .. })) || reaper_kills <= 1);
    }
}
