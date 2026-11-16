//! Actions a game defines for its players, and the helpers game code uses.

use std::sync::Arc;

use autoengine::content::{ActionDef, AugmentDef, ContentSet, CurrencyDef, RoundGrant};
use autoengine::game::{Command, CommandError, Game, GameEvent, Grant, PlayerId};
use autoengine::tools::lint;

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);

fn act(id: &str) -> Command {
    Command::Action { id: id.into(), arg: 0 }
}

/// The sample units with a `souls` currency and a few actions.
fn base() -> autoengine::content::GameBuilder {
    autoengine::sample::content().into_builder().currency(CurrencyDef::new("souls", "Souls").starting(5))
}

fn start(content: ContentSet) -> Game {
    let mut g = Game::new(Arc::new(content), 7, &["a", "b"]);
    g.begin_round().unwrap();
    g
}

#[test]
fn an_action_takes_its_price_and_gives_what_it_says() {
    let content = base()
        .action(ActionDef::new("study", "Study").description("Souls for experience.").costs("souls", 2).costs("gold", 1).gives(Grant::Xp(3)).gives(Grant::Gold(0)))
        .build()
        .unwrap();
    let mut g = start(content);
    g.add_gold(P0, 10);
    let (gold, xp, spent) = (g.player(P0).gold, (g.player(P0).level, g.player(P0).xp), g.player(P0).stats.gold_spent);
    g.apply(P0, act("study")).unwrap();
    assert_eq!(g.currency(P0, "souls"), 3);
    assert_eq!(g.player(P0).gold, gold - 1);
    assert_eq!(g.player(P0).stats.gold_spent, spent + 1);
    assert_ne!((g.player(P0).level, g.player(P0).xp), xp);
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::ActionUsed { player, action, arg: 0 } if *player == P0 && action == "study")));
    // Nobody else paid, and the command is part of the game's inputs.
    assert_eq!(g.currency(P1, "souls"), 5);
    assert!(g.inputs().iter().any(|i| matches!(i, autoengine::game::Input::Command { command, .. } if *command == act("study"))));

    // The price is checked for every currency, and a refusal changes nothing.
    g.apply(P0, act("study")).unwrap();
    assert_eq!(g.apply(P0, act("study")), Err(CommandError::NotEnough { currency: "souls".into(), need: 2, have: 1 }));
    assert_eq!(g.apply(P0, act("stady")), Err(CommandError::UnknownAction));
    g.add_currency(P0, "souls", 4);
    let gold = g.player(P0).gold;
    g.add_gold(P0, -gold);
    assert_eq!(g.apply(P0, act("study")), Err(CommandError::NotEnoughGold { need: 1, have: 0 }));
    assert_eq!(g.currency(P0, "souls"), 5);
}

#[test]
fn a_limit_counts_uses_per_round() {
    let content = base().action(ActionDef::new("alms", "Alms").gives(Grant::Gold(1)).per_round(2)).build().unwrap();
    let mut g = start(content);
    g.apply(P0, act("alms")).unwrap();
    g.apply(P0, act("alms")).unwrap();
    assert_eq!(g.player(P0).actions_used.get("alms"), Some(&2));
    let limit = g.apply(P0, act("alms")).unwrap_err();
    assert_eq!(limit, CommandError::ActionLimit { per_round: 2 });
    assert_eq!(limit.to_string(), "already used 2 times this round");
    // Each player has their own count, and it starts over every round.
    g.apply(P1, act("alms")).unwrap();
    g.finish_round().unwrap();
    g.begin_round().unwrap();
    assert!(g.player(P0).actions_used.is_empty());
    g.apply(P0, act("alms")).unwrap();
}

