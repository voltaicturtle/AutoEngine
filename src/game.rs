//! The game around the fights: players, the shop, gold and experience, the
//! bench and board, items, star upgrades, and the round loop.
//!
//! A [`Game`] is the authoritative state of one lobby. It is driven in a
//! simple cycle:
//!
//! 1. [`Game::begin_round`] pays income, refreshes shops and starts the next
//!    round from the schedule.
//! 2. Players send [`Command`]s through [`Game::apply`]. Invalid commands are
//!    rejected with a [`CommandError`] and change nothing.
//! 3. [`Game::finish_round`] lets the round resolve (run its fights, hand out
//!    rewards, auto-pick for anyone who did not choose) and eliminates
//!    players who ran out of health.
//!
//! Everything that happens is reported as a [`GameEvent`], and each fight is
//! kept as a [`CombatRecord`] with its full event log, so clients can show
//! exactly what the server computed. Like combat, a game is deterministic:
//! the same seed and the same commands give the same game.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::combat::{Event, Team};
use crate::content::{BoardUnit, ContentSet, RoundKindDef, TeamSetup};
use crate::rng::Rng;
use crate::round::{self, RoundFactory, RoundType};

/// Identifies a player in a game, by seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlayerId(pub u8);

/// A unit a player owns, on the bench or the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedUnit {
    /// Unique within the game; commands refer to units by this.
    pub uid: u32,
    /// Id of the unit definition.
    pub unit: String,
    pub star: u8,
    pub items: Vec<String>,
}

/// Where a unit can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Slot {
    Bench(usize),
    /// `(column, row)` from the owner's point of view; the owner's half is
    /// the higher-numbered rows.
    Board(i32, i32),
}

/// Something a player asks the game to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Buy the unit in a shop slot.
    BuyUnit { slot: usize },
    SellUnit { uid: u32 },
    /// Pay to replace the shop's offers.
    Reroll,
    BuyXp,
    /// Keep the current shop offers for the next round.
    SetShopLock { locked: bool },
    /// Move a unit; if the destination is taken the two units swap.
    MoveUnit { uid: u32, to: Slot },
    /// Give an item from the player's inventory to a unit.
    EquipItem { item: usize, uid: u32 },
    /// Choose an option the current round is offering.
    Pick { option: usize },
    /// Signal that the player is done for this round.
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandError {
    /// Commands are only accepted between `begin_round` and `finish_round`.
    NotPlanning,
    UnknownPlayer,
    Eliminated,
    NotEnoughGold { need: i64, have: i64 },
    EmptyShopSlot,
    SoldOut,
    BenchFull,
    BoardFull,
    UnknownUnit,
    InvalidSlot,
    UnknownItem,
    ItemSlotsFull,
    MaxLevel,
    NothingToPick,
    NotYourTurn,
    InvalidOption,
    UnknownRoundKind(String),
    GameOver,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::NotPlanning => write!(f, "no round is in progress"),
            CommandError::UnknownPlayer => write!(f, "no such player"),
            CommandError::Eliminated => write!(f, "the player has been eliminated"),
            CommandError::NotEnoughGold { need, have } => write!(f, "needs {need} gold, has {have}"),
            CommandError::EmptyShopSlot => write!(f, "that shop slot is empty"),
            CommandError::SoldOut => write!(f, "no copies of that unit are left"),
            CommandError::BenchFull => write!(f, "the bench is full"),
            CommandError::BoardFull => write!(f, "the board is full at this level"),
            CommandError::UnknownUnit => write!(f, "the player owns no such unit"),
            CommandError::InvalidSlot => write!(f, "that is not a valid place for a unit"),
            CommandError::UnknownItem => write!(f, "no such item in the inventory"),
            CommandError::ItemSlotsFull => write!(f, "the unit cannot hold more items"),
            CommandError::MaxLevel => write!(f, "already at the highest level"),
            CommandError::NothingToPick => write!(f, "this round has nothing to pick"),
            CommandError::NotYourTurn => write!(f, "it is not this player's turn to pick"),
            CommandError::InvalidOption => write!(f, "that option is not available"),
            CommandError::UnknownRoundKind(kind) => write!(f, "no round type named `{kind}` is registered"),
            CommandError::GameOver => write!(f, "the game is over"),
        }
    }
}

impl std::error::Error for CommandError {}

