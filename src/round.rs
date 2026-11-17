//! Round types.
//!
//! A round is anything that happens between two shop phases. The game does
//! not hardcode what that is: it talks to a [`RoundType`] through one small
//! interface and the schedule in the content files names which type each
//! round uses. The built-in types cover fights between players, fights
//! against neutral boards, a shared draft and private choices; a game can
//! add its own with [`Game::register_round_type`].

use std::collections::BTreeMap;

use crate::content::{BoardUnit, ChoiceOffer, RoundKindDef};
use crate::game::{CommandError, Game, GameEvent, Opponent, PlayerId};

/// Builds a custom round from the `params` written in the schedule.
pub type RoundFactory = Box<dyn Fn(&BTreeMap<String, i64>) -> Box<dyn RoundType> + Send>;

/// The behaviour of one kind of round.
///
/// The usual shop and board commands stay available during every round;
/// a round type only describes what is special about it.
pub trait RoundType: Send {
    /// A short name for the kind, reported to clients.
    fn kind(&self) -> &str;

    /// Called once when the round starts, after income and shop refresh.
    fn begin(&mut self, _game: &mut Game) {}

    /// Players who currently need to make a pick. The round is not
    /// considered ready while this is non-empty.
    fn awaiting(&self, _game: &Game) -> Vec<PlayerId> {
        Vec::new()
    }

    /// The options on offer to a player; `None` marks one already taken.
    fn options(&self, _game: &Game, _player: PlayerId) -> Vec<Option<String>> {
        Vec::new()
    }

    /// Handles a player's [`Command::Pick`](crate::game::Command::Pick).
    fn pick(&mut self, _game: &mut Game, _player: PlayerId, _option: usize) -> Result<(), CommandError> {
        Err(CommandError::NothingToPick)
    }

    /// Handles a player's [`Command::RerollPick`](crate::game::Command::RerollPick):
    /// fresh options in place of the ones on offer.
    fn reroll(&mut self, _game: &mut Game, _player: PlayerId) -> Result<(), CommandError> {
        Err(CommandError::NothingToPick)
    }

    /// How many more times a player may reroll their options this round.
    fn rerolls_left(&self, _game: &Game, _player: PlayerId) -> u32 {
        0
    }

    /// Called once when the round ends. Must settle everything, including
    /// picking for players who did not.
    fn finish(&mut self, game: &mut Game);
}

/// Creates the round type for a built-in kind.
pub(crate) fn builtin(def: &RoundKindDef) -> Box<dyn RoundType> {
    match def {
        RoundKindDef::Pvp => Box::new(PvpRound),
        RoundKindDef::Pve { enemies, gold, items, rewards } => {
            Box::new(PveRound { enemies: enemies.clone(), gold: *gold, items: *items, rewards: rewards.clone() })
        }
        RoundKindDef::Draft { extra, max_cost } => Box::new(DraftRound {
            extra: *extra,
            max_cost: *max_cost,
            offers: Vec::new(),
            order: Vec::new(),
            turn: 0,
        }),
        RoundKindDef::Choice { options, offer } => {
            Box::new(ChoiceRound { count: *options, offer: offer.clone(), offers: BTreeMap::new(), rerolled: BTreeMap::new() })
        }
        RoundKindDef::Custom { .. } => unreachable!("custom rounds are built by their factory"),
    }
}

/// Pairs players for a round of fights: of all the ways to pair them, the
/// one that repeats recent fights least (see [`Game::rematch_cost`]), the
/// first such in the given (shuffled) order. With an odd number, one is
/// left over. Lobbies of more than twelve are paired greedily instead.
pub fn pair_up(game: &Game, players: &[PlayerId]) -> (Vec<(PlayerId, PlayerId)>, Option<PlayerId>) {
    type Pairs = Vec<(PlayerId, PlayerId)>;
    // Teammates fight each other only when there is nobody else.
    let cost = |a: PlayerId, b: PlayerId| game.rematch_cost(a, b) + game.rematch_cost(b, a) + if game.teammates(a, b) { 1_000_000 } else { 0 };

    /// Every pairing of `left` (one may sit out if the count is odd).
    fn search(
        cost: &dyn Fn(PlayerId, PlayerId) -> u32,
        left: &[PlayerId],
        spent: u32,
        chosen: &mut Pairs,
        sitting: Option<PlayerId>,
        best: &mut Option<(u32, Pairs, Option<PlayerId>)>,
    ) {
        if best.as_ref().is_some_and(|(b, _, _)| spent >= *b) {
            return;
        }
        let Some((&a, rest)) = left.split_first() else {
            *best = Some((spent, chosen.clone(), sitting));
            return;
        };
        for i in 0..rest.len() {
            let b = rest[i];
            let others: Vec<PlayerId> = rest.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, p)| *p).collect();
            chosen.push((a, b));
            search(cost, &others, spent + cost(a, b), chosen, sitting, best);
            chosen.pop();
        }
        if left.len() % 2 == 1 && sitting.is_none() {
            search(cost, rest, spent, chosen, Some(a), best);
        }
    }

    if players.len() <= 12 {
        let mut best = None;
        search(&cost, players, 0, &mut Vec::new(), None, &mut best);
        let (_, pairs, odd) = best.expect("there is always a pairing");
        return (pairs, odd);
    }
    let mut left = players.to_vec();
    let mut pairs = Vec::new();
    while left.len() >= 2 {
        let a = left.remove(0);
        let i = (0..left.len()).min_by_key(|i| (cost(a, left[*i]), *i)).unwrap();
        pairs.push((a, left.remove(i)));
    }
    (pairs, left.pop())
}

