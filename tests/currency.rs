//! Currencies players hold besides gold, and the sell refund rule.

use std::collections::BTreeMap;
use std::sync::Arc;

use autoengine::content::{AugmentDef, ContentSet, CurrencyDef, RoundDef, RoundGrant, RoundKindDef, Schedule};
use autoengine::game::{Command, CommandError, Game, GameEvent, Grant, PlayerId};
use autoengine::protocol::DevCommand;

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);

/// The sample game with a `souls` currency: one to start, two a round.
fn with_souls() -> ContentSet {
    autoengine::sample::content()
        .into_builder()
        .currency(CurrencyDef::new("souls", "Souls").starting(1).per_round(2).color([150, 90, 200]))
        .build()
        .unwrap()
}

#[test]
fn players_start_with_a_currency_and_are_paid_each_round() {
    let mut g = Game::new(Arc::new(with_souls()), 1, &["a", "b"]);
    assert_eq!(g.currency(P0, "souls"), 1);
    assert_eq!(g.player(P1).wallet.get("souls"), Some(&1));
    g.begin_round().unwrap();
    // The first round pays nothing, like gold income.
    let first = g.currency(P0, "souls");
    g.finish_round().unwrap();
    g.begin_round().unwrap();
    assert_eq!(g.currency(P0, "souls"), first + 2);
    assert!(g.events().iter().any(|e| matches!(
        e,
        GameEvent::CurrencyChanged { player, currency, amount } if *player == P0 && currency == "souls" && *amount == first + 2
    )));
}

#[test]
fn a_currency_can_be_given_spent_and_never_goes_negative() {
    let mut g = Game::new(Arc::new(with_souls()), 1, &["a", "b"]);
    g.add_currency(P0, "souls", 4);
    assert_eq!(g.currency(P0, "souls"), 5);
    // Spending takes nothing when there is not enough.
    let err = g.spend_currency(P0, "souls", 6).unwrap_err();
    assert_eq!(err, CommandError::NotEnough { currency: "souls".into(), need: 6, have: 5 });
    assert_eq!(err.to_string(), "needs 6 souls, has 5");
    assert_eq!(g.currency(P0, "souls"), 5);
    g.spend_currency(P0, "souls", 3).unwrap();
    assert_eq!(g.currency(P0, "souls"), 2);
    // Taking more than there is stops at zero.
    g.add_currency(P0, "souls", -10);
    assert_eq!(g.currency(P0, "souls"), 0);
    // A currency the game does not define is ignored or refused.
    g.add_currency(P0, "gems", 5);
    assert_eq!(g.currency(P0, "gems"), 0);
    assert_eq!(g.spend_currency(P0, "gems", 0), Err(CommandError::UnknownCurrency));
    assert_eq!(g.player(P1).wallet.get("souls"), Some(&1));
}

#[test]
fn grants_and_dev_commands_hand_out_currencies() {
    let mut g = Game::new(Arc::new(with_souls()), 1, &["a", "b"]);
    g.grant(P0, Grant::Currency { currency: "souls".into(), amount: 7 }).unwrap();
    assert_eq!(g.currency(P0, "souls"), 8);
    assert_eq!(g.grant(P0, Grant::Currency { currency: "sols".into(), amount: 7 }), Err(CommandError::UnknownCurrency));
    assert_eq!(DevCommand::parse("give souls 3"), Ok(DevCommand::Currency("souls".into(), 3)));
    assert!(DevCommand::parse("give souls").is_err());
    assert!(DevCommand::parse("give").is_err());
}

#[test]
fn an_augment_can_pay_a_currency_every_round() {
    let content = with_souls()
        .into_builder()
        .augment(AugmentDef::new("reaper", "Reaper").per_round(RoundGrant::default().currency("souls", 3)))
        .build()
        .unwrap();
    let mut g = Game::new(Arc::new(content), 3, &["a", "b"]);
    g.begin_round().unwrap();
    g.grant(P0, Grant::Augment("reaper".into())).unwrap();
    g.finish_round().unwrap();
    let (a, b) = (g.currency(P0, "souls"), g.currency(P1, "souls"));
    g.begin_round().unwrap();
    assert_eq!(g.currency(P1, "souls") - b, 2);
    assert_eq!(g.currency(P0, "souls") - a, 2 + 3);
}

