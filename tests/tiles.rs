//! Tiles: effects that belong to a hex.

use std::sync::Arc;

use autoengine::combat::{Combat, CombatConfig, EventKind, SpawnError, Team, UnitId};
use autoengine::content::{BoardUnit, ContentSet, RoundDef, RoundKindDef, Schedule, TeamSetup, UnitDef};
use autoengine::effect::{Ability, Condition, Effect, Passive, Selector, Trigger, UnitTemplate, Who};
use autoengine::game::{Game, Opponent, PlayerId};
use autoengine::hex::{Hex, HexBoard};
use autoengine::playback::Playback;
use autoengine::stats::{Stat, StatBlock};
use autoengine::tile::TileDef;
use autoengine::tools::lint;

fn at(col: i32, row: i32) -> Hex {
    Hex::from_offset(col, row)
}

/// No crits, 100 attack damage, one attack per second (every 20 ticks).
fn hitter(name: &str) -> UnitTemplate {
    UnitTemplate::new(name, StatBlock::base().with(Stat::CritChance, 0).with(Stat::AttackDamage, 100).with(Stat::AttackSpeed, 1000))
}

/// Lots of health, no resistances, does no damage.
fn dummy(name: &str) -> UnitTemplate {
    UnitTemplate::new(name, StatBlock::base().with(Stat::MaxHealth, 100_000).with(Stat::AttackDamage, 0))
}

fn fight(tiles: Vec<TileDef>) -> Combat {
    Combat::with_config(HexBoard::new(7, 8), 1, CombatConfig { overtime_after_ms: 0, tiles, ..CombatConfig::default() })
}

fn steps(c: &mut Combat, n: u32) {
    for _ in 0..n {
        c.step();
    }
}

fn lost(c: &Combat, id: UnitId) -> i64 {
    c.unit(id).stat(Stat::MaxHealth) - c.unit(id).health()
}

fn embers() -> TileDef {
    TileDef::new("embers", "Embers").enemies_only().per_second(Effect::true_damage(40)).color([230, 90, 40])
}

#[test]
fn a_tile_hurts_whoever_stands_on_it_every_second() {
    let mut c = fight(vec![TileDef::new("lava", "Lava").per_second(Effect::true_damage(40))]);
    let a = c.spawn(dummy("a"), Team(0), at(3, 4)).unwrap();
    let b = c.spawn(dummy("b"), Team(1), at(3, 3)).unwrap();
    // Nobody laid it, so it has no side.
    assert!(c.place_tile("lava", at(3, 4), None, None));
    assert!(!c.place_tile("ice", at(3, 4), None, None));
    assert!(!c.place_tile("lava", at(30, 4), None, None));
    steps(&mut c, 19);
    assert_eq!(lost(&c, a), 0);
    steps(&mut c, 1);
    assert_eq!((lost(&c, a), lost(&c, b)), (40, 0));
    steps(&mut c, 40);
    assert_eq!(lost(&c, a), 120);
    let tiles = c.tiles();
    assert_eq!(
        (tiles.len(), tiles[0].def.id.as_str(), tiles[0].pos, tiles[0].team, tiles[0].remaining_ms),
        (1, "lava", at(3, 4), None, None)
    );
}

#[test]
fn standing_on_a_tile_changes_stats_until_the_unit_leaves() {
    let shrine = TileDef::new("shrine", "Shrine").mods([Stat::AttackDamage.flat(50), Stat::MaxHealth.flat(100)]);
    let mut c = fight(vec![shrine]);
    // The hitter starts on the shrine and has to walk off it to reach its target.
    let a = c.spawn(hitter("a"), Team(0), at(3, 7)).unwrap();
    c.spawn(dummy("b"), Team(1), at(3, 0)).unwrap();
    c.place_tile("shrine", at(3, 7), None, None);
    assert_eq!(c.unit(a).stat(Stat::AttackDamage), 100, "tiles do nothing before the fight starts");
    c.step();
    assert_eq!((c.unit(a).stat(Stat::AttackDamage), c.unit(a).stat(Stat::MaxHealth), c.unit(a).health()), (150, 600, 600));
    while c.unit(a).pos() == at(3, 7) {
        c.step();
    }
    assert_eq!((c.unit(a).stat(Stat::AttackDamage), c.unit(a).stat(Stat::MaxHealth), c.unit(a).health()), (100, 500, 500));
}

