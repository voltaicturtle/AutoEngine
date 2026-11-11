//! Runs a sample fight headlessly and prints the event log.
//!
//! Usage: `autoengine [seed] [--quiet]`

use std::sync::Arc;

use autoengine::combat::{Combat, EventKind, Team, UnitId};
use autoengine::effect::{
    Ability, Amount, Anchor, DamageKind, Effect, EffectSpec, Passive, ScaleBy, Selector,
    StatusKind, Trigger, UnitTemplate, Who,
};
use autoengine::hex::{Hex, HexBoard};
use autoengine::stats::{Stat, StatBlock, StatMod};

/// A small original roster that exercises most of the effect system.
fn roster() -> [Arc<UnitTemplate>; 5] {
    let ironbark = UnitTemplate::new(
        "Ironbark Warden",
        StatBlock::base()
            .with(Stat::MaxHealth, 900)
            .with(Stat::AttackDamage, 45)
            .with(Stat::Armor, 40)
            .with(Stat::MagicResist, 30)
            .with(Stat::AttackSpeed, 600)
            .with(Stat::MaxMana, 80)
            .with(Stat::StartingMana, 30),
    )
    .with_tag("Grove")
    .with_ability(Ability {
        name: "Rootwall".into(),
        cast_time_ms: 300,
        effects: vec![
            EffectSpec::new(
                Selector::Source,
                Effect::Shield {
                    amount: Amount::power(250).plus(Who::Source, ScaleBy::Stat(Stat::MaxHealth), 100),
                    duration_ms: Some(4000),
                },
            ),
            EffectSpec::new(
                Selector::EnemiesWithin { anchor: Anchor::Source, radius: 1 },
                Effect::ApplyStatus { status: StatusKind::Stun, duration_ms: 1000 },
            ),
        ],
    })
    .with_passive(Passive {
        name: "Bramble Hide".into(),
        trigger: Trigger::OnDamaged,
        effects: vec![EffectSpec::new(
            Selector::EventOther,
            Effect::Damage { amount: Amount::flat(12), kind: DamageKind::Magic, can_crit: false },
        )],
        max_procs: None,
    });

    let emberlash = UnitTemplate::new(
        "Emberlash Duelist",
        StatBlock::base()
            .with(Stat::MaxHealth, 650)
            .with(Stat::AttackDamage, 65)
            .with(Stat::Armor, 25)
            .with(Stat::MagicResist, 20)
            .with(Stat::AttackSpeed, 850)
            .with(Stat::MaxMana, 60)
            .with(Stat::Lifesteal, 100),
    )
    .with_tag("Cinder")
    .with_ability(Ability {
        name: "Searing Flurry".into(),
        cast_time_ms: 250,
        effects: vec![
            EffectSpec::new(
                Selector::CurrentTarget,
                Effect::Damage {
                    amount: Amount::power(80).plus(Who::Source, ScaleBy::Stat(Stat::AttackDamage), 1500),
                    kind: DamageKind::Physical,
                    can_crit: true,
                },
            ),
            EffectSpec::new(
                Selector::Source,
                Effect::ApplyStatus {
                    status: StatusKind::StatMods(vec![StatMod::percent(Stat::AttackSpeed, 400)]),
                    duration_ms: 3000,
                },
            ),
        ],
    })
    .with_passive(Passive {
        name: "Last Spark".into(),
        trigger: Trigger::HealthBelow { permille: 400 },
        effects: vec![EffectSpec::new(
            Selector::Source,
            Effect::ModifyStats { mods: vec![StatMod::percent(Stat::AttackDamage, 300)] },
        )],
        max_procs: Some(1),
    });

    let stormcaller = UnitTemplate::new(
        "Tidewater Stormcaller",
        StatBlock::base()
            .with(Stat::MaxHealth, 500)
            .with(Stat::AttackDamage, 40)
            .with(Stat::Armor, 15)
            .with(Stat::MagicResist, 15)
            .with(Stat::Range, 4)
            .with(Stat::MaxMana, 70)
            .with(Stat::StartingMana, 20),
    )
    .with_tag("Tide")
    .with_ability(Ability {
        name: "Breaking Wave".into(),
        cast_time_ms: 500,
        effects: vec![
            EffectSpec::new(
                Selector::EnemiesWithin { anchor: Anchor::CurrentTarget, radius: 1 },
                Effect::Damage { amount: Amount::power(180), kind: DamageKind::Magic, can_crit: false },
            ),
            EffectSpec::new(
                Selector::EnemiesWithin { anchor: Anchor::CurrentTarget, radius: 1 },
                Effect::ApplyStatus {
                    status: StatusKind::DamageOverTime {
                        per_second: Amount::power(30),
                        kind: DamageKind::Magic,
                    },
                    duration_ms: 3000,
                },
            ),
        ],
    });

    let mender = UnitTemplate::new(
        "Dawnpetal Mender",
        StatBlock::base()
            .with(Stat::MaxHealth, 520)
            .with(Stat::AttackDamage, 35)
            .with(Stat::Armor, 15)
            .with(Stat::MagicResist, 25)
            .with(Stat::Range, 3)
            .with(Stat::MaxMana, 60),
    )
    .with_tag("Grove")
    .with_ability(Ability {
        name: "Morning Dew".into(),
        cast_time_ms: 400,
        effects: vec![EffectSpec::new(
            Selector::LowestHealthAllies(2),
            Effect::Heal {
                amount: Amount::power(150).plus(Who::Target, ScaleBy::MissingHealth, 150),
            },
        )],
    });

    let wisp = Arc::new(UnitTemplate::new(
        "Grave Wisp",
        StatBlock::base()
            .with(Stat::MaxHealth, 200)
            .with(Stat::AttackDamage, 30)
            .with(Stat::AttackSpeed, 900),
    ));
    let keeper = UnitTemplate::new(
        "Hollow Lantern Keeper",
        StatBlock::base()
            .with(Stat::MaxHealth, 700)
            .with(Stat::AttackDamage, 50)
            .with(Stat::Armor, 30)
            .with(Stat::MagicResist, 30),
    )
    .with_tag("Hollow")
    .with_passive(Passive {
        name: "Lantern's Release".into(),
        trigger: Trigger::OnDeath,
        effects: vec![EffectSpec::new(Selector::Source, Effect::Summon { template: wisp, count: 2 })],
        max_procs: None,
    });

    [ironbark, emberlash, stormcaller, mender, keeper].map(Arc::new)
}