#[test]
fn a_handler_does_the_work_and_can_refuse() {
    let content = base()
        .action(ActionDef::new("bloodprice", "Blood Price").costs("souls", 1))
        .action_handler("bloodprice", |game, player, _| {
            if game.player(player).health <= 95 {
                return Err(CommandError::CannotUse("not enough health".into()));
            }
            game.damage_player(player, 5);
            game.add_gold(player, 4);
            Ok(())
        })
        .build()
        .unwrap();
    let mut g = start(content);
    let gold = g.player(P0).gold;
    g.apply(P0, act("bloodprice")).unwrap();
    assert_eq!((g.player(P0).gold, g.player(P0).health, g.currency(P0, "souls")), (gold + 4, 95, 4));
    // Refused: nothing is charged and nothing is recorded.
    let inputs = g.inputs().len();
    assert_eq!(g.apply(P0, act("bloodprice")), Err(CommandError::CannotUse("not enough health".into())));
    assert_eq!((g.player(P0).gold, g.currency(P0, "souls")), (gold + 4, 4));
    assert_eq!(g.inputs().len(), inputs);
    assert_eq!(g.events().iter().filter(|e| matches!(e, GameEvent::ActionUsed { .. })).count(), 1);
}

#[test]
fn an_action_on_a_unit_needs_one_of_the_players_own() {
    // The sample's offering: give up a unit for favor equal to its worth.
    let mut g = start(autoengine::sample::content());
    let unit = g.content().units().find(|u| u.in_shop && u.cost == 2).unwrap().id.clone();
    g.add_unit(P0, &unit, vec![]);
    g.add_unit(P1, &unit, vec![]);
    let mine = g.player(P0).bench.iter().flatten().last().unwrap().uid;
    let theirs = g.player(P1).bench.iter().flatten().last().unwrap().uid;
    let pool = g.pool_count(&unit);
    let offer = |uid: u32| Command::Action { id: "offering".into(), arg: i64::from(uid) };
    assert_eq!(g.apply(P0, offer(theirs)), Err(CommandError::UnknownUnit));
    assert_eq!(g.apply(P0, Command::Action { id: "offering".into(), arg: -1 }), Err(CommandError::UnknownUnit));
    let favor = g.currency(P0, "favor");
    g.apply(P0, offer(mine)).unwrap();
    assert_eq!(g.currency(P0, "favor"), favor + 2);
    assert!(g.player(P0).unit(mine).is_none());
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::UnitRemoved { player, uid, .. } if *player == P0 && *uid == mine)));
    // add_unit does not take from the pool, so giving one back adds a copy.
    assert_eq!(g.pool_count(&unit), pool + 1);
    // Once a round.
    g.add_unit(P0, &unit, vec![]);
    let next = g.player(P0).bench.iter().flatten().last().unwrap().uid;
    assert_eq!(g.apply(P0, offer(next)), Err(CommandError::ActionLimit { per_round: 1 }));
    // Petition spends the favor.
    g.add_currency(P0, "favor", 1);
    let xp = (g.player(P0).level, g.player(P0).xp);
    g.apply(P0, act("petition")).unwrap();
    assert_eq!(g.currency(P0, "favor"), favor);
    assert_ne!((g.player(P0).level, g.player(P0).xp), xp);
}

#[test]
fn actions_are_validated() {
    let err = |b: autoengine::content::GameBuilder| b.build().err().expect("should not build").to_string();
    let message = err(base().action(ActionDef::new("a", "A").costs("soul", 1)));
    assert!(message.contains("costs unknown currency `soul`") && message.contains("souls"), "{message}");
    assert!(err(base().action(ActionDef::new("a", "A").costs("souls", 0))).contains("must be positive"));
    assert!(err(base().action(ActionDef::new("a", "A").per_round(0))).contains("0 times a round"));
    assert!(err(base().action(ActionDef::new("", "A"))).contains("needs an id"));
    assert!(err(base().action(ActionDef::new("a", "A").gives(Grant::Item("nothing".into())))).contains("gives unknown item `nothing`"));
    assert!(err(base().action(ActionDef::new("a", "A").gives(Grant::Unit("nobody".into())))).contains("gives unknown unit `nobody`"));
    assert!(err(base().action(ActionDef::new("a", "A").gives(Grant::Currency { currency: "gems".into(), amount: 1 }))).contains("gives unknown currency `gems`"));
    let message = err(base().action(ActionDef::new("study", "Study").gives(Grant::Xp(1))).action_handler("stdy", |_, _, _| Ok(())));
    assert!(message.contains("handler for the action `stdy`") && message.contains("study"), "{message}");
    // The same id again replaces the action.
    let twice = base().action(ActionDef::new("a", "A").gives(Grant::Xp(1))).action(ActionDef::new("a", "B").gives(Grant::Xp(2))).build().unwrap();
    assert_eq!(twice.actions().iter().filter(|a| a.id == "a").count(), 1);
    assert_eq!(twice.action("a").unwrap().name, "B");
    // Actions can be translated like everything else with a name.
    assert!(twice.text_keys().contains(&"action.a.description".to_string()));
    assert_eq!(twice.text(None, "action", "a", "name").as_deref(), Some("B"));
}

