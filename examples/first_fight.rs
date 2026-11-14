//! The smallest useful program: load content, set up two teams, fight.
//!
//!     cargo run --example first_fight

use autoengine::prelude::*;

fn main() {
    // A content folder holds the whole game as data files.
    let content = ContentSet::load_dir("content").expect("run this from the repository root");

    // A team is a list of units and where they stand, from that team's own
    // side of the board: row 4 is the front line, row 7 the back.
    let unit = |id: &str, star, pos| BoardUnit { unit: id.into(), star, items: vec![], pos };
    let home = TeamSetup {
        units: vec![unit("ironbark_warden", 2, (3, 4)), unit("dawnpetal_mender", 1, (3, 7))],
        bonuses: vec![],
    };
    let away = TeamSetup {
        units: vec![unit("emberlash_duelist", 1, (3, 4)), unit("ashen_hound", 2, (2, 4))],
        bonuses: vec![],
    };

    // The same seed always gives exactly the same fight.
    let mut combat = content.setup_combat(42, &[home, away]).expect("the teams are valid");

    // `run` fights to the end. `step` advances one tick at a time instead,
    // for programs that want to watch it happen.
    let outcome = combat.run().clone();

    // Everything that happened is in the event log.
    let casts = combat.events().iter().filter(|e| matches!(e.kind, EventKind::CastStarted { .. })).count();
    let deaths = combat.events().iter().filter(|e| matches!(e.kind, EventKind::Died { .. })).count();
    println!("{} events, {casts} abilities cast, {deaths} units fell", combat.events().len());

    match outcome.winner {
        Some(team) => println!("team {} wins after {:.1}s", team.0, outcome.duration_ms as f64 / 1000.0),
        None => println!("a draw after {:.1}s", outcome.duration_ms as f64 / 1000.0),
    }
    for id in &outcome.survivors {
        let unit = combat.unit(*id);
        println!("  {} survives with {} of {} health", unit.name(), unit.health(), unit.stat(Stat::MaxHealth));
    }
}
