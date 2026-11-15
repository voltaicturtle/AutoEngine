//! Computer players.
//!
//! Bots play through the same [`Command`]s as human players, so they double
//! as a test of the command interface and let a full lobby run with no one
//! connected. They come in three [`Difficulty`] levels:
//!
//! * **Easy** buys what it can afford, never rerolls and picks at random.
//! * **Normal** buys units, favours copies of what it already owns, levels
//!   up with spare gold, fields its strongest units with melee in front and
//!   hands out items.
//! * **Hard** shops like normal, but fields the lineup that reaches the
//!   most trait breakpoints, gives items to its strongest units, picks the
//!   option worth most to it and spends what it has saved once it cannot
//!   level any further. It wins about three games in four against normal
//!   bots with the sample content.

use crate::game::{Command, Game, PlayerId, Slot};
use crate::rng::Rng;
use crate::stats::Stat;

/// Gold a bot keeps in reserve once the early rounds are over, so that it
/// earns some interest.
const RESERVE: i64 = 10;

/// How well a bot plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Difficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}

impl Difficulty {
    pub const ALL: [Difficulty; 3] = [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard];

    pub fn name(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Normal => "normal",
            Difficulty::Hard => "hard",
        }
    }
}

impl std::str::FromStr for Difficulty {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Difficulty::ALL
            .into_iter()
            .find(|d| d.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("no bot difficulty called \"{s}\"; use easy, normal or hard"))
    }
}

/// Plays one round for every remaining player and resolves it. `rng` drives
/// the bots' own choices and is separate from the game's random stream.
pub fn play_round(game: &mut Game, rng: &mut Rng) -> Result<(), crate::game::CommandError> {
    play_round_with(game, rng, |_| Difficulty::Normal)
}

/// [`play_round`] with each player's difficulty chosen by `level`.
pub fn play_round_with(
    game: &mut Game,
    rng: &mut Rng,
    level: impl Fn(PlayerId) -> Difficulty,
) -> Result<(), crate::game::CommandError> {
    game.begin_round()?;
    for id in game.alive() {
        play_turn_as(game, id, rng, level(id));
    }
    // Rounds with turn order (drafts) hand the pick from player to player.
    for _ in 0..game.players().len() * 2 {
        let waiting = game.awaiting();
        if waiting.is_empty() {
            break;
        }
        for id in waiting {
            make_pick_as(game, id, rng, level(id));
        }
    }
    game.finish_round()
}

/// Plays one player's turn at normal difficulty.
pub fn play_turn(game: &mut Game, id: PlayerId, rng: &mut Rng) {
    play_turn_as(game, id, rng, Difficulty::Normal);
}

/// Plays one player's turn: pick, shop, level, arrange, equip, ready.
pub fn play_turn_as(game: &mut Game, id: PlayerId, rng: &mut Rng, difficulty: Difficulty) {
    match difficulty {
        Difficulty::Easy => easy_turn(game, id, rng),
        Difficulty::Normal => normal_turn(game, id, rng),
        Difficulty::Hard => hard_turn(game, id, rng),
    }
}

fn easy_turn(game: &mut Game, id: PlayerId, rng: &mut Rng) {
    make_pick(game, id, rng);
    for slot in 0..game.player(id).shop.len() {
        if game.player(id).units().count() < game.board_limit(id) + 1 {
            let _ = game.apply(id, Command::BuyUnit { slot });
        }
    }
    let economy = game.content().rules().economy.clone();
    if game.round_index() % 3 == 2 && game.player(id).gold >= economy.xp_cost + 20 {
        let _ = game.apply(id, Command::BuyXp);
    }
    arrange(game, id);
    equip(game, id);
    let _ = game.apply(id, Command::Ready);
}

