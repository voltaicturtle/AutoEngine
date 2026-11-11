//! Simple computer players.
//!
//! Bots play through the same [`Command`]s as human players, so they double
//! as a test of the command interface and let a full lobby run with no one
//! connected. The strategy is deliberately plain: buy units, favour copies
//! of what is already owned, level up with spare gold, field the strongest
//! units with melee in front, and hand out items.

use crate::game::{Command, Game, PlayerId, Slot};
use crate::rng::Rng;
use crate::stats::Stat;

/// Gold a bot keeps in reserve once the early rounds are over, so that it
/// earns some interest.
const RESERVE: i64 = 10;

/// Plays one round for every remaining player and resolves it. `rng` drives
/// the bots' own choices and is separate from the game's random stream.
pub fn play_round(game: &mut Game, rng: &mut Rng) -> Result<(), crate::game::CommandError> {
    game.begin_round()?;
    for id in game.alive() {
        play_turn(game, id, rng);
    }
    // Rounds with turn order (drafts) hand the pick from player to player.
    for _ in 0..game.players().len() * 2 {
        let waiting = game.awaiting();
        if waiting.is_empty() {
            break;
        }
        for id in waiting {
            make_pick(game, id, rng);
        }
    }
    game.finish_round()
}

/// Plays one player's turn: pick, shop, level, arrange, equip, ready.
pub fn play_turn(game: &mut Game, id: PlayerId, rng: &mut Rng) {
    make_pick(game, id, rng);
    shop(game, id);
    let reserve = if game.round_index() < 4 { 0 } else { RESERVE };
    let economy = game.content().rules().economy.clone();
    while game.player(id).gold >= economy.xp_cost + reserve + 4 {
        if game.apply(id, Command::BuyXp).is_err() {
            break;
        }
    }
    if game.player(id).gold >= 30 + economy.reroll_cost && game.apply(id, Command::Reroll).is_ok() {
        shop(game, id);
    }
    arrange(game, id);
    equip(game, id);
    let _ = game.apply(id, Command::Ready);
}

fn make_pick(game: &mut Game, id: PlayerId, rng: &mut Rng) {
    if !game.awaiting().contains(&id) {
        return;
    }
    let open: Vec<usize> =
        game.options(id).iter().enumerate().filter(|(_, o)| o.is_some()).map(|(i, _)| i).collect();
    if !open.is_empty() {
        let option = open[rng.below(open.len() as u64) as usize];
        let _ = game.apply(id, Command::Pick { option });
    }
}

fn shop(game: &mut Game, id: PlayerId) {
    for slot in 0..game.player(id).shop.len() {
        let p = game.player(id);
        let Some(offer) = p.shop[slot].clone() else { continue };
        let owned = p.units().count();
        let has_copy = p.units().any(|u| u.unit == offer);
        if has_copy || owned < game.board_limit(id) + 2 {
            let _ = game.apply(id, Command::BuyUnit { slot });
        }
    }
}

/// Puts the strongest units on the board: melee toward the front, ranged
/// toward the back.
fn arrange(game: &mut Game, id: PlayerId) {
    let rules = game.content().rules();
    let (cols, rows) = (rules.board_cols as i32, rules.board_rows as i32);
    let limit = game.board_limit(id);
    let content = game.content();
    let cost = |unit: &str| content.unit(unit).map(|u| u.cost).unwrap_or(0);
    let is_ranged =
        |unit: &str| content.unit(unit).and_then(|u| u.stats.get(&Stat::Range)).is_some_and(|r| *r > 1);

    let p = game.player(id);
    let mut ranked: Vec<(u32, String, u8)> = p.units().map(|u| (u.uid, u.unit.clone(), u.star)).collect();
    ranked.sort_by_key(|(uid, unit, star)| (std::cmp::Reverse(*star), std::cmp::Reverse(cost(unit)), *uid));
    let wanted: Vec<(u32, bool)> = ranked.iter().take(limit).map(|(uid, unit, _)| (*uid, is_ranged(unit))).collect();

    let mut columns: Vec<i32> = (0..cols).collect();
    columns.sort_by_key(|c| ((c - cols / 2).abs(), *c));
    let front_rows: Vec<i32> = (rows / 2..rows).collect();
    let back_rows: Vec<i32> = (rows / 2..rows).rev().collect();

    for (uid, ranged) in &wanted {
        let p = game.player(id);
        if matches!(p.locate(*uid), Some(Slot::Board(..))) {
            continue;
        }
        // Take the place of a unit that should not be fielded, if any.
        let benchable = p.board.iter().find(|(_, u)| !wanted.iter().any(|(w, _)| *w == u.uid)).map(|(pos, _)| *pos);
        let target = benchable.or_else(|| {
            let row_order = if *ranged { &back_rows } else { &front_rows };
            row_order
                .iter()
                .flat_map(|r| columns.iter().map(move |c| (*c, *r)))
                .find(|pos| !p.board.contains_key(pos))
        });
        if let Some((c, r)) = target {
            let _ = game.apply(id, Command::MoveUnit { uid: *uid, to: Slot::Board(c, r) });
        }
    }
}

/// Gives every spare item to a fielded unit that can take it.
fn equip(game: &mut Game, id: PlayerId) {
    let mut item = 0;
    while item < game.player(id).items.len() {
        let uids: Vec<u32> = game.player(id).board.values().map(|u| u.uid).collect();
        let equipped = uids.into_iter().any(|uid| game.apply(id, Command::EquipItem { item, uid }).is_ok());
        if !equipped {
            item += 1;
        }
    }
}
