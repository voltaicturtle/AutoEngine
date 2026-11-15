use std::sync::Arc;

use autoengine::bot::{Difficulty, play_round_with};
use autoengine::prelude::*;

/// Average placement per difficulty over some games where they meet.
fn placements(a: Difficulty, b: Difficulty, games: u64) -> (f64, f64) {
    let content = Arc::new(autoengine::sample::content());
    let mut total = [0u32; 2];
    for seed in 0..games {
        let mut game = Game::new(content.clone(), seed, &["a", "b", "c", "d"]);
        let mut rng = Rng::new(seed ^ 77);
        // Seats alternate, and swap every other game.
        let side = |id: PlayerId| (id.0 as u64 + seed) % 2;
        let level = |id: PlayerId| if side(id) == 0 { a } else { b };
        while !game.is_over() && game.round_index() < 200 {
            play_round_with(&mut game, &mut rng, level).unwrap();
            game.drain_events();
        }
        for p in game.players() {
            total[side(p.id) as usize] += p.placement.unwrap();
        }
    }
    let n = games as f64 * 2.0;
    (total[0] as f64 / n, total[1] as f64 / n)
}

#[test]
fn harder_bots_place_better() {
    let (easy, normal) = placements(Difficulty::Easy, Difficulty::Normal, 16);
    assert!(normal + 0.5 < easy, "normal {normal} vs easy {easy}");
    let (normal, hard) = placements(Difficulty::Normal, Difficulty::Hard, 40);
    assert!(hard < normal, "hard {hard} vs normal {normal}");
}

#[test]
fn difficulties_have_names() {
    for level in Difficulty::ALL {
        assert_eq!(level.name().parse::<Difficulty>(), Ok(level));
    }
    assert_eq!("HARD".parse::<Difficulty>(), Ok(Difficulty::Hard));
    assert!("brutal".parse::<Difficulty>().unwrap_err().contains("easy, normal or hard"));
    assert_eq!(Difficulty::default(), Difficulty::Normal);
}