#[test]
fn currencies_are_validated_and_can_be_translated() {
    let base = || autoengine::sample::content().into_builder();
    let err = |r: Result<ContentSet, autoengine::content::ContentError>| r.err().expect("should not build").to_string();
    assert!(err(base().currency(CurrencyDef::new("gold", "Gold")).build()).contains("`gold` is taken"));
    assert!(err(base().currency(CurrencyDef::new("", "Nothing")).build()).contains("needs an id"));
    // Adding the same id again replaces it instead of clashing.
    let twice = base().currency(CurrencyDef::new("souls", "Souls")).currency(CurrencyDef::new("souls", "Spirits").per_round(1)).build().unwrap();
    assert_eq!(twice.currencies().iter().filter(|c| c.id == "souls").count(), 1);
    assert_eq!(twice.currency("souls").unwrap().name, "Spirits");
    // An augment that pays a currency nobody defined is a typo.
    let typo = with_souls().into_builder().augment(AugmentDef::new("reaper", "Reaper").per_round(RoundGrant::default().currency("soul", 3))).build();
    let message = err(typo);
    assert!(message.contains("unknown currency `soul`"), "{message}");
    assert!(message.contains("souls"), "{message}");

    let content = with_souls();
    assert!(content.text_keys().contains(&"currency.souls.name".to_string()));
    assert_eq!(content.text(None, "currency", "souls", "name").as_deref(), Some("Souls"));
    let translated = content.with_translations(BTreeMap::from([("de".to_string(), BTreeMap::from([("currency.souls.name".to_string(), "Seelen".to_string())]))])).unwrap();
    assert_eq!(translated.text(Some("de"), "currency", "souls", "name").as_deref(), Some("Seelen"));
}

#[cfg(feature = "scripting")]
#[test]
fn scripts_read_give_and_take_currencies() {
    let source = r#"
        fn finish(game) {
            game.give(0, "souls", 4);
            game.log(`has ${game.amount(0, "souls")}`);
            game.log(`took 3: ${game.take(0, "souls", 3)}`);
            game.log(`took 9: ${game.take(0, "souls", 9)}`);
            game.log(`left ${game.amount(0, "souls")}, gems ${game.amount(0, "gems")}`);
        }
    "#;
    let round = RoundDef {
        name: "tithe".into(),
        stage: 1,
        kind: RoundKindDef::Custom { kind: "tithe".into(), params: BTreeMap::new() },
        tiles: Vec::new(),
    };
    let content = with_souls()
        .with_scripts(BTreeMap::from([("tithe".to_string(), source.to_string())]))
        .unwrap()
        .with_schedule(Schedule { rounds: vec![round], repeat_from: 0 })
        .unwrap();
    let mut g = Game::new(Arc::new(content), 1, &["a", "b"]);
    g.begin_round().unwrap();
    let before = g.currency(P0, "souls");
    g.finish_round().unwrap();
    let logs: Vec<String> = g
        .events()
        .iter()
        .filter_map(|e| match e {
            GameEvent::Script { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(logs, vec![format!("has {}", before + 4), "took 3: true".to_string(), "took 9: false".to_string(), format!("left {}, gems 0", before + 1)]);
    assert_eq!(g.currency(P0, "souls"), before + 1);
    assert_eq!(g.currency(P1, "souls"), before);
}

#[test]
fn the_sell_refund_is_a_rule() {
    let sell = |permille: u32| {
        let base = autoengine::sample::content();
        let mut rules = base.rules().clone();
        rules.sell_refund_permille = permille;
        let content = base.with_game_rules(rules).unwrap();
        let unit = content.units().find(|u| u.in_shop && u.cost >= 2).unwrap();
        let (id, cost) = (unit.id.clone(), unit.cost as i64);
        let mut g = Game::new(Arc::new(content), 1, &["a", "b"]);
        g.begin_round().unwrap();
        g.add_unit(P0, &id, vec![]);
        let uid = g.player(P0).bench.iter().flatten().last().unwrap().uid;
        let gold = g.player(P0).gold;
        g.apply(P0, Command::SellUnit { uid }).unwrap();
        (g.player(P0).gold - gold, cost)
    };
    let (full, cost) = sell(1000);
    assert_eq!(full, cost);
    assert_eq!(sell(500).0, cost / 2);
    assert_eq!(sell(0).0, 0);
}