#[test]
fn stepping_on_and_off_a_tile_runs_its_effects() {
    let trap = TileDef::new("trap", "Trap").on_enter(Effect::true_damage(10)).on_leave(Effect::true_damage(3));
    let mut c = fight(vec![trap]);
    let a = c.spawn(hitter("a").with_stat(Stat::MaxHealth, 100_000), Team(0), at(3, 7)).unwrap();
    // The target stays put, so the walker has to come all the way.
    c.spawn(dummy("b").with_stat(Stat::MoveSpeed, 0), Team(1), at(3, 0)).unwrap();
    // A whole row of traps between the two: the walker crosses exactly one.
    for col in 0..7 {
        c.place_tile("trap", at(col, 4), None, None);
    }
    let mut seen = Vec::new();
    for _ in 0..200 {
        c.step();
        if seen.last() != Some(&lost(&c, a)) {
            seen.push(lost(&c, a));
        }
    }
    assert_eq!(seen, vec![0, 10, 13]);
}

#[test]
fn an_ability_lays_tiles_that_only_hurt_the_other_side() {
    let mut c = fight(vec![embers()]);
    let scorch =
        Passive::new("Scorch", Trigger::CombatStart).effect(Effect::tile_for("embers", 2000).spread(1).to(Selector::CurrentTarget));
    let a = c.spawn(dummy("a").with_passive(scorch), Team(0), at(3, 4)).unwrap();
    let b = c.spawn(dummy("b"), Team(1), at(3, 3)).unwrap();
    steps(&mut c, 1);
    // The target's hex and the six around it, the caster's own included.
    assert_eq!(c.tiles().len(), 7);
    assert!(c.has_tile(at(3, 4), "embers") && c.tiles().iter().all(|t| t.team == Some(Team(0))));
    steps(&mut c, 19);
    assert_eq!((lost(&c, a), lost(&c, b)), (0, 40));
    // Two seconds means two pulses, and then the tiles are gone.
    steps(&mut c, 20);
    assert_eq!((lost(&c, a), lost(&c, b)), (0, 80));
    assert!(c.tiles().is_empty());
    steps(&mut c, 40);
    assert_eq!(lost(&c, b), 80);
    let count = |want: fn(&EventKind) -> bool| c.events().iter().filter(|e| want(&e.kind)).count();
    assert_eq!(count(|k| matches!(k, EventKind::TilePlaced { duration_ms: Some(2000), .. })), 7);
    assert_eq!(count(|k| matches!(k, EventKind::TileRemoved { .. })), 7);

    // The events are enough for a front end to draw them.
    let mut playback = Playback::from_events(c.events().to_vec(), 50);
    playback.advance_to(500);
    assert_eq!(playback.tiles().len(), 7);
    assert_eq!(
        (playback.tiles()[0].tile.as_str(), playback.tiles()[0].team, playback.tiles()[0].until_ms),
        ("embers", Some(Team(0)), Some(2000))
    );
    playback.advance_to(2100);
    assert!(playback.tiles().is_empty());
}

#[test]
fn laying_a_tile_again_keeps_the_longer_time() {
    let mut c = fight(vec![embers()]);
    c.spawn(dummy("a"), Team(0), at(3, 4)).unwrap();
    c.spawn(dummy("b"), Team(1), at(3, 3)).unwrap();
    c.place_tile("embers", at(0, 0), None, Some(1000));
    steps(&mut c, 10);
    c.place_tile("embers", at(0, 0), None, Some(300));
    assert_eq!(c.tiles()[0].remaining_ms, Some(500));
    c.place_tile("embers", at(0, 0), None, Some(2000));
    assert_eq!((c.tiles().len(), c.tiles()[0].remaining_ms), (1, Some(2000)));
    assert!(c.clear_tile("embers", at(0, 0)));
    assert!(!c.clear_tile("embers", at(0, 0)));
    assert!(c.tiles().is_empty());
}

#[test]
fn a_blocking_tile_cannot_be_stood_on_and_units_walk_around_it() {
    let mut c = fight(vec![TileDef::new("boulder", "Boulder").blocking()]);
    let a = c.spawn(hitter("a"), Team(0), at(0, 7)).unwrap();
    let b = c.spawn(dummy("b"), Team(1), at(0, 0)).unwrap();
    // A wall across the board with one gap, on the far side.
    for col in 0..6 {
        assert!(c.place_tile("boulder", at(col, 4), None, None));
    }
    assert!(!c.can_stand(at(2, 4)) && c.can_stand(at(6, 4)));
    assert_eq!(c.spawn(dummy("c"), Team(0), at(2, 4)).err(), Some(SpawnError::Blocked(at(2, 4))));
    // A boulder is not dropped on someone's head.
    assert!(!c.place_tile("boulder", at(0, 7), None, None));
    let mut path = Vec::new();
    for _ in 0..400 {
        c.step();
        if path.last() != Some(&c.unit(a).pos()) {
            path.push(c.unit(a).pos());
        }
    }
    assert!(path.contains(&at(6, 4)), "the only way through is the gap: {path:?}");
    assert!(path.iter().all(|h| h.to_offset().1 != 4 || *h == at(6, 4)));
    assert!(lost(&c, b) > 0);
}

