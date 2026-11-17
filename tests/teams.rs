//! Playing in teams, and shop offers that come with something extra.

use std::sync::Arc;

use autoengine::content::{ContentSet, GameRules, RoundDef, RoundKindDef, Schedule, ShopSpecial};
use autoengine::game::{Command, CommandError, Game, GameEvent, Opponent, PlayerId};

fn teams_of_two() -> ContentSet {
    let base = autoengine::sample::content();
    let rules = GameRules { team_size: 2, ..base.rules().clone() };
    base.into_builder().rules(rules).schedule(Schedule::new([RoundDef::new("duel", 3, RoundKindDef::pvp())])).build().unwrap()
}

fn p(i: u8) -> PlayerId {
    PlayerId(i)
}

#[test]
fn teammates_share_health_and_never_fight_each_other() {
    let content = Arc::new(teams_of_two());
    let mut g = Game::new(content.clone(), 3, &["a", "b", "c", "d", "e", "f"]);
    assert_eq!((g.team_of(p(0)), g.team_of(p(1)), g.team_of(p(2)), g.team_of(p(5))), (0, 0, 1, 2));
    assert!(g.teammates(p(0), p(1)) && !g.teammates(p(1), p(2)) && !g.teammates(p(0), p(0)));
    for round in 0..6 {
        g.begin_round().unwrap();
        // Only the first team fields anything, so everyone else loses.
        for id in [p(0), p(1)] {
            if round == 0 {
                g.add_unit(id, "ironbark_warden", vec![]);
            }
        }
        g.finish_round().unwrap();
        for c in g.combats() {
            if let Opponent::Player(other) | Opponent::Ghost(other) = c.away {
                assert!(!g.teammates(c.home, other), "round {round}: {:?} fought a teammate", c.home);
            }
        }
        for team in [[0, 1], [2, 3], [4, 5]] {
            assert_eq!(g.player(p(team[0])).health, g.player(p(team[1])).health, "round {round}");
        }
        if g.is_over() {
            break;
        }
    }
    // Healing one heals the team too.
    let mut g = Game::new(content, 3, &["a", "b", "c", "d"]);
    g.begin_round().unwrap();
    g.damage_player(p(0), 10);
    assert_eq!((g.player(p(0)).health, g.player(p(1)).health, g.player(p(2)).health), (90, 90, 100));
    g.heal_player(p(1), 4);
    assert_eq!((g.player(p(0)).health, g.player(p(1)).health), (94, 94));
    assert_eq!(g.team_members(p(1)), vec![p(0), p(1)]);
}

#[test]
fn teams_go_out_together_and_place_together() {
    let mut g = Game::new(Arc::new(teams_of_two()), 5, &["a", "b", "c", "d", "e", "f"]);
    g.begin_round().unwrap();
    g.damage_player(p(2), 1000);
    g.damage_player(p(4), 50);
    g.finish_round().unwrap();
    assert!(!g.player(p(2)).alive && !g.player(p(3)).alive);
    assert_eq!((g.player(p(2)).placement, g.player(p(3)).placement), (Some(3), Some(3)));
    assert_eq!(g.alive_teams(), vec![0, 2]);
    g.begin_round().unwrap();
    let left = g.player(p(4)).health;
    g.damage_player(p(5), left);
    g.finish_round().unwrap();
    assert!(g.is_over());
    let places: Vec<Option<u32>> = g.players().iter().map(|pl| pl.placement).collect();
    assert_eq!(places, vec![Some(1), Some(1), Some(3), Some(3), Some(2), Some(2)]);
    assert_eq!(g.winner(), Some(p(0)));
}

#[test]
fn a_player_can_send_a_teammate_a_unit() {
    let mut g = Game::new(Arc::new(teams_of_two()), 5, &["a", "b", "c", "d"]);
    g.begin_round().unwrap();
    g.add_unit(p(0), "ashen_hound", vec!["whetstone".into()]);
    let uid = g.player(p(0)).units().next().unwrap().uid;
    assert_eq!(g.apply(p(0), Command::Gift { uid, to: p(2) }), Err(CommandError::NotATeammate));
    assert_eq!(g.apply(p(0), Command::Gift { uid, to: p(0) }), Err(CommandError::NotATeammate));
    assert_eq!(g.apply(p(0), Command::Gift { uid: 999, to: p(1) }), Err(CommandError::UnknownUnit));
    // Two copies are waiting on the other side, so the gift makes a two-star.
    g.add_unit(p(1), "ashen_hound", vec![]);
    g.add_unit(p(1), "ashen_hound", vec![]);
    g.apply(p(0), Command::Gift { uid, to: p(1) }).unwrap();
    assert_eq!(g.player(p(0)).units().count(), 0);
    let hound = g.player(p(1)).units().next().unwrap();
    assert_eq!((hound.star, hound.items.clone()), (2, vec!["whetstone".to_string()]));
    assert!(g.events().iter().any(|e| matches!(e, GameEvent::UnitGifted { from, to, .. } if *from == p(0) && *to == p(1))));

    // In a game without teams nobody is a teammate.
    let mut solo = Game::new(Arc::new(autoengine::sample::content()), 5, &["a", "b"]);
    solo.begin_round().unwrap();
    solo.add_unit(p(0), "ashen_hound", vec![]);
    let uid = solo.player(p(0)).units().next().unwrap().uid;
    assert_eq!(solo.apply(p(0), Command::Gift { uid, to: p(1) }), Err(CommandError::NotATeammate));
    let bad = GameRules { team_size: 0, ..autoengine::sample::content().rules().clone() };
    assert!(autoengine::sample::content().into_builder().rules(bad).build().unwrap_err().to_string().contains("team size of 0"));
}