#[test]
fn lint_points_at_currencies_and_actions_that_lead_nowhere() {
    // No scripts or handlers here, so the content has to do it all itself.
    let bare = || ContentSet::builder().unit(autoengine::content::UnitDef::new("pawn", "Pawn").cost(1));
    let content = bare().currency(CurrencyDef::new("dust", "Dust")).action(ActionDef::new("wave", "Wave")).build().unwrap();
    let warnings: Vec<String> = lint(&content, None).iter().map(ToString::to_string).collect();
    assert!(warnings.iter().any(|w| w == "currency dust: nothing in the content gives it to players"), "{warnings:?}");
    assert!(warnings.iter().any(|w| w == "currency dust: nothing in the content lets players spend it"), "{warnings:?}");
    assert!(warnings.iter().any(|w| w == "action wave: gives nothing and has no handler, so using it does nothing"), "{warnings:?}");
    let content = bare()
        .currency(CurrencyDef::new("dust", "Dust").per_round(1))
        .action(ActionDef::new("wave", "Wave").costs("dust", 1).gives(Grant::Gold(1)))
        .build()
        .unwrap();
    let warnings: Vec<String> = lint(&content, None).iter().map(ToString::to_string).collect();
    assert!(!warnings.iter().any(|w| w.contains("dust") || w.contains("wave")), "{warnings:?}");
}

#[test]
fn players_can_be_healed_up_to_their_starting_health() {
    let content = base().augment(AugmentDef::new("mend", "Mend").per_round(RoundGrant { health: 3, ..Default::default() })).build().unwrap();
    let mut g = start(content);
    let full = g.player(P0).health;
    g.heal_player(P0, 5);
    assert_eq!(g.player(P0).health, full);
    g.damage_player(P0, 10);
    g.heal_player(P0, 4);
    assert_eq!(g.player(P0).health, full - 6);
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::PlayerHealed { player, amount: 4, health } if *player == P0 && *health == full - 6)));
    g.grant(P0, Grant::Heal(100)).unwrap();
    assert_eq!(g.player(P0).health, full);
    // An augment can heal every round.
    g.damage_player(P0, 10);
    g.damage_player(P1, 10);
    g.grant(P0, Grant::Augment("mend".into())).unwrap();
    g.finish_round().unwrap();
    let (a, b) = (g.player(P0).health, g.player(P1).health);
    g.begin_round().unwrap();
    assert_eq!(g.player(P0).health, (a + 3).min(full));
    assert_eq!(g.player(P1).health, b);
}

#[test]
fn gold_is_a_currency_like_any_other_to_game_code() {
    let mut g = start(base().build().unwrap());
    let gold = g.player(P0).gold;
    assert_eq!(g.currency(P0, "gold"), gold);
    g.add_currency(P0, "gold", 3);
    assert_eq!(g.player(P0).gold, gold + 3);
    g.spend_currency(P0, "gold", 2).unwrap();
    assert_eq!(g.player(P0).gold, gold + 1);
    assert_eq!(g.spend_currency(P0, "gold", 1000), Err(CommandError::NotEnoughGold { need: 1000, have: gold + 1 }));
    assert_eq!(g.player(P0).balance("gold"), gold + 1);
}