/// One player's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub health: i64,
    pub gold: i64,
    pub level: u32,
    pub xp: u32,
    pub bench: Vec<Option<OwnedUnit>>,
    /// Fielded units by `(column, row)`.
    #[serde(with = "board_serde")]
    pub board: BTreeMap<(i32, i32), OwnedUnit>,
    /// Items not yet given to a unit.
    pub items: Vec<String>,
    pub shop: Vec<Option<String>>,
    pub shop_locked: bool,
    pub augments: Vec<String>,
    /// Positive for consecutive wins, negative for consecutive losses.
    pub streak: i32,
    pub alive: bool,
    /// Final place, once eliminated or victorious (1 is the winner).
    pub placement: Option<u32>,
    pub ready: bool,
}

/// Serializes the board as a list of entries, since formats such as JSON
/// cannot use a pair as a map key.
mod board_serde {
    use super::OwnedUnit;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    type Board = BTreeMap<(i32, i32), OwnedUnit>;

    pub fn serialize<S: Serializer>(board: &Board, s: S) -> Result<S::Ok, S::Error> {
        board.iter().collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Board, D::Error> {
        Ok(Vec::<((i32, i32), OwnedUnit)>::deserialize(d)?.into_iter().collect())
    }
}

impl Player {
    /// Every unit the player owns, board first.
    pub fn units(&self) -> impl Iterator<Item = &OwnedUnit> {
        self.board.values().chain(self.bench.iter().flatten())
    }

    /// Where a unit is.
    pub fn locate(&self, uid: u32) -> Option<Slot> {
        if let Some((pos, _)) = self.board.iter().find(|(_, u)| u.uid == uid) {
            return Some(Slot::Board(pos.0, pos.1));
        }
        self.bench.iter().position(|s| s.as_ref().is_some_and(|u| u.uid == uid)).map(Slot::Bench)
    }

    pub fn unit(&self, uid: u32) -> Option<&OwnedUnit> {
        self.units().find(|u| u.uid == uid)
    }

    fn unit_mut(&mut self, uid: u32) -> Option<&mut OwnedUnit> {
        self.board.values_mut().chain(self.bench.iter_mut().flatten()).find(|u| u.uid == uid)
    }

    fn take(&mut self, slot: Slot) -> Option<OwnedUnit> {
        match slot {
            Slot::Bench(i) => self.bench.get_mut(i).and_then(Option::take),
            Slot::Board(c, r) => self.board.remove(&(c, r)),
        }
    }

    fn put(&mut self, slot: Slot, unit: OwnedUnit) {
        match slot {
            Slot::Bench(i) => self.bench[i] = Some(unit),
            Slot::Board(c, r) => {
                self.board.insert((c, r), unit);
            }
        }
    }

    /// The player's board as input for combat.
    pub fn board_units(&self) -> Vec<BoardUnit> {
        self.board
            .iter()
            .map(|(pos, u)| BoardUnit { unit: u.unit.clone(), star: u.star, items: u.items.clone(), pos: *pos })
            .collect()
    }
}

/// Who a player fights.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Opponent {
    Player(PlayerId),
    /// A copy of another player's board, used when the lobby is uneven. The
    /// copied player is not affected by the result.
    Ghost(PlayerId),
    /// A neutral board from the round definition.
    Neutral,
}

/// A finished fight, with everything needed to replay it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatRecord {
    pub home: PlayerId,
    pub away: Opponent,
    pub seed: u64,
    /// Length of one simulation tick; event ticks are multiples of this.
    pub tick_ms: u32,
    /// `Team(0)` is the home player.
    pub winner: Option<Team>,
    pub duration_ms: u32,
    /// Units of the winning side still standing.
    pub survivors: u32,
    /// Both sides as they entered the fight, home first. With the seed
    /// this is enough to fight it again (see `replay`).
    #[serde(default)]
    pub teams: Vec<TeamSetup>,
    pub events: Vec<Event>,
}

/// Something that happened in the game outside of a fight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameEvent {
    RoundStarted { index: usize, name: String, kind: String, stage: u32 },
    RoundFinished { index: usize },
    ShopRolled { player: PlayerId, offers: Vec<Option<String>> },
    GoldChanged { player: PlayerId, gold: i64 },
    XpChanged { player: PlayerId, level: u32, xp: u32 },
    UnitGained { player: PlayerId, uid: u32, unit: String },
    UnitSold { player: PlayerId, uid: u32, unit: String },
    UnitUpgraded { player: PlayerId, uid: u32, unit: String, star: u8 },
    UnitMoved { player: PlayerId, uid: u32, to: Slot },
    ItemGained { player: PlayerId, item: String },
    ItemEquipped { player: PlayerId, uid: u32, item: String },
    ItemCombined { player: PlayerId, uid: u32, item: String },
    /// Options offered to one player, or to everyone when `player` is `None`.
    ChoiceOffered { player: Option<PlayerId>, options: Vec<String> },
    ChoiceMade { player: PlayerId, option: usize, choice: String },
    AugmentGained { player: PlayerId, augment: String },
    PlayerReady { player: PlayerId },
    /// Index into [`Game::combats`] for the full record.
    CombatFinished { combat: usize, home: PlayerId, away: Opponent, winner: Option<Team> },
    PlayerDamaged { player: PlayerId, amount: i64, health: i64 },
    PlayerEliminated { player: PlayerId, placement: u32 },
    GameOver { winner: PlayerId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    BetweenRounds,
    Planning,
    Over,
}