#[test]
fn some_shop_offers_come_with_a_higher_star_or_an_extra_trait() {
    let base = autoengine::sample::content();
    let specials = vec![ShopSpecial::new(1000).star(2), ShopSpecial::new(1000).from_level(1).with_trait_from(["tide"])];
    let rules = GameRules { shop_specials: specials.clone(), ..base.rules().clone() };
    let content = Arc::new(base.clone().into_builder().rules(rules).build().unwrap());
    let mut g = Game::new(content.clone(), 9, &["a", "b"]);
    g.begin_round().unwrap();
    let pool_total = |g: &Game| -> u32 { g.pool().values().sum() };
    // Every offer is a two-star: three copies each, held out of the pool.
    let shop = g.player(p(0)).shop.clone();
    let offers = g.player(p(0)).shop_specials.clone();
    assert!(offers.iter().zip(&shop).all(|(o, u)| u.is_none() || o.as_ref().is_some_and(|o| o.star == 2)));
    let unit = shop[0].clone().unwrap();
    let cost = content.unit(&unit).unwrap().cost as i64;
    assert_eq!(g.shop_price(p(0), 0), Some(cost * 3));
    g.add_gold(p(0), 100);
    let gold = g.player(p(0)).gold;
    g.apply(p(0), Command::BuyUnit { slot: 0 }).unwrap();
    let bought = g.player(p(0)).units().next().unwrap().clone();
    assert_eq!((bought.unit.clone(), bought.star), (unit.clone(), 2));
    assert_eq!(g.player(p(0)).gold, gold - cost * 3);
    // Selling it gives back what three copies are worth, and all three go
    // back to the pool; rerolling puts the held copies back too.
    let before = pool_total(&g);
    g.apply(p(0), Command::SellUnit { uid: bought.uid }).unwrap();
    assert_eq!(pool_total(&g), before + 3);
    let held = g.player(p(0)).shop.iter().flatten().count() as u32 * 3;
    let before = pool_total(&g);
    g.apply(p(0), Command::Reroll).unwrap();
    let held_now = g.player(p(0)).shop.iter().flatten().count() as u32 * 3;
    assert_eq!(pool_total(&g), before + held - held_now);

    // An extra trait counts for the unit's traits.
    let rules = GameRules { shop_specials: vec![specials[1].clone()], ..base.rules().clone() };
    let content = Arc::new(base.clone().into_builder().rules(rules).build().unwrap());
    let mut g = Game::new(content.clone(), 9, &["a", "b"]);
    g.begin_round().unwrap();
    g.add_gold(p(0), 100);
    let slot = (0..5).find(|i| g.player(p(0)).shop_specials[*i].is_some()).expect("a special offer");
    let unit = g.player(p(0)).shop[slot].clone().unwrap();
    assert!(!content.unit(&unit).unwrap().traits.contains(&"tide".to_string()));
    // Units that already have the trait are never given it again.
    for (offer, special) in g.player(p(0)).shop.iter().zip(&g.player(p(0)).shop_specials) {
        if let (Some(u), None) = (offer, special) {
            assert!(content.unit(u).unwrap().traits.contains(&"tide".to_string()));
        }
    }
    g.apply(p(0), Command::BuyUnit { slot }).unwrap();
    let bought = g.player(p(0)).units().next().unwrap().clone();
    assert_eq!(bought.traits, vec!["tide".to_string()]);
    let traits = content.active_traits(&[autoengine::content::BoardUnit::new(&bought.unit, (0, 4)).with_trait("tide")]).unwrap();
    assert!(traits.iter().any(|t| t.id == "tide"));

    let bad = GameRules { shop_specials: vec![ShopSpecial::new(10).with_trait_from(["tidal"])], ..base.rules().clone() };
    assert!(base.clone().into_builder().rules(bad).build().unwrap_err().to_string().contains("unknown trait `tidal`"));
    let bad = GameRules { shop_specials: vec![ShopSpecial::new(10).star(9)], ..base.rules().clone() };
    assert!(base.into_builder().rules(bad).build().unwrap_err().to_string().contains("9 star units"));
}