#[test]
fn bots_spend_currencies_that_have_no_other_use() {
    use autoengine::bot::{self, Difficulty};
    let content = Arc::new(autoengine::sample::content());
    let mut g = Game::new(content, 11, &["a", "b", "c", "d"]);
    let mut rng = autoengine::rng::Rng::new(5);
    let mut used = 0;
    for _ in 0..8 {
        if g.is_over() {
            break;
        }
        bot::play_round_with(&mut g, &mut rng, |_| Difficulty::Hard).unwrap();
        used += g.drain_events().iter().filter(|e| matches!(e, GameEvent::ActionUsed { action, .. } if action == "petition")).count();
    }
    assert!(used > 0, "favor should have been spent on petitions");
    assert!(g.players().iter().all(|p| p.wallet["favor"] < 3 || !p.alive));
}

#[test]
fn pve_rounds_and_single_use_items_can_give_any_grant() {
    use autoengine::content::{BoardUnit, Consumable, ItemDef, RoundDef, RoundKindDef, Schedule};
    let golem = BoardUnit::new("moss_golem", (3, 4));
    let hunt = RoundKindDef::pve([golem]).gold(1).reward(Grant::Currency { currency: "souls".into(), amount: 2 }).reward(Grant::Xp(1));
    let build = |hunt: RoundKindDef| {
        base()
            .item(ItemDef::new("soul_jar", "Soul Jar").consumable(Consumable::Give(Grant::Currency { currency: "souls".into(), amount: 3 })))
            .schedule(Schedule { rounds: vec![RoundDef::new("hunt", 1, hunt)], repeat_from: 0 })
            .build()
    };
    let mut g = start(build(hunt).unwrap());
    // Only the winner is rewarded.
    g.add_unit(P0, "ironbark_warden", vec![]);
    let uid = g.player(P0).bench.iter().flatten().last().unwrap().uid;
    g.apply(P0, Command::MoveUnit { uid, to: autoengine::game::Slot::Board(3, 4) }).unwrap();
    g.finish_round().unwrap();
    assert_eq!((g.currency(P0, "souls"), g.currency(P1, "souls")), (7, 5));
    // A single-use item hands over its grant and is gone.
    g.begin_round().unwrap();
    g.give_item(P0, "soul_jar");
    let item = g.player(P0).items.iter().position(|i| i == "soul_jar").unwrap();
    g.apply(P0, Command::EquipItem { item, uid }).unwrap();
    assert_eq!(g.currency(P0, "souls"), 10);
    assert!(!g.player(P0).items.contains(&"soul_jar".to_string()));

    // Rewards are checked like every other id.
    let typo = RoundKindDef::pve([BoardUnit::new("moss_golem", (3, 4))]).reward(Grant::Currency { currency: "sols".into(), amount: 2 });
    let message = build(typo).err().unwrap().to_string();
    assert!(message.contains("round `hunt` gives unknown currency `sols`") && message.contains("souls"), "{message}");
}

#[test]
fn a_unique_item_fits_once_and_a_unit_can_set_its_own_pool_size() {
    use autoengine::content::{ItemDef, UnitDef};
    let content = base()
        .item(ItemDef::new("crown", "Crown").unique())
        .unit(UnitDef::new("lone_king", "Lone King").cost(1).pool_size(1))
        .build()
        .unwrap();
    let mut g = start(content);
    assert_eq!(g.pool_count("lone_king") + g.players().iter().flat_map(|p| p.shop.iter().flatten()).filter(|u| *u == "lone_king").count() as u32, 1);
    g.add_unit(P0, "lone_king", vec![]);
    let uid = g.player(P0).bench.iter().flatten().last().unwrap().uid;
    g.give_item(P0, "crown");
    g.give_item(P0, "crown");
    g.apply(P0, Command::EquipItem { item: 0, uid }).unwrap();
    assert_eq!(g.apply(P0, Command::EquipItem { item: 0, uid }), Err(CommandError::CannotUse("a unit can hold only one of these".into())));
    assert_eq!(g.player(P0).unit(uid).unwrap().items, vec!["crown".to_string()]);
    assert_eq!(g.player(P0).items, vec!["crown".to_string()]);
}