fn normal_turn(game: &mut Game, id: PlayerId, rng: &mut Rng) {
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

/// Picks a random open option if the current round is waiting on this player.
pub fn make_pick(game: &mut Game, id: PlayerId, rng: &mut Rng) {
    make_pick_as(game, id, rng, Difficulty::Normal);
}

/// Picks an option if the current round is waiting on this player: the one
/// worth most to it on hard, a random one otherwise.
pub fn make_pick_as(game: &mut Game, id: PlayerId, rng: &mut Rng, difficulty: Difficulty) {
    if !game.awaiting().contains(&id) {
        return;
    }
    if difficulty == Difficulty::Hard {
        let best = game
            .options(id)
            .iter()
            .enumerate()
            .filter_map(|(i, o)| o.as_ref().map(|o| (option_value(game, id, o), std::cmp::Reverse(i))))
            .max();
        if let Some((_, std::cmp::Reverse(option))) = best {
            let _ = game.apply(id, Command::Pick { option });
        }
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
    let limit = game.board_limit(id);
    let content = game.content();
    let cost = |unit: &str| content.unit(unit).map(|u| u.cost).unwrap_or(0);
    let p = game.player(id);
    let mut ranked: Vec<(u32, String, u8)> = p.units().map(|u| (u.uid, u.unit.clone(), u.star)).collect();
    ranked.sort_by_key(|(uid, unit, star)| (std::cmp::Reverse(*star), std::cmp::Reverse(cost(unit)), *uid));
    let chosen: Vec<u32> = ranked.iter().take(limit).map(|(uid, _, _)| *uid).collect();
    field(game, id, &chosen);
}

/// Puts the chosen units on the board, benching any others there: melee
/// toward the front, ranged toward the back.
fn field(game: &mut Game, id: PlayerId, chosen: &[u32]) {
    let rules = game.content().rules();
    let (cols, rows) = (rules.board_cols as i32, rules.board_rows as i32);
    let content = game.content();
    let is_ranged =
        |unit: &str| content.unit(unit).and_then(|u| u.stats.get(&Stat::Range)).is_some_and(|r| *r > 1);
    let p = game.player(id);
    let wanted: Vec<(u32, bool)> =
        chosen.iter().filter_map(|uid| p.unit(*uid).map(|u| (*uid, is_ranged(&u.unit)))).collect();

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

/// Whether a bot should hand this item out: it leaves single-use items
/// that undo things (removers, reforgers) alone.
fn worth_using(game: &Game, id: PlayerId, item: usize) -> bool {
    use crate::content::Consumable;
    let held = &game.player(id).items[item];
    !matches!(
        game.content().item(held).and_then(|i| i.consumable.as_ref()),
        Some(Consumable::RemoveItems | Consumable::Reforge)
    )
}

/// Gives every spare item to a fielded unit that can take it.
fn equip(game: &mut Game, id: PlayerId) {
    let mut item = 0;
    while item < game.player(id).items.len() {
        if !worth_using(game, id, item) {
            item += 1;
            continue;
        }
        let uids: Vec<u32> = game.player(id).board.values().map(|u| u.uid).collect();
        let equipped = uids.into_iter().any(|uid| game.apply(id, Command::EquipItem { item, uid }).is_ok());
        if !equipped {
            item += 1;
        }
    }
}

// ----- hard -----------------------------------------------------------------

/// The traits on a player's board, with how many fielded units have each.
fn board_traits(game: &Game, id: PlayerId) -> std::collections::BTreeMap<String, u32> {
    let mut traits = std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for unit in game.player(id).board.values() {
        if !seen.insert(unit.unit.clone()) {
            continue;
        }
        for t in game.content().unit(&unit.unit).map(|u| u.traits.as_slice()).unwrap_or_default() {
            *traits.entry(t.clone()).or_insert(0) += 1;
        }
    }
    traits
}

/// How much a unit would add to a player: copies toward an upgrade count
/// most, then traits it shares with the board, then its cost.
fn unit_value(game: &Game, id: PlayerId, unit: &str) -> i64 {
    let Some(def) = game.content().unit(unit) else { return 0 };
    let p = game.player(id);
    let copies = p.units().filter(|u| u.unit == unit && u.star == 1).count() as i64;
    let traits = board_traits(game, id);
    let shared: i64 = def.traits.iter().map(|t| traits.get(t).copied().unwrap_or(0) as i64).sum();
    copies * 12 + shared * 4 + def.cost as i64 * 2
}

/// What an option offered by a round is worth: a unit (maybe with an item),
/// an augment or an item.
fn option_value(game: &Game, id: PlayerId, option: &str) -> i64 {
    let content = game.content();
    option
        .split(" + ")
        .map(|part| {
            if content.unit(part).is_some() {
                unit_value(game, id, part)
            } else if let Some(augment) = content.augment(part) {
                // Augments last the whole game, and pay out every round.
                let r = &augment.per_round;
                20 + augment.gold / 2
                    + augment.bonuses.len() as i64 * 6
                    + r.gold * 6
                    + r.xp as i64 * 3
                    + r.free_rerolls as i64 * 3
                    + r.components as i64 * 8
            } else if let Some(item) = content.item(part) {
                if item.recipe.is_some() { 14 } else { 8 }
            } else {
                0
            }
        })
        .sum()
}

fn hard_turn(game: &mut Game, id: PlayerId, rng: &mut Rng) {
    make_pick_as(game, id, rng, Difficulty::Hard);
    shop(game, id);
    let reserve = if game.round_index() < 4 { 0 } else { RESERVE };
    let economy = game.content().rules().economy.clone();
    while game.player(id).gold >= economy.xp_cost + reserve + 4 {
        if game.apply(id, Command::BuyXp).is_err() {
            break;
        }
    }
    // At the top level there is nothing left to save for beyond the most
    // interest a round can pay, so the rest goes on rerolls.
    let at_max = game.player(id).level as usize >= game.content().rules().levels.len();
    let keep = if at_max { economy.interest_per * economy.interest_cap } else { 30 };
    for _ in 0..if at_max { 30 } else { 1 } {
        if game.player(id).gold < keep + economy.reroll_cost || game.apply(id, Command::Reroll).is_err() {
            break;
        }
        shop(game, id);
    }
    hard_arrange(game, id);
    hard_equip(game, id);
    let _ = game.apply(id, Command::Ready);
}

/// Puts items on the fielded units with the most stars and cost first.
fn hard_equip(game: &mut Game, id: PlayerId) {
    let mut item = 0;
    while item < game.player(id).items.len() {
        if !worth_using(game, id, item) {
            item += 1;
            continue;
        }
        let content = game.content();
        let mut uids: Vec<(u8, u32, u32)> = game
            .player(id)
            .board
            .values()
            .map(|u| (u.star, content.unit(&u.unit).map(|d| d.cost).unwrap_or(0), u.uid))
            .collect();
        uids.sort_by_key(|(star, cost, uid)| (std::cmp::Reverse(*star), std::cmp::Reverse(*cost), *uid));
        let equipped = uids.into_iter().any(|(_, _, uid)| game.apply(id, Command::EquipItem { item, uid }).is_ok());
        if !equipped {
            item += 1;
        }
    }
}

/// A unit's rough strength: cost, times the copies it is made of.
fn power(game: &Game, unit: &crate::game::OwnedUnit) -> i64 {
    let cost = game.content().unit(&unit.unit).map(|u| u.cost as i64).unwrap_or(0);
    let copies = (game.content().rules().copies_to_upgrade as i64).pow(unit.star.saturating_sub(1) as u32);
    (cost + 1) * copies * 10 + unit.items.len() as i64 * 8
}

/// How good a lineup is: the units' strength plus the trait breakpoints
/// they reach together.
fn lineup_score(game: &Game, id: PlayerId, uids: &[u32]) -> i64 {
    let p = game.player(id);
    let units: Vec<&crate::game::OwnedUnit> = uids.iter().filter_map(|u| p.unit(*u)).collect();
    let board: Vec<crate::content::BoardUnit> = units
        .iter()
        .enumerate()
        .map(|(i, u)| crate::content::BoardUnit { unit: u.unit.clone(), star: u.star, items: u.items.clone(), pos: (i as i32, 0) })
        .collect();
    let traits: i64 = game
        .content()
        .active_traits(&board)
        .map(|t| t.iter().filter_map(|t| t.tier).map(|tier| (tier as i64 + 1) * 15).sum())
        .unwrap_or(0);
    units.iter().map(|u| power(game, u)).sum::<i64>() + traits
}

/// Fields the strongest lineup it can find: the strongest units first, then
/// swaps with the bench while they raise the score (trait breakpoints).
fn hard_arrange(game: &mut Game, id: PlayerId) {
    let limit = game.board_limit(id);
    let p = game.player(id);
    let mut all: Vec<(i64, u32)> = p.units().map(|u| (power(game, u), u.uid)).collect();
    all.sort_by_key(|(power, uid)| (std::cmp::Reverse(*power), *uid));
    let mut chosen: Vec<u32> = all.iter().take(limit).map(|(_, uid)| *uid).collect();
    let rest: Vec<u32> = all.iter().skip(limit).map(|(_, uid)| *uid).collect();
    let mut score = lineup_score(game, id, &chosen);
    for _ in 0..4 {
        let mut improved = false;
        for &out in &rest {
            if chosen.contains(&out) {
                continue;
            }
            for i in 0..chosen.len() {
                let mut trial = chosen.clone();
                trial[i] = out;
                let s = lineup_score(game, id, &trial);
                if s > score {
                    (chosen, score, improved) = (trial, s, true);
                    break;
                }
            }
        }
        if !improved {
            break;
        }
    }
    field(game, id, &chosen);
}