/// Players are paired (avoiding recent rematches) and fight. The loser
/// takes damage.
pub struct PvpRound;

impl RoundType for PvpRound {
    fn kind(&self) -> &str {
        "pvp"
    }

    fn finish(&mut self, game: &mut Game) {
        let mut players = game.alive();
        game.rng().shuffle(&mut players);
        let (pairs, odd) = pair_up(game, &players);
        for (a, b) in pairs {
            let result = game.fight(a, Opponent::Player(b), &[]);
            let damage = game.loss_damage(result.survivors);
            match result.home_won {
                Some(a_won) => {
                    let (winner, loser) = if a_won { (a, b) } else { (b, a) };
                    game.record_result(winner, true);
                    game.record_result(loser, false);
                    game.damage_player(loser, damage);
                }
                None => {
                    for p in [a, b] {
                        game.record_result(p, false);
                        game.damage_player(p, damage);
                    }
                }
            }
        }
        // With an odd number of players, the one left over fights a copy of
        // someone else's board.
        if let Some(odd) = &odd {
            let others: Vec<PlayerId> = players.iter().copied().filter(|p| p != odd && !game.teammates(*p, *odd)).collect();
            if others.is_empty() {
                return;
            }
            let ghost = others[game.rng().below(others.len() as u64) as usize];
            let result = game.fight(*odd, Opponent::Ghost(ghost), &[]);
            let won = result.home_won == Some(true);
            game.record_result(*odd, won);
            if !won {
                let damage = game.loss_damage(result.survivors);
                game.damage_player(*odd, damage);
            }
        }
    }
}

/// Every player fights the same neutral board for rewards.
pub struct PveRound {
    enemies: Vec<BoardUnit>,
    gold: i64,
    items: u32,
    rewards: Vec<crate::game::Grant>,
}

impl RoundType for PveRound {
    fn kind(&self) -> &str {
        "pve"
    }

    fn finish(&mut self, game: &mut Game) {
        for player in game.alive() {
            let result = game.fight(player, Opponent::Neutral, &self.enemies);
            if result.home_won == Some(true) {
                game.add_gold(player, self.gold);
                for _ in 0..self.items {
                    game.give_random_component(player);
                }
                for reward in &self.rewards {
                    // Checked when the content was built, so this cannot fail.
                    let _ = game.give(player, reward);
                }
            } else {
                let damage = game.loss_damage(result.survivors);
                game.damage_player(player, damage);
            }
        }
    }
}

/// A shared draft. One set of units, each carrying an item, is offered to
/// everyone; players pick one each in turn, lowest health first.
pub struct DraftRound {
    extra: u32,
    max_cost: Option<u32>,
    /// `(unit, item)`; `None` once taken.
    offers: Vec<Option<(String, Option<String>)>>,
    order: Vec<PlayerId>,
    turn: usize,
}

impl DraftRound {
    fn describe(offer: &(String, Option<String>)) -> String {
        match &offer.1 {
            Some(item) => format!("{} + {}", offer.0, item),
            None => offer.0.clone(),
        }
    }

    fn take(&mut self, game: &mut Game, player: PlayerId, option: usize) {
        let (unit, item) = self.offers[option].take().expect("offer is available");
        game.emit(GameEvent::ChoiceMade {
            player,
            option,
            choice: Self::describe(&(unit.clone(), item.clone())),
        });
        game.add_unit(player, &unit, item.into_iter().collect());
        self.turn += 1;
    }
}

impl RoundType for DraftRound {
    fn kind(&self) -> &str {
        "draft"
    }

    fn begin(&mut self, game: &mut Game) {
        let mut order = game.alive();
        order.sort_by_key(|p| (game.player(*p).health, *p));
        let components = game.component_items();
        for _ in 0..order.len() as u32 + self.extra {
            let available = game.available_units(self.max_cost);
            let weights: Vec<u64> = available.iter().map(|(_, n)| *n as u64).collect();
            let Some(i) = game.rng().weighted_index(&weights) else { break };
            let unit = available[i].0.clone();
            game.take_from_pool(&unit);
            let item = if components.is_empty() {
                None
            } else {
                Some(components[game.rng().below(components.len() as u64) as usize].clone())
            };
            self.offers.push(Some((unit, item)));
        }
        self.order = order;
        let options = self.offers.iter().flatten().map(Self::describe).collect();
        game.emit(GameEvent::ChoiceOffered { player: None, options });
    }