#[test]
fn a_condition_can_ask_what_a_unit_stands_on() {
    let mark = TileDef::new("sigil", "Sigil");
    let mut c = fight(vec![mark]);
    let smite = Passive::new("Smite", Trigger::CombatStart)
        .effect(Effect::true_damage(100).to(Selector::AllEnemies).when(Condition::OnTile { who: Who::Target, tile: "sigil".into() }));
    c.spawn(dummy("a").with_passive(smite), Team(0), at(3, 7)).unwrap();
    let on = c.spawn(dummy("on"), Team(1), at(2, 0)).unwrap();
    let off = c.spawn(dummy("off"), Team(1), at(4, 0)).unwrap();
    c.place_tile("sigil", at(2, 0), None, None);
    steps(&mut c, 1);
    assert_eq!((lost(&c, on), lost(&c, off)), (100, 0));
}

#[test]
fn dying_and_coming_back_on_a_tile_keeps_its_stats_right() {
    let shrine = TileDef::new("shrine", "Shrine").mods([Stat::Armor.flat(25)]);
    let mut c = fight(vec![shrine]);
    let second_wind = Passive::new("Second Wind", Trigger::OnDeath).max_procs(1).effect(Effect::revive(50).to(Selector::Source));
    let phoenix =
        UnitTemplate::new("phoenix", StatBlock::base().with(Stat::MaxHealth, 60).with(Stat::AttackDamage, 0)).with_passive(second_wind);
    let a = c.spawn(phoenix, Team(0), at(3, 4)).unwrap();
    c.spawn(hitter("killer"), Team(1), at(3, 3)).unwrap();
    c.place_tile("shrine", at(3, 4), None, None);
    let mut armor = Vec::new();
    while !c.events().iter().any(|e| matches!(e.kind, EventKind::Revived { .. })) {
        c.step();
        armor.push(c.unit(a).stat(Stat::Armor));
    }
    // 25 while alive, back to 0 when dead, and 25 again (not 50) after.
    assert_eq!(armor.first(), Some(&25));
    assert_eq!(c.unit(a).stat(Stat::Armor), 25);
    c.run();
    assert_eq!(c.unit(a).stat(Stat::Armor), 0);
}

/// The sample game with two tiles and one fight that starts with a shrine.
fn game_content() -> ContentSet {
    let round = RoundDef::new("duel", 1, RoundKindDef::pvp()).tile("shrine", (3, 4)).tile_at("embers", (0, 0));
    autoengine::sample::content()
        .into_builder()
        .tile(TileDef::new("shrine", "Shrine").description("Whoever stands here hits harder.").mods([Stat::AttackDamage.percent(200)]))
        .tile(embers())
        .schedule(Schedule { rounds: vec![round], repeat_from: 0 })
        .build()
        .unwrap()
}

#[test]
fn a_round_starts_its_fights_with_tiles_and_they_replay() {
    let content = Arc::new(game_content());
    let mut g = Game::new(content.clone(), 5, &["a", "b"]);
    g.begin_round().unwrap();
    for id in [PlayerId(0), PlayerId(1)] {
        g.add_unit(id, "ironbark_warden", vec![]);
    }
    g.finish_round().unwrap();
    let record = g.combats().iter().find(|c| matches!(c.away, Opponent::Player(_))).expect("the two fought").clone();
    assert_eq!(record.tiles.len(), 2);
    // The shrine is laid for both sides, the embers on one hex only.
    let laid: Vec<(String, (i32, i32))> = record
        .events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::TilePlaced { tile, pos, team: None, duration_ms: None } => Some((tile.clone(), pos.to_offset())),
            _ => None,
        })
        .collect();
    assert_eq!(laid, vec![("shrine".to_string(), (3, 4)), ("shrine".to_string(), (3, 3)), ("embers".to_string(), (0, 0))]);
    // A client that gets only the teams, tiles and seed fights the same fight.
    let again = autoengine::replay::fight_again(&content, &record, record.events.len()).unwrap();
    assert_eq!(again, record.events);

    // Outside a game, a fight is given its tiles directly.
    let team = || TeamSetup::new().unit(BoardUnit::new("ironbark_warden", (3, 4)));
    let placement = record.tiles[0].clone();
    let mut combat = content.setup_combat_with(1, &[team(), team()], &[placement]).unwrap();
    combat.step();
    assert_eq!(combat.tiles().len(), 2);
    let warden = &combat.units()[0];
    assert_eq!(
        warden.stat(Stat::AttackDamage),
        content.base_template("ironbark_warden", 1).unwrap().stats.get(Stat::AttackDamage) * 12 / 10
    );
}