/// The result of [`Game::fight`], from the home player's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FightResult {
    /// `Some(true)` if the home player won, `None` for a draw.
    pub home_won: Option<bool>,
    /// Units of the winning side still standing.
    pub survivors: u32,
}

/// The authoritative state of one lobby.
pub struct Game {
    content: Arc<ContentSet>,
    rng: Rng,
    players: Vec<Player>,
    pool: BTreeMap<String, u32>,
    round_index: usize,
    round_name: String,
    round_kind: String,
    stage: u32,
    phase: Phase,
    current: Option<Box<dyn RoundType>>,
    factories: BTreeMap<String, RoundFactory>,
    events: Vec<GameEvent>,
    combats: Vec<CombatRecord>,
    next_uid: u32,
}

impl Game {
    /// Creates a game with one player per name.
    pub fn new(content: impl Into<Arc<ContentSet>>, seed: u64, names: &[&str]) -> Self {
        assert!(!names.is_empty() && names.len() <= 250, "a game needs 1 to 250 players");
        let content = content.into();
        let rules = content.rules();
        let pool = content
            .units()
            .filter(|u| u.in_shop)
            .map(|u| {
                let size = rules.pool_sizes.get(&u.cost).copied().unwrap_or(rules.default_pool_size);
                (u.id.clone(), size)
            })
            .collect();
        let players = names
            .iter()
            .enumerate()
            .map(|(i, name)| Player {
                id: PlayerId(i as u8),
                name: name.to_string(),
                health: rules.starting_health,
                gold: rules.economy.starting_gold,
                level: 1,
                xp: 0,
                bench: vec![None; rules.bench_size],
                board: BTreeMap::new(),
                items: Vec::new(),
                shop: vec![None; rules.shop_size],
                shop_locked: false,
                augments: Vec::new(),
                streak: 0,
                alive: true,
                placement: None,
                ready: false,
            })
            .collect();
        Game {
            content,
            rng: Rng::new(seed),
            players,
            pool,
            round_index: 0,
            round_name: String::new(),
            round_kind: String::new(),
            stage: 1,
            phase: Phase::BetweenRounds,
            current: None,
            factories: BTreeMap::new(),
            events: Vec::new(),
            combats: Vec::new(),
            next_uid: 1,
        }
    }

    /// Registers a custom round type under the name used by
    /// `Custom(kind: "...")` entries in the schedule.
    pub fn register_round_type(&mut self, kind: impl Into<String>, factory: RoundFactory) {
        self.factories.insert(kind.into(), factory);
    }

    // ----- inspection -----------------------------------------------------