fn main() {
    let mut seed = 1u64;
    let mut quiet = false;
    for arg in std::env::args().skip(1) {
        if arg == "--quiet" {
            quiet = true;
        } else if let Ok(s) = arg.parse() {
            seed = s;
        } else {
            eprintln!("usage: autoengine [seed] [--quiet]");
            std::process::exit(2);
        }
    }

    let [ironbark, emberlash, stormcaller, mender, keeper] = roster();
    let board = HexBoard::new(7, 8);
    let mut combat = Combat::new(board.clone(), seed);

    // Team 0 sets up on the bottom half; team 1's layout is mirrored onto the top.
    let home = [
        (ironbark.clone(), (3, 4)),
        (emberlash.clone(), (2, 4)),
        (stormcaller.clone(), (1, 7)),
        (mender.clone(), (4, 6)),
    ];
    let away = [
        (keeper, (3, 4)),
        (emberlash, (4, 4)),
        (stormcaller, (5, 7)),
        (ironbark, (2, 5)),
    ];
    for (t, (c, r)) in home {
        combat.spawn(t, Team(0), Hex::from_offset(c, r)).expect("valid setup");
    }
    for (t, (c, r)) in away {
        let pos = board.mirror(Hex::from_offset(c, r));
        combat.spawn(t, Team(1), pos).expect("valid setup");
    }

    combat.run();

    let label = |id: UnitId| {
        let u = combat.unit(id);
        format!("{}#{}[T{}]", u.name(), id.0, u.team().0)
    };
    if !quiet {
        for e in combat.events() {
            let t = e.tick * combat.config().tick_ms;
            let line = match &e.kind {
                EventKind::Spawned { unit, pos, .. } => {
                    format!("{} appears at {:?}", label(*unit), pos.to_offset())
                }
                EventKind::Moved { unit, to, .. } => {
                    format!("{} moves to {:?}", label(*unit), to.to_offset())
                }
                EventKind::Attacked { .. } => continue, // the damage line follows
                EventKind::Damaged { source, target, amount, absorbed, kind, crit } => format!(
                    "{} hits {} for {} {:?}{}{}",
                    label(*source),
                    label(*target),
                    amount,
                    kind,
                    if *crit { " (crit)" } else { "" },
                    if *absorbed > 0 { format!(" ({absorbed} shielded)") } else { String::new() },
                ),
                EventKind::Healed { target, amount } => format!("{} heals {}", label(*target), amount),
                EventKind::Shielded { target, amount } => {
                    format!("{} gains a {} shield", label(*target), amount)
                }
                EventKind::CastStarted { unit } => {
                    let name = combat.unit(*unit).template().ability.as_ref().map(|a| a.name.as_str());
                    format!("{} begins casting {}", label(*unit), name.unwrap_or("?"))
                }
                EventKind::CastFinished { unit } => format!("{} finishes casting", label(*unit)),
                EventKind::PassiveTriggered { unit, passive } => format!(
                    "{} triggers {}",
                    label(*unit),
                    combat.unit(*unit).template().passives[*passive].name
                ),
                EventKind::StatusApplied { target, status, duration_ms, .. } => {
                    let what = match status {
                        StatusKind::Stun => "stunned",
                        StatusKind::StatMods(_) => "stat-modified",
                        StatusKind::DamageOverTime { .. } => "afflicted",
                    };
                    format!("{} is {} for {}ms", label(*target), what, duration_ms)
                }
                EventKind::StatusExpired { .. } => continue,
                EventKind::Died { unit, killer } => {
                    format!("{} dies (killed by {})", label(*unit), label(*killer))
                }
                EventKind::Ended { winner, timed_out } => match (winner, timed_out) {
                    (Some(team), _) => format!("Team {} wins", team.0),
                    (None, true) => "Time ran out: draw".to_string(),
                    (None, false) => "Everyone fell: draw".to_string(),
                },
            };
            println!("[{:>6.2}s] {}", t as f64 / 1000.0, line);
        }
        println!();
    }

    let outcome = combat.outcome().unwrap();
    println!(
        "seed {seed}: {} after {:.2}s ({} events)",
        match outcome.winner {
            Some(team) => format!("team {} wins", team.0),
            None => "draw".to_string(),
        },
        outcome.duration_ms as f64 / 1000.0,
        combat.events().len()
    );
    for id in &outcome.survivors {
        let u = combat.unit(*id);
        println!("  survivor: {} with {}/{} health", label(*id), u.health(), u.stat(Stat::MaxHealth));
    }
}