    fn awaiting(&self, _game: &Game) -> Vec<PlayerId> {
        let anything_left = self.offers.iter().any(Option::is_some);
        self.order.get(self.turn).filter(|_| anything_left).copied().into_iter().collect()
    }

    fn options(&self, _game: &Game, _player: PlayerId) -> Vec<Option<String>> {
        self.offers.iter().map(|o| o.as_ref().map(Self::describe)).collect()
    }

    fn pick(&mut self, game: &mut Game, player: PlayerId, option: usize) -> Result<(), CommandError> {
        if self.order.get(self.turn) != Some(&player) {
            return Err(CommandError::NotYourTurn);
        }
        if !matches!(self.offers.get(option), Some(Some(_))) {
            return Err(CommandError::InvalidOption);
        }
        self.take(game, player, option);
        Ok(())
    }

    fn finish(&mut self, game: &mut Game) {
        // Anyone who has not picked gets a random remaining offer.
        while self.turn < self.order.len() {
            let left: Vec<usize> = (0..self.offers.len()).filter(|i| self.offers[*i].is_some()).collect();
            if left.is_empty() {
                break;
            }
            let option = left[game.rng().below(left.len() as u64) as usize];
            let player = self.order[self.turn];
            self.take(game, player, option);
        }
        for (unit, _) in self.offers.drain(..).flatten() {
            game.put_in_pool(&unit);
        }
    }
}

/// Each player privately picks one of several augments or items.
pub struct ChoiceRound {
    count: u32,
    offer: ChoiceOffer,
    /// Options still waiting on each player.
    offers: BTreeMap<PlayerId, Vec<String>>,
    /// Rerolls each player has used this round.
    rerolled: BTreeMap<PlayerId, u32>,
}

impl ChoiceRound {
    /// Everything that could be offered to a player, before shuffling.
    fn candidates(&self, game: &Game, player: PlayerId) -> Vec<String> {
        game.choice_candidates(player, &self.offer)
    }

    fn grant(&mut self, game: &mut Game, player: PlayerId, option: usize) {
        let options = self.offers.remove(&player).expect("player has an offer");
        let choice = options[option].clone();
        game.emit(GameEvent::ChoiceMade { player, option, choice: choice.clone() });
        game.give_choice(player, &self.offer, &choice);
    }
}

impl RoundType for ChoiceRound {
    fn kind(&self) -> &str {
        "choice"
    }

    fn begin(&mut self, game: &mut Game) {
        // One tier for everyone, where the round rolls for it.
        self.offer = game.settle_offer(&self.offer);
        for player in game.alive() {
            let mut candidates = self.candidates(game, player);
            game.rng().shuffle(&mut candidates);
            candidates.truncate(self.count as usize);
            if candidates.is_empty() {
                continue;
            }
            game.emit(GameEvent::ChoiceOffered { player: Some(player), options: candidates.clone() });
            self.offers.insert(player, candidates);
        }
    }

    fn awaiting(&self, _game: &Game) -> Vec<PlayerId> {
        self.offers.keys().copied().collect()
    }

    fn options(&self, _game: &Game, player: PlayerId) -> Vec<Option<String>> {
        self.offers.get(&player).map(|o| o.iter().cloned().map(Some).collect()).unwrap_or_default()
    }

    fn pick(&mut self, game: &mut Game, player: PlayerId, option: usize) -> Result<(), CommandError> {
        let options = self.offers.get(&player).ok_or(CommandError::NothingToPick)?;
        if option >= options.len() {
            return Err(CommandError::InvalidOption);
        }
        self.grant(game, player, option);
        Ok(())
    }

    fn reroll(&mut self, game: &mut Game, player: PlayerId) -> Result<(), CommandError> {
        let current = self.offers.get(&player).ok_or(CommandError::NothingToPick)?.clone();
        if self.rerolls_left(game, player) == 0 {
            return Err(CommandError::NoRerollsLeft);
        }
        // New options where there are enough; the old ones fill any gap.
        let mut fresh: Vec<String> = self.candidates(game, player).into_iter().filter(|c| !current.contains(c)).collect();
        if fresh.is_empty() {
            return Err(CommandError::NoRerollsLeft);
        }
        game.rng().shuffle(&mut fresh);
        fresh.truncate(self.count as usize);
        *self.rerolled.entry(player).or_insert(0) += 1;
        game.emit(GameEvent::ChoiceOffered { player: Some(player), options: fresh.clone() });
        self.offers.insert(player, fresh);
        Ok(())
    }

    fn rerolls_left(&self, game: &Game, player: PlayerId) -> u32 {
        if !self.offers.contains_key(&player) {
            return 0;
        }
        let used = self.rerolled.get(&player).copied().unwrap_or(0);
        game.content().rules().choice_rerolls.saturating_sub(used)
    }

    fn finish(&mut self, game: &mut Game) {
        let undecided: Vec<(PlayerId, usize)> = self.offers.iter().map(|(p, o)| (*p, o.len())).collect();
        for (player, count) in undecided {
            let option = game.rng().below(count as u64) as usize;
            self.grant(game, player, option);
        }
    }
}