    pub fn content(&self) -> &ContentSet {
        &self.content
    }
    pub fn players(&self) -> &[Player] {
        &self.players
    }
    pub fn player(&self, id: PlayerId) -> &Player {
        &self.players[id.0 as usize]
    }
    /// Players still in the game, by seat.
    pub fn alive(&self) -> Vec<PlayerId> {
        self.players.iter().filter(|p| p.alive).map(|p| p.id).collect()
    }
    /// Zero-based index of the current (or next) round.
    pub fn round_index(&self) -> usize {
        self.round_index
    }
    /// Name of the current round from the schedule.
    pub fn round_name(&self) -> &str {
        &self.round_name
    }
    /// Kind of the current round, as reported by its round type.
    pub fn round_kind(&self) -> &str {
        &self.round_kind
    }
    /// Stage of the current round, which scales damage.
    pub fn stage(&self) -> u32 {
        self.stage
    }
    pub fn in_round(&self) -> bool {
        self.phase == Phase::Planning
    }
    pub fn is_over(&self) -> bool {
        self.phase == Phase::Over
    }
    pub fn winner(&self) -> Option<PlayerId> {
        self.players.iter().find(|p| p.placement == Some(1)).map(|p| p.id)
    }
    /// Copies of a unit left in the shared pool.
    pub fn pool_count(&self, unit: &str) -> u32 {
        self.pool.get(unit).copied().unwrap_or(0)
    }
    /// How many units a player may field at their level.
    pub fn board_limit(&self, id: PlayerId) -> usize {
        let levels = &self.content.rules().levels;
        levels[(self.player(id).level as usize - 1).min(levels.len() - 1)].board_size
    }
    /// Players the current round is waiting on for a pick.
    pub fn awaiting(&self) -> Vec<PlayerId> {
        self.current.as_ref().map(|r| r.awaiting(self)).unwrap_or_default()
    }
    /// What the current round offers a player; `None` entries are taken.
    pub fn options(&self, id: PlayerId) -> Vec<Option<String>> {
        self.current.as_ref().map(|r| r.options(self, id)).unwrap_or_default()
    }
    /// True when every remaining player is ready and no picks are pending.
    pub fn all_ready(&self) -> bool {
        self.players.iter().filter(|p| p.alive).all(|p| p.ready) && self.awaiting().is_empty()
    }
    /// Every event so far.
    pub fn events(&self) -> &[GameEvent] {
        &self.events
    }
    /// Removes and returns the events recorded since the last call.
    pub fn drain_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.events)
    }
    /// Fights of the most recent round.
    pub fn combats(&self) -> &[CombatRecord] {
        &self.combats
    }

    // ----- the round loop -------------------------------------------------

    /// Starts the next round: pays income, refreshes shops and hands control
    /// to the round type named by the schedule.
    pub fn begin_round(&mut self) -> Result<(), CommandError> {
        match self.phase {
            Phase::Over => return Err(CommandError::GameOver),
            Phase::Planning => return Ok(()),
            Phase::BetweenRounds => {}
        }
        let content = self.content.clone();
        let (def, stage) = content.schedule().round(self.round_index);
        let mut round: Box<dyn RoundType> = match &def.kind {
            RoundKindDef::Custom { kind, params } => match self.factories.get(kind) {
                Some(factory) => factory(params),
                None => return Err(CommandError::UnknownRoundKind(kind.clone())),
            },
            builtin => round::builtin(builtin),
        };
        self.stage = stage;
        self.round_name = def.name.clone();
        self.round_kind = round.kind().to_string();
        self.combats.clear();
        self.emit(GameEvent::RoundStarted {
            index: self.round_index,
            name: def.name.clone(),
            kind: round.kind().to_string(),
            stage,
        });
        let economy = &content.rules().economy;
        for id in self.alive() {
            if self.round_index > 0 {
                let p = self.player(id);
                let interest = (p.gold / economy.interest_per).clamp(0, economy.interest_cap);
                let streak = p.streak.unsigned_abs();
                let streak_gold = economy
                    .streak_bonuses
                    .iter()
                    .filter(|(at, _)| *at <= streak)
                    .map(|(_, gold)| *gold)
                    .max()
                    .unwrap_or(0);
                self.add_gold(id, economy.round_income + interest + streak_gold);
                self.add_xp(id, economy.xp_per_round);
            }
            let p = &mut self.players[id.0 as usize];
            p.ready = false;
            if p.shop_locked {
                p.shop_locked = false;
            } else {
                self.roll_shop(id);
            }
        }
        self.phase = Phase::Planning;
        round.begin(self);
        self.current = Some(round);
        Ok(())
    }

    /// Resolves the current round and eliminates players without health.
    pub fn finish_round(&mut self) -> Result<(), CommandError> {
        if self.phase != Phase::Planning {
            return Err(CommandError::NotPlanning);
        }
        let mut round = self.current.take().expect("a round is in progress");
        round.finish(self);
        self.emit(GameEvent::RoundFinished { index: self.round_index });

        // Eliminations: among players who fell this round, more remaining
        // health places higher; ties go to the earlier seat.
        let alive_before = self.players.iter().filter(|p| p.alive).count() as u32;
        let mut fallen: Vec<(i64, PlayerId)> =
            self.players.iter().filter(|p| p.alive && p.health <= 0).map(|p| (p.health, p.id)).collect();
        fallen.sort_by_key(|(health, id)| (*health, std::cmp::Reverse(*id)));
        for (i, (_, id)) in fallen.iter().enumerate() {
            let placement = alive_before - i as u32;
            self.eliminate(*id, placement);
        }
        let alive = self.alive();
        if alive.len() <= 1 {
            if let Some(last) = alive.first() {
                self.players[last.0 as usize].placement = Some(1);
            }
            let winner = self.winner().expect("someone placed first");
            self.phase = Phase::Over;
            self.emit(GameEvent::GameOver { winner });
        } else {
            self.phase = Phase::BetweenRounds;
            self.round_index += 1;
        }
        Ok(())
    }

    fn eliminate(&mut self, id: PlayerId, placement: u32) {
        let p = &mut self.players[id.0 as usize];
        p.alive = false;
        p.placement = Some(placement);
        let units: Vec<OwnedUnit> =
            std::mem::take(&mut p.board).into_values().chain(p.bench.iter_mut().filter_map(Option::take)).collect();
        for unit in units {
            self.return_to_pool(&unit);
        }
        self.emit(GameEvent::PlayerEliminated { player: id, placement });
    }

    // ----- commands -------------------------------------------------------

    /// Applies a player's command, or rejects it without changing anything.
    pub fn apply(&mut self, id: PlayerId, command: Command) -> Result<(), CommandError> {
        if self.phase == Phase::Over {
            return Err(CommandError::GameOver);
        }
        if self.phase != Phase::Planning {
            return Err(CommandError::NotPlanning);
        }
        let Some(player) = self.players.get(id.0 as usize) else {
            return Err(CommandError::UnknownPlayer);
        };
        if !player.alive {
            return Err(CommandError::Eliminated);
        }
        match command {
            Command::BuyUnit { slot } => self.buy_unit(id, slot),
            Command::SellUnit { uid } => self.sell_unit(id, uid),
            Command::Reroll => {
                self.spend(id, self.content.rules().economy.reroll_cost)?;
                self.roll_shop(id);
                Ok(())
            }
            Command::BuyXp => {
                let rules = self.content.rules();
                if self.player(id).level as usize >= rules.levels.len() {
                    return Err(CommandError::MaxLevel);
                }
                let (cost, xp) = (rules.economy.xp_cost, rules.economy.xp_per_purchase);
                self.spend(id, cost)?;
                self.add_xp(id, xp);
                Ok(())
            }
            Command::SetShopLock { locked } => {
                self.players[id.0 as usize].shop_locked = locked;
                Ok(())
            }
            Command::MoveUnit { uid, to } => self.move_unit(id, uid, to),
            Command::EquipItem { item, uid } => self.equip_item(id, item, uid),
            Command::Pick { option } => {
                let mut round = self.current.take().expect("a round is in progress");
                let result = round.pick(self, id, option);
                self.current = Some(round);
                result
            }
            Command::Ready => {
                self.players[id.0 as usize].ready = true;
                self.emit(GameEvent::PlayerReady { player: id });
                Ok(())
            }
        }
    }

    fn spend(&mut self, id: PlayerId, cost: i64) -> Result<(), CommandError> {
        let have = self.player(id).gold;
        if have < cost {
            return Err(CommandError::NotEnoughGold { need: cost, have });
        }
        self.add_gold(id, -cost);
        Ok(())
    }

    fn buy_unit(&mut self, id: PlayerId, slot: usize) -> Result<(), CommandError> {
        let content = self.content.clone();
        let p = self.player(id);
        let unit_id = p.shop.get(slot).cloned().flatten().ok_or(CommandError::EmptyShopSlot)?;
        let cost = content.unit(&unit_id).map(|u| u.cost as i64).unwrap_or(0);
        if p.gold < cost {
            return Err(CommandError::NotEnoughGold { need: cost, have: p.gold });
        }
        if self.pool_count(&unit_id) == 0 {
            return Err(CommandError::SoldOut);
        }
        // A full bench is fine if this copy completes an upgrade.
        let completes_upgrade = p.units().filter(|u| u.unit == unit_id && u.star == 1).count() as u32 + 1
            >= content.rules().copies_to_upgrade
            && content.rules().star_levels.len() > 1;
        if p.bench.iter().all(Option::is_some) && !completes_upgrade {
            return Err(CommandError::BenchFull);
        }
        *self.pool.get_mut(&unit_id).unwrap() -= 1;
        self.players[id.0 as usize].shop[slot] = None;
        self.add_gold(id, -cost);
        self.add_unit(id, &unit_id, Vec::new());
        Ok(())
    }

    fn sell_unit(&mut self, id: PlayerId, uid: u32) -> Result<(), CommandError> {
        let p = &mut self.players[id.0 as usize];
        let slot = p.locate(uid).ok_or(CommandError::UnknownUnit)?;
        let unit = p.take(slot).unwrap();
        p.items.extend(unit.items.iter().cloned());
        let cost = self.content.unit(&unit.unit).map(|u| u.cost as i64).unwrap_or(0);
        let copies = self.copies_in(&unit);
        self.return_to_pool(&unit);
        self.emit(GameEvent::UnitSold { player: id, uid, unit: unit.unit.clone() });
        self.add_gold(id, cost * copies as i64);
        Ok(())
    }

    fn move_unit(&mut self, id: PlayerId, uid: u32, to: Slot) -> Result<(), CommandError> {
        let rules = self.content.rules();
        let valid = match to {
            Slot::Bench(i) => i < rules.bench_size,
            Slot::Board(c, r) => {
                (0..rules.board_cols as i32).contains(&c)
                    && ((rules.board_rows / 2) as i32..rules.board_rows as i32).contains(&r)
            }
        };
        if !valid {
            return Err(CommandError::InvalidSlot);
        }
        let limit = self.board_limit(id);
        let p = &mut self.players[id.0 as usize];
        let from = p.locate(uid).ok_or(CommandError::UnknownUnit)?;
        if from == to {
            return Ok(());
        }
        let occupied = match to {
            Slot::Bench(i) => p.bench[i].is_some(),
            Slot::Board(c, r) => p.board.contains_key(&(c, r)),
        };
        let grows_board = matches!(to, Slot::Board(..)) && matches!(from, Slot::Bench(_)) && !occupied;
        if grows_board && p.board.len() >= limit {
            return Err(CommandError::BoardFull);
        }
        let moving = p.take(from).unwrap();
        if let Some(other) = p.take(to) {
            let other_uid = other.uid;
            p.put(from, other);
            self.emit(GameEvent::UnitMoved { player: id, uid: other_uid, to: from });
        }
        self.players[id.0 as usize].put(to, moving);
        self.emit(GameEvent::UnitMoved { player: id, uid, to });
        Ok(())
    }

    fn equip_item(&mut self, id: PlayerId, item: usize, uid: u32) -> Result<(), CommandError> {
        let content = self.content.clone();
        let max = content.rules().max_items_per_unit;
        let p = &mut self.players[id.0 as usize];
        let item_id = p.items.get(item).cloned().ok_or(CommandError::UnknownItem)?;
        let unit = p.unit_mut(uid).ok_or(CommandError::UnknownUnit)?;
        // A component may be added to a full unit if it combines with one
        // the unit already holds.
        let combines = unit.items.iter().any(|held| content.combine_items(held, &item_id).is_some());
        if unit.items.len() >= max && !combines {
            return Err(CommandError::ItemSlotsFull);
        }
        unit.items.push(item_id.clone());
        p.items.remove(item);
        self.emit(GameEvent::ItemEquipped { player: id, uid, item: item_id });
        self.combine_items_on(id, uid);
        Ok(())
    }

    /// Merges pairs of components on a unit into finished items.
    fn combine_items_on(&mut self, id: PlayerId, uid: u32) {
        let content = self.content.clone();
        loop {
            let unit = self.players[id.0 as usize].unit_mut(uid).expect("unit exists");
            let mut found = None;
            'search: for a in 0..unit.items.len() {
                for b in a + 1..unit.items.len() {
                    if let Some(result) = content.combine_items(&unit.items[a], &unit.items[b]) {
                        found = Some((a, b, result.id.clone()));
                        break 'search;
                    }
                }
            }
            let Some((a, b, result)) = found else { break };
            unit.items.remove(b);
            unit.items[a] = result.clone();
            self.emit(GameEvent::ItemCombined { player: id, uid, item: result });
        }
    }

    // ----- helpers for round types ----------------------------------------

    /// Records an event.
    pub fn emit(&mut self, event: GameEvent) {
        self.events.push(event);
    }

    /// The game's random stream. Round types must draw all randomness here
    /// to keep games reproducible.
    pub fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }

    pub fn add_gold(&mut self, id: PlayerId, amount: i64) {
        if amount == 0 {
            return;
        }
        let p = &mut self.players[id.0 as usize];
        p.gold = (p.gold + amount).max(0);
        let gold = p.gold;
        self.emit(GameEvent::GoldChanged { player: id, gold });
    }

    pub fn add_xp(&mut self, id: PlayerId, amount: u32) {
        let levels = &self.content.rules().levels;
        let p = &mut self.players[id.0 as usize];
        if p.level as usize >= levels.len() || amount == 0 {
            return;
        }
        p.xp += amount;
        while (p.level as usize) < levels.len() && p.xp >= levels[p.level as usize - 1].xp_to_next {
            p.xp -= levels[p.level as usize - 1].xp_to_next;
            p.level += 1;
        }
        if p.level as usize >= levels.len() {
            p.xp = 0;
        }
        let (level, xp) = (p.level, p.xp);
        self.emit(GameEvent::XpChanged { player: id, level, xp });
    }

    /// Adds an item to a player's inventory.
    pub fn give_item(&mut self, id: PlayerId, item: &str) {
        self.players[id.0 as usize].items.push(item.to_string());
        self.emit(GameEvent::ItemGained { player: id, item: item.to_string() });
    }

    /// Ids of items that are not made from other items, ordered by id.
    pub fn component_items(&self) -> Vec<String> {
        self.content.items().filter(|i| i.recipe.is_none()).map(|i| i.id.clone()).collect()
    }

    /// Gives a random component item, if the content has any.
    pub fn give_random_component(&mut self, id: PlayerId) {
        let components = self.component_items();
        if !components.is_empty() {
            let pick = self.rng.below(components.len() as u64) as usize;
            self.give_item(id, &components[pick]);
        }
    }

    /// Gives a player an augment: its bonuses apply to their team from now on.
    pub fn give_augment(&mut self, id: PlayerId, augment: &str) {
        let gold = self.content.augment(augment).map(|a| a.gold).unwrap_or(0);
        self.players[id.0 as usize].augments.push(augment.to_string());
        self.emit(GameEvent::AugmentGained { player: id, augment: augment.to_string() });
        self.add_gold(id, gold);
    }

    /// Takes one copy of a unit out of the shared pool, if any are left.
    pub fn take_from_pool(&mut self, unit: &str) -> bool {
        match self.pool.get_mut(unit) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        }
    }

    /// Puts one copy of a unit back into the shared pool.
    pub fn put_in_pool(&mut self, unit: &str) {
        if let Some(n) = self.pool.get_mut(unit) {
            *n += 1;
        }
    }

    /// Units a shop or draft may currently offer, with the copies left of
    /// each, ordered by id.
    pub fn available_units(&self, max_cost: Option<u32>) -> Vec<(String, u32)> {
        self.pool
            .iter()
            .filter(|(id, n)| {
                **n > 0 && max_cost.is_none_or(|max| self.content.unit(id).is_some_and(|u| u.cost <= max))
            })
            .map(|(id, n)| (id.clone(), *n))
            .collect()
    }

    /// Gives a player a one-star unit (not taken from the pool; use
    /// [`Game::take_from_pool`] first if it should be). It goes to the bench,
    /// or is refunded as gold if there is no room and it completes no upgrade.
    pub fn add_unit(&mut self, id: PlayerId, unit_id: &str, items: Vec<String>) {
        let uid = self.next_uid;
        self.next_uid += 1;
        let unit = OwnedUnit { uid, unit: unit_id.to_string(), star: 1, items };
        let p = &mut self.players[id.0 as usize];
        match p.bench.iter().position(Option::is_none) {
            Some(free) => p.bench[free] = Some(unit),
            // Temporarily over capacity; an upgrade must free the space.
            None => p.bench.push(Some(unit)),
        }
        self.emit(GameEvent::UnitGained { player: id, uid, unit: unit_id.to_string() });
        self.check_upgrades(id, unit_id);

        let bench_size = self.content.rules().bench_size;
        let p = &mut self.players[id.0 as usize];
        if p.bench.len() > bench_size {
            if let Some(extra) = p.bench.pop().flatten() {
                // No room after all: refund it.
                p.items.extend(extra.items.iter().cloned());
                let cost = self.content.unit(&extra.unit).map(|u| u.cost as i64).unwrap_or(0);
                self.return_to_pool(&extra);
                self.emit(GameEvent::UnitSold { player: id, uid: extra.uid, unit: extra.unit.clone() });
                self.add_gold(id, cost);
            }
            self.players[id.0 as usize].bench.truncate(bench_size);
        }
    }

    /// Merges matching copies into higher star levels for as long as possible.
    fn check_upgrades(&mut self, id: PlayerId, unit_id: &str) {
        let rules = self.content.rules();
        let (needed, max_star, max_items) =
            (rules.copies_to_upgrade as usize, rules.star_levels.len() as u8, rules.max_items_per_unit);
        let mut star = 1u8;
        while star < max_star {
            let p = &mut self.players[id.0 as usize];
            // Board copies first, so the upgraded unit stays where it fights.
            let matching: Vec<u32> =
                p.units().filter(|u| u.unit == unit_id && u.star == star).map(|u| u.uid).take(needed).collect();
            if matching.len() < needed {
                star += 1;
                continue;
            }
            let keep = matching[0];
            let mut items = Vec::new();
            for uid in &matching[1..] {
                let slot = p.locate(*uid).unwrap();
                items.extend(p.take(slot).unwrap().items);
            }
            let mut overflow = Vec::new();
            let kept = p.unit_mut(keep).unwrap();
            kept.star = star + 1;
            for item in items {
                if kept.items.len() < max_items {
                    kept.items.push(item);
                } else {
                    overflow.push(item);
                }
            }
            p.items.extend(overflow);
            self.emit(GameEvent::UnitUpgraded { player: id, uid: keep, unit: unit_id.to_string(), star: star + 1 });
            self.combine_items_on(id, keep);
            // The new unit may complete a set at the next star level.
            star += 1;
        }
    }

    /// One-star copies a unit stands for.
    fn copies_in(&self, unit: &OwnedUnit) -> u32 {
        self.content.rules().copies_to_upgrade.pow(unit.star.saturating_sub(1) as u32)
    }

    fn return_to_pool(&mut self, unit: &OwnedUnit) {
        let copies = self.copies_in(unit);
        if let Some(n) = self.pool.get_mut(&unit.unit) {
            *n += copies;
        }
    }

    fn roll_shop(&mut self, id: PlayerId) {
        let content = self.content.clone();
        let rules = content.rules();
        let level = self.player(id).level as usize;
        let odds = &rules.levels[(level - 1).min(rules.levels.len() - 1)].shop_odds;
        let mut offers = Vec::with_capacity(rules.shop_size);
        for _ in 0..rules.shop_size {
            let available = self.available_units(None);
            let of_cost = |cost: u32| -> Vec<&(String, u32)> {
                available.iter().filter(|(u, _)| content.unit(u).is_some_and(|d| d.cost == cost)).collect()
            };
            let tier_weights: Vec<u64> = odds
                .iter()
                .enumerate()
                .map(|(i, w)| if of_cost(i as u32 + 1).is_empty() { 0 } else { *w as u64 })
                .collect();
            let offer = self.rng.weighted_index(&tier_weights).and_then(|tier| {
                let candidates = of_cost(tier as u32 + 1);
                let weights: Vec<u64> = candidates.iter().map(|(_, n)| *n as u64).collect();
                self.rng.weighted_index(&weights).map(|i| candidates[i].0.clone())
            });
            offers.push(offer);
        }
        self.players[id.0 as usize].shop = offers.clone();
        self.emit(GameEvent::ShopRolled { player: id, offers });
    }

    /// A player's board and augments as input for combat.
    pub fn team_setup(&self, id: PlayerId) -> TeamSetup {
        let p = self.player(id);
        let bonuses = p
            .augments
            .iter()
            .filter_map(|a| self.content.augment(a))
            .flat_map(|a| a.bonuses.iter().cloned())
            .collect();
        TeamSetup { units: p.board_units(), bonuses }
    }

    /// Runs a fight for `home` and records it. `neutral` supplies the enemy
    /// board for [`Opponent::Neutral`].
    pub fn fight(&mut self, home: PlayerId, away: Opponent, neutral: &[BoardUnit]) -> FightResult {
        let seed = self.rng.next_u64();
        let away_setup = match &away {
            Opponent::Player(p) | Opponent::Ghost(p) => self.team_setup(*p),
            Opponent::Neutral => TeamSetup { units: neutral.to_vec(), bonuses: Vec::new() },
        };
        let teams = vec![self.team_setup(home), away_setup];
        let mut combat = self.content.setup_combat(seed, &teams).expect("boards are validated when they are built");
        let outcome = combat.run().clone();
        let survivors = match outcome.winner {
            Some(team) => combat.units().iter().filter(|u| u.is_alive() && u.team() == team).count() as u32,
            None => 0,
        };
        let index = self.combats.len();
        self.combats.push(CombatRecord {
            home,
            away: away.clone(),
            seed,
            tick_ms: combat.config().tick_ms,
            winner: outcome.winner,
            duration_ms: outcome.duration_ms,
            survivors,
            teams,
            events: combat.drain_events(),
        });
        self.emit(GameEvent::CombatFinished { combat: index, home, away, winner: outcome.winner });
        FightResult { home_won: outcome.winner.map(|t| t == Team(0)), survivors }
    }

    /// Damage for losing a fight at the current stage with this many enemy
    /// units left standing.
    pub fn loss_damage(&self, survivors: u32) -> i64 {
        let rules = self.content.rules();
        let table = &rules.base_damage_by_stage;
        let stage = self.stage.max(1) as usize;
        let base = match table.get(stage - 1) {
            Some(d) => *d,
            None => table.last().copied().unwrap_or(0) + (stage - table.len()) as i64,
        };
        base + survivors as i64 * rules.damage_per_survivor
    }

    pub fn damage_player(&mut self, id: PlayerId, amount: i64) {
        let p = &mut self.players[id.0 as usize];
        p.health -= amount;
        let health = p.health;
        self.emit(GameEvent::PlayerDamaged { player: id, amount, health });
    }

    /// Updates a player's streak after a fight against another player and
    /// pays the win bonus.
    pub fn record_result(&mut self, id: PlayerId, won: bool) {
        let p = &mut self.players[id.0 as usize];
        p.streak = if won { p.streak.max(0) + 1 } else { p.streak.min(0) - 1 };
        if won {
            let bonus = self.content.rules().economy.win_bonus;
            self.add_gold(id, bonus);
        }
    }
}