#[test]
fn clients_are_told_the_tiles_of_the_round() {
    use autoengine::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
    use autoengine::session::{Session, SessionConfig};

    let content = Arc::new(game_content());
    let config = SessionConfig { seats: 2, seed: 7, planning_ms: 30_000, wait_for_playback: false, after_combat_ms: 0 };
    let mut s = Session::new(content.clone(), config);
    s.handle(1, ClientMsg::Join { name: "Ana".into(), version: PROTOCOL_VERSION, token: None }, 0);
    s.handle(1, ClientMsg::Start, 0);
    let view = s
        .drain_outbox()
        .into_iter()
        .rev()
        .find_map(|(_, m)| match m {
            ServerMsg::State(view) => Some(view),
            _ => None,
        })
        .expect("a state after the game starts");
    assert_eq!(view.round_tiles, content.schedule().round(0).0.tiles);
    assert_eq!(view.round_tiles.len(), 2);
    // An update only repeats them when they change.
    let mut later = view.clone();
    later.stage += 1;
    assert_eq!(view.diff(&later).unwrap().round_tiles, None);
    later.round_tiles.clear();
    let update = view.diff(&later).unwrap();
    assert_eq!(update.round_tiles, Some(vec![]));
    let mut patched = view.clone();
    patched.patch(update);
    assert_eq!(patched, later);
}

#[test]
fn tiles_are_validated_translated_and_linted() {
    let base = || autoengine::sample::content().into_builder();
    let err = |b: autoengine::content::GameBuilder| b.build().err().expect("should not build").to_string();
    let caster = |effect: Effect| {
        UnitDef::new("pyre", "Pyre")
            .cost(1)
            .stat(Stat::MaxMana, 50)
            .ability(Ability::new("Scorch").effect(effect.to(Selector::CurrentTarget)))
    };
    let message = err(base().tile(embers()).unit(caster(Effect::tile("ember"))));
    assert!(message.contains("refers to unknown tile `ember`") && message.contains("embers"), "{message}");
    assert!(err(base().tile(TileDef::new("", "Nothing"))).contains("a tile needs an id"));
    assert!(err(base().tile(TileDef::new("wall", "Wall").blocking().per_second(Effect::true_damage(1)))).contains("blocks its hex"));
    assert!(err(base().tile(TileDef::new("mist", "Mist").on_enter(Effect::resource("steam", 5)))).contains("unknown resource `steam`"));
    let round = |tile: &str, pos| Schedule { rounds: vec![RoundDef::new("duel", 1, RoundKindDef::pvp()).tile(tile, pos)], repeat_from: 0 };
    assert!(err(base().tile(embers()).schedule(round("embers", (9, 9)))).contains("off the board"));
    assert!(err(base().schedule(round("mist", (3, 4)))).contains("round `duel` refers to unknown tile `mist`"));
    let condition = Effect::true_damage(1).to(Selector::CurrentTarget).when(Condition::OnTile { who: Who::Target, tile: "embres".into() });
    let unit = UnitDef::new("pyre", "Pyre").cost(1).stat(Stat::MaxMana, 50).ability(Ability::new("Scorch").effect(condition));
    assert!(err(base().tile(embers()).unit(unit)).contains("unknown tile `embres`"));
    // The same id again replaces the tile.
    let twice = base().tile(embers()).tile(TileDef::new("embers", "Cinders")).unit(caster(Effect::tile("embers"))).build().unwrap();
    assert_eq!(twice.tiles().iter().filter(|t| t.id == "embers").count(), 1);
    assert_eq!(twice.tile("embers").unwrap().name, "Cinders");
    assert!(twice.text_keys().contains(&"tile.embers.description".to_string()));
    assert_eq!(twice.text(None, "tile", "embers", "name").as_deref(), Some("Cinders"));

    // A tile nothing lays is probably a mistake.
    let unused = base().tile(TileDef::new("mist", "Mist")).build().unwrap();
    assert!(lint(&unused, None).iter().any(|w| w.subject == "tile mist" && w.message.contains("nothing in the content lays it")));
    assert!(!lint(&twice, None).iter().any(|w| w.subject == "tile embers"));
    assert!(!lint(&autoengine::sample::content(), None).iter().any(|w| w.subject.starts_with("tile")));
}
