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
use crate::content::{ActionDef, ActionTarget, BoardUnit, ChoiceOffer, ContentSet, RoundKindDef, ShopSpecial, TeamSetup};
use crate::perk::{Perk, Rule, RuleMod, When};
use crate::stats::StatMod;
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
    /// Stat changes this unit has earned for good, one entry per stat.
    #[serde(default)]
    pub growth: Vec<StatMod>,
    /// Traits this one unit counts as having besides its own.
    #[serde(default)]
    pub traits: Vec<String>,
}

impl OwnedUnit {
    /// Adds to the stat changes the unit keeps, merging by stat.
    fn grow(&mut self, mods: &[StatMod]) {
        for m in mods {
            match self.growth.iter_mut().find(|g| g.stat == m.stat) {
                Some(g) => {
                    g.flat += m.flat;
                    g.permille += m.permille;
                }
                None => self.growth.push(*m),
            }
        }
    }
}

/// What a special shop offer comes with besides the unit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopOffer {
    pub star: u8,
    /// A trait the unit gets, if any.
    pub extra_trait: Option<String>,
}

/// A pick waiting on one player, opened by a [`Grant::Choice`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingChoice {
    pub offer: ChoiceOffer,
    /// Ids of what can be picked.
    pub options: Vec<String>,
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
    /// Swap the options the current round offers for new ones, where the
    /// round allows it (see [`Game::rerolls_left`]).
    RerollPick,
    /// Use one of the game's own actions (see
    /// [`ActionDef`](crate::content::ActionDef)). `arg` is whatever the
    /// action needs: the `uid` of a unit for an action aimed at one, and
    /// otherwise a number only the game's own code gives a meaning to.
    Action {
        id: String,
        #[serde(default)]
        arg: i64,
    },
    /// Signal that the player is done for this round.
    /// Sends a unit to a teammate's bench (see `GameRules::team_size`),
    /// with its items.
    Gift { uid: u32, to: PlayerId },
    Ready,
    /// Give up. The player leaves when the round ends, placed below
    /// everyone else who goes out that round; any pick the round is waiting
    /// on them for is made for them. It cannot be taken back.
    Forfeit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandError {
    /// Commands are only accepted between `begin_round` and `finish_round`.
    NotPlanning,
    UnknownPlayer,
    Eliminated,
    NotEnoughGold { need: i64, have: i64 },
    /// Not enough of a currency other than gold.
    NotEnough { currency: String, need: i64, have: i64 },
    /// The game has no currency with that id.
    UnknownCurrency,
    /// The game has no action with that id.
    UnknownAction,
    /// The action has been used as often as it can be this round.
    ActionLimit { per_round: u32 },
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
    /// The player has forfeited and only waits for the round to end.
    Forfeited,
    /// That player is not on the same team (or is out of the game).
    NotATeammate,
    /// A single-use item cannot do anything to that unit; the reason says why.
    CannotUse(String),
    NoRerollsLeft,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::NotPlanning => write!(f, "no round is in progress"),
            CommandError::UnknownPlayer => write!(f, "no such player"),
            CommandError::Eliminated => write!(f, "the player has been eliminated"),
            CommandError::NotEnoughGold { need, have } => write!(f, "needs {need} gold, has {have}"),
            CommandError::NotEnough { currency, need, have } => write!(f, "needs {need} {currency}, has {have}"),
            CommandError::UnknownCurrency => write!(f, "the game has no such currency"),
            CommandError::UnknownAction => write!(f, "the game has no such action"),
            CommandError::ActionLimit { per_round: 1 } => write!(f, "already used this round"),
            CommandError::ActionLimit { per_round } => write!(f, "already used {per_round} times this round"),
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
            CommandError::NotATeammate => write!(f, "that player is not a teammate still in the game"),
            CommandError::Forfeited => write!(f, "the player has forfeited"),
            CommandError::CannotUse(why) => write!(f, "{why}"),
            CommandError::NoRerollsLeft => write!(f, "no rerolls of these options are left"),
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
    /// Has given up, and goes out when the round ends.
    #[serde(default)]
    pub forfeited: bool,
    /// Rerolls that cost no gold, used first.
    #[serde(default)]
    pub free_rerolls: u32,
    /// How the game has gone for this player so far.
    #[serde(default)]
    pub stats: PlayerStats,
    /// How much the player has of each currency besides gold, by id (see
    /// `GameRules::currencies`).
    #[serde(default)]
    pub wallet: BTreeMap<String, i64>,
    /// How often the player has used each of the game's actions this round.
    #[serde(default)]
    pub actions_used: BTreeMap<String, u32>,
    /// What each shop offer comes with besides the unit, slot for slot with
    /// `shop` (see `GameRules::shop_specials`). `None` for a plain offer.
    #[serde(default)]
    pub shop_specials: Vec<Option<ShopOffer>>,
    /// Picks waiting on the player, oldest first (see [`Grant::Choice`]).
    /// `Command::Pick` answers the first one.
    #[serde(default)]
    pub choices: Vec<PendingChoice>,
}

/// Running totals for one player, for end-of-game summaries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerStats {
    /// Rounds started while still in the game.
    pub rounds: u32,
    pub gold_earned: i64,
    pub gold_spent: i64,
    pub rerolls: u32,
    pub xp_bought: u32,
    pub units_bought: u32,
    pub units_sold: u32,
    /// Fights against other players' boards, ghosts included, and neutral
    /// camps.
    pub fights_won: u32,
    pub fights_lost: u32,
    pub fights_drawn: u32,
    /// Enemy units that died in this player's fights.
    pub kills: u32,
    pub damage_taken: i64,
    /// The longest run of wins.
    pub best_streak: u32,
    /// The most stars any of the player's units reached.
    pub best_star: u8,
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
    /// How much the player has of a currency. `"gold"` is their gold.
    pub fn balance(&self, currency: &str) -> i64 {
        if currency == "gold" { self.gold } else { self.wallet.get(currency).copied().unwrap_or(0) }
    }

    /// Whether the player could use one of the game's actions right now, as
    /// far as its limit and price go. Clients use this to grey a button
    /// out; the action's own handler can still refuse.
    pub fn can_use(&self, action: &ActionDef) -> Result<(), CommandError> {
        if let Some(per_round) = action.per_round
            && self.actions_used.get(&action.id).copied().unwrap_or(0) >= per_round
        {
            return Err(CommandError::ActionLimit { per_round });
        }
        for price in &action.prices {
            let have = self.balance(&price.currency);
            if have < price.amount {
                return Err(match price.currency.as_str() {
                    "gold" => CommandError::NotEnoughGold { need: price.amount, have },
                    _ => CommandError::NotEnough { currency: price.currency.clone(), need: price.amount, have },
                });
            }
        }
        Ok(())
    }

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
            .map(|(pos, u)| BoardUnit {
                unit: u.unit.clone(),
                star: u.star,
                items: u.items.clone(),
                pos: *pos,
                growth: u.growth.clone(),
                traits: u.traits.clone(),
            })
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
    /// The [tiles](crate::tile) the fight started with.
    #[serde(default)]
    pub tiles: Vec<crate::tile::TilePlacement>,
    pub events: Vec<Event>,
}

/// Something that happened in the game outside of a fight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameEvent {
    RoundStarted { index: usize, name: String, kind: String, stage: u32 },
    RoundFinished { index: usize },
    ShopRolled { player: PlayerId, offers: Vec<Option<String>> },
    GoldChanged { player: PlayerId, gold: i64 },
    /// A player now has `amount` of a currency other than gold.
    CurrencyChanged { player: PlayerId, currency: String, amount: i64 },
    /// A player used one of the game's own actions, and has paid for it.
    ActionUsed { player: PlayerId, action: String, arg: i64 },
    XpChanged { player: PlayerId, level: u32, xp: u32 },
    UnitGained { player: PlayerId, uid: u32, unit: String },
    UnitSold { player: PlayerId, uid: u32, unit: String },
    /// A unit was taken away without being sold (see [`Game::remove_unit`]).
    UnitRemoved { player: PlayerId, uid: u32, unit: String },
    /// A unit went from one player to a teammate. It keeps its `uid`.
    UnitGifted { from: PlayerId, to: PlayerId, uid: u32, unit: String },
    UnitUpgraded { player: PlayerId, uid: u32, unit: String, star: u8 },
    UnitMoved { player: PlayerId, uid: u32, to: Slot },
    ItemGained { player: PlayerId, item: String },
    ItemEquipped { player: PlayerId, uid: u32, item: String },
    ItemCombined { player: PlayerId, uid: u32, item: String },
    /// A single-use item was used on a unit.
    ItemUsed { player: PlayerId, uid: u32, item: String },
    /// Options offered to one player, or to everyone when `player` is `None`.
    ChoiceOffered { player: Option<PlayerId>, options: Vec<String> },
    ChoiceMade { player: PlayerId, option: usize, choice: String },
    AugmentGained { player: PlayerId, augment: String },
    /// This game's twist (see `GameRules::lobby_modifiers`), which every
    /// player has.
    LobbyModifier { augment: String },
    PlayerReady { player: PlayerId },
    /// Index into [`Game::combats`] for the full record.
    CombatFinished { combat: usize, home: PlayerId, away: Opponent, winner: Option<Team> },
    PlayerDamaged { player: PlayerId, amount: i64, health: i64 },
    /// A player got `amount` health back and now has `health`.
    PlayerHealed { player: PlayerId, amount: i64, health: i64 },
    PlayerForfeited { player: PlayerId },
    PlayerEliminated { player: PlayerId, placement: u32 },
    GameOver { winner: PlayerId },
    /// A line a script logged, or an error it ran into.
    Script { script: String, message: String },
}

/// Code that reacts to what happens in a game.
///
/// Hooks are called with every [`GameEvent`], after the command or round
/// step that produced it has finished, and may change the game in response;
/// events they cause are delivered too. This is how scripts listen in, and
/// games built on the engine can register their own with [`Game::add_hook`].
pub trait GameHook: Send {
    fn on_event(&mut self, game: &mut Game, event: &GameEvent);
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
    hooks: Vec<Box<dyn GameHook>>,
    /// How many of `events` the hooks have been shown.
    hooks_seen: usize,
    /// The seed the game was created with, for [`Game::record`].
    seed: u64,
    /// Every input that changed the game, in order, for [`Game::record`].
    inputs: Vec<Input>,
    /// Each player's most recent player opponents, oldest first.
    opponents: BTreeMap<PlayerId, Vec<PlayerId>>,
    /// The augment every player got at the start, if the game has one.
    lobby_modifier: Option<String>,
    /// How deep perks are nested: what one hands out can set off another.
    perk_depth: u32,
}

/// Something from outside that moved the game along. A game is its content,
/// its seed, its players' names and the list of these: replaying them gives
/// back exactly the same game (see [`crate::matchlog`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    BeginRound,
    FinishRound,
    Command { player: PlayerId, command: Command },
    Grant { player: PlayerId, grant: Grant },
}

/// Something handed to a player from outside the rules: developer cheats,
/// admin tools, rewards from a tournament host. Unlike calling
/// [`Game::add_gold`] and friends directly, a grant is recorded, so games
/// that use them can still be replayed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grant {
    Gold(i64),
    Xp(u32),
    Unit(String),
    Item(String),
    Augment(String),
    Damage(i64),
    /// Health back, up to what players start with.
    Heal(i64),
    /// Some of a currency other than gold; negative takes it away.
    Currency { currency: String, amount: i64 },
    /// A random component item.
    Component,
    /// A random item with this [tag](crate::content::ItemDef::tag).
    ItemTagged(String),
    /// A random unit of this cost, taken out of the shared pool.
    UnitOfCost(u32),
    /// Rerolls that cost nothing.
    Rerolls(u32),
    /// One roll on a [loot table](crate::perk::LootTable).
    Loot(String),
    /// Opens a pick of one out of `options` for the player (see
    /// [`Player::choices`]). One not made by the end of the round is made
    /// at random.
    Choice { options: u32, offer: ChoiceOffer },
}

impl std::fmt::Display for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Grant::Gold(n) => write!(f, "{n} gold"),
            Grant::Xp(n) => write!(f, "{n} experience"),
            Grant::Unit(id) => write!(f, "a unit ({id})"),
            Grant::Item(id) => write!(f, "an item ({id})"),
            Grant::Augment(id) => write!(f, "an augment ({id})"),
            Grant::Damage(n) => write!(f, "{n} damage"),
            Grant::Heal(n) => write!(f, "{n} health"),
            Grant::Currency { currency, amount } => write!(f, "{amount} {currency}"),
            Grant::Component => write!(f, "a random component"),
            Grant::ItemTagged(tag) => write!(f, "a random {tag} item"),
            Grant::UnitOfCost(cost) => write!(f, "a random unit of cost {cost}"),
            Grant::Rerolls(n) => write!(f, "{n} free rerolls"),
            Grant::Loot(table) => write!(f, "a roll for {table}"),
            Grant::Choice { options, .. } => write!(f, "a pick of one out of {options}"),
        }
    }
}

/// How deep loot tables and perks may set each other off.
const MAX_GRANT_DEPTH: u32 = 8;

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
                let by_cost = rules.pool_sizes.get(&u.cost).copied().unwrap_or(rules.default_pool_size);
                let size = u.pool_size.unwrap_or(by_cost);
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
                forfeited: false,
                free_rerolls: 0,
                stats: PlayerStats::default(),
                wallet: rules.currencies.iter().map(|c| (c.id.clone(), c.starting.max(0))).collect(),
                actions_used: BTreeMap::new(),
                shop_specials: Vec::new(),
                choices: Vec::new(),
            })
            .collect();
        #[cfg(feature = "scripting")]
        let scripts = content.scripts().clone();
        #[cfg_attr(not(feature = "scripting"), allow(unused_mut))]
        let mut game = Game {
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
            hooks: Vec::new(),
            hooks_seen: 0,
            seed,
            inputs: Vec::new(),
            opponents: BTreeMap::new(),
            lobby_modifier: None,
            perk_depth: 0,
        };
        let modifiers = game.content.rules().lobby_modifiers.clone();
        if !modifiers.is_empty() {
            let pick = modifiers[game.rng.below(modifiers.len() as u64) as usize].clone();
            game.emit(GameEvent::LobbyModifier { augment: pick.clone() });
            for id in game.alive() {
                game.give_augment(id, &pick);
            }
            game.lobby_modifier = Some(pick);
        }
        #[cfg(feature = "scripting")]
        if !scripts.is_empty() {
            // Content is checked when it is built, so this only fails for
            // scripts that never went through that check.
            match crate::script::ScriptHost::compile(&scripts) {
                Ok(host) => Arc::new(host).install(&mut game),
                Err(e) => game.emit(GameEvent::Script { script: e.script, message: format!("error: {}", e.message) }),
            }
        }
        game
    }

    /// Subscribes a hook to everything that happens from now on.
    pub fn add_hook(&mut self, hook: Box<dyn GameHook>) {
        self.hooks.push(hook);
    }

    /// Shows the hooks every event they have not seen yet, including the
    /// ones they cause, up to a limit that stops hooks feeding each other
    /// forever.
    fn run_hooks(&mut self) {
        if self.hooks.is_empty() {
            self.hooks_seen = self.events.len();
            return;
        }
        let mut hooks = std::mem::take(&mut self.hooks);
        let mut budget = 10_000;
        while self.hooks_seen < self.events.len() && budget > 0 {
            let event = self.events[self.hooks_seen].clone();
            self.hooks_seen += 1;
            budget -= 1;
            for hook in &mut hooks {
                hook.on_event(self, &event);
            }
        }
        self.hooks_seen = self.events.len();
        // Hooks added while running come after the existing ones.
        hooks.append(&mut self.hooks);
        self.hooks = hooks;
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
    /// The [tiles](crate::tile) the current round's fights start with.
    pub fn round_tiles(&self) -> &[crate::tile::TilePlacement] {
        &self.content.schedule().round(self.round_index).0.tiles
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
    /// Copies of a unit left in the shared pool. Copies offered in a shop or
    /// a draft are held out of the pool until they are bought or put back.
    pub fn pool_count(&self, unit: &str) -> u32 {
        self.pool.get(unit).copied().unwrap_or(0)
    }
    /// The whole shared pool: copies left of every shop unit.
    pub fn pool(&self) -> &BTreeMap<String, u32> {
        &self.pool
    }
    /// How many units a player may field at their level.
    pub fn board_limit(&self, id: PlayerId) -> usize {
        self.rule(id, Rule::TeamSize) as usize
    }
    /// The team a player is on (see `GameRules::team_size`). With teams of
    /// one, every player is a team of their own.
    pub fn team_of(&self, id: PlayerId) -> u32 {
        id.0 as u32 / self.content.rules().team_size.max(1)
    }
    /// Whether two different players are on the same team.
    pub fn teammates(&self, a: PlayerId, b: PlayerId) -> bool {
        a != b && self.team_of(a) == self.team_of(b)
    }
    /// The teams that still have someone in the game, in seat order.
    pub fn alive_teams(&self) -> Vec<u32> {
        let mut teams: Vec<u32> = self.alive().into_iter().map(|p| self.team_of(p)).collect();
        teams.dedup();
        teams
    }
    /// What it costs to buy the offer in a shop slot, or `None` for an
    /// empty slot.
    pub fn shop_price(&self, id: PlayerId, slot: usize) -> Option<i64> {
        let p = self.player(id);
        let unit = p.shop.get(slot).cloned().flatten()?;
        let star = p.shop_specials.get(slot).cloned().flatten().map_or(1, |s| s.star.max(1));
        let cost = self.content.unit(&unit).map_or(0, |u| u.cost as i64);
        Some(cost * self.content.rules().copies_to_upgrade.pow(star as u32 - 1) as i64)
    }
    /// How many team slots the units on a player's board take.
    pub fn board_slots_used(&self, id: PlayerId) -> usize {
        self.player(id).board.values().map(|u| self.slots_of(u)).sum()
    }
    fn slots_of(&self, unit: &OwnedUnit) -> usize {
        self.content.unit(&unit.unit).map_or(1, |d| d.team_slots as usize)
    }
    /// What a [rule](Rule) is for one player right now: the game's rule
    /// plus the changes from their augments, active trait tiers and the
    /// items their board units hold.
    pub fn rule(&self, id: PlayerId, rule: Rule) -> i64 {
        let rules = self.content.rules();
        let economy = &rules.economy;
        let level = self.player(id).level as usize;
        let base = match rule {
            Rule::TeamSize => rules.levels[(level.max(1) - 1).min(rules.levels.len() - 1)].board_size as i64,
            Rule::RerollCost => economy.reroll_cost,
            Rule::XpCost => economy.xp_cost,
            Rule::XpPerPurchase => economy.xp_per_purchase as i64,
            Rule::XpPerRound => economy.xp_per_round as i64,
            Rule::RoundIncome => economy.round_income,
            Rule::InterestCap => economy.interest_cap,
            Rule::WinBonus => economy.win_bonus,
            Rule::DamageTaken | Rule::ShopLevel => 0,
        };
        let total = base + self.perks_and_rules(id).1.iter().filter(|m| m.rule == rule).map(|m| m.add).sum::<i64>();
        match rule {
            Rule::DamageTaken | Rule::ShopLevel => total,
            _ => total.max(0),
        }
    }
    /// The perks and rule changes a player has right now, with where each
    /// perk comes from left out: augments first, then active trait tiers,
    /// then items held by board units.
    pub fn perks_and_rules(&self, id: PlayerId) -> (Vec<Perk>, Vec<RuleMod>) {
        let content = &self.content;
        let p = self.player(id);
        let (mut perks, mut rules) = (Vec::new(), Vec::new());
        for augment in p.augments.iter().filter_map(|a| content.augment(a)) {
            perks.extend(augment.perks.iter().cloned());
            rules.extend(augment.rules.iter().copied());
        }
        // Content is checked when it is built, so a board the game made
        // always counts.
        for active in content.active_traits(&p.board_units()).unwrap_or_default() {
            let tier = active.tier.and_then(|t| content.trait_def(&active.id).and_then(|d| d.breakpoints.get(t)));
            if let Some(tier) = tier {
                perks.extend(tier.perks.iter().cloned());
                rules.extend(tier.rules.iter().copied());
            }
        }
        for item in p.board.values().flat_map(|u| &u.items).filter_map(|i| content.item(i)) {
            perks.extend(item.perks.iter().cloned());
            rules.extend(item.rules.iter().copied());
        }
        (perks, rules)
    }
    /// Players the game is waiting on for a pick: those the current round
    /// waits for, and those with a [pick of their own](Player::choices).
    pub fn awaiting(&self) -> Vec<PlayerId> {
        let mut waiting = self.round_awaiting();
        for p in self.players.iter().filter(|p| p.alive && !p.choices.is_empty()) {
            if !waiting.contains(&p.id) {
                waiting.push(p.id);
            }
        }
        waiting
    }
    fn round_awaiting(&self) -> Vec<PlayerId> {
        self.current.as_ref().map(|r| r.awaiting(self)).unwrap_or_default()
    }
    /// How many more times a player may reroll what the current round
    /// offers them.
    pub fn rerolls_left(&self, id: PlayerId) -> u32 {
        self.current.as_ref().map(|r| r.rerolls_left(self, id)).unwrap_or(0)
    }
    /// What the current round offers a player; `None` entries are taken.
    pub fn options(&self, id: PlayerId) -> Vec<Option<String>> {
        let own = self.players.get(id.0 as usize).and_then(|p| p.choices.first());
        match own {
            // The round's pick comes first when it is waiting on them.
            Some(choice) if !self.round_awaiting().contains(&id) => choice.options.iter().cloned().map(Some).collect(),
            _ => self.current.as_ref().map(|r| r.options(self, id)).unwrap_or_default(),
        }
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
        self.hooks_seen = 0;
        std::mem::take(&mut self.events)
    }
    /// Fights of the most recent round.
    pub fn combats(&self) -> &[CombatRecord] {
        &self.combats
    }
    /// The players `id` fought most recently (ghosts not counted), oldest
    /// first, as many as [`GameRules::avoid_rematch_rounds`](crate::content::GameRules) keeps.
    pub fn recent_opponents(&self, id: PlayerId) -> &[PlayerId] {
        self.opponents.get(&id).map(Vec::as_slice).unwrap_or_default()
    }
    /// How much pairing `a` with `b` now would repeat a recent fight: 0 if
    /// not at all, higher for more recent ones.
    pub fn rematch_cost(&self, a: PlayerId, b: PlayerId) -> u32 {
        let recent = self.recent_opponents(a);
        recent.iter().rposition(|p| *p == b).map(|i| i as u32 + 1).unwrap_or(0)
    }

    // ----- the round loop -------------------------------------------------

    /// Starts the next round: pays income, refreshes shops and hands control
    /// to the round type named by the schedule.
    pub fn begin_round(&mut self) -> Result<(), CommandError> {
        let result = self.begin_round_inner();
        if result.is_ok() {
            self.inputs.push(Input::BeginRound);
        }
        self.run_hooks();
        result
    }

    fn begin_round_inner(&mut self) -> Result<(), CommandError> {
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
                let interest = (p.gold / economy.interest_per).clamp(0, self.rule(id, Rule::InterestCap));
                let streak = p.streak.unsigned_abs();
                let streak_gold = economy
                    .streak_bonuses
                    .iter()
                    .filter(|(at, _)| *at <= streak)
                    .map(|(_, gold)| *gold)
                    .max()
                    .unwrap_or(0);
                self.add_gold(id, self.rule(id, Rule::RoundIncome) + interest + streak_gold);
                for currency in &content.rules().currencies {
                    self.add_currency(id, &currency.id, currency.per_round);
                }
                self.add_xp(id, self.rule(id, Rule::XpPerRound) as u32);
                self.augment_grants(id);
                self.fire(id, When::RoundStart);
            }
            let p = &mut self.players[id.0 as usize];
            p.ready = false;
            p.actions_used.clear();
            p.stats.rounds += 1;
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
        let result = self.finish_round_inner();
        if result.is_ok() {
            self.inputs.push(Input::FinishRound);
        }
        self.run_hooks();
        result
    }

    fn finish_round_inner(&mut self) -> Result<(), CommandError> {
        if self.phase != Phase::Planning {
            return Err(CommandError::NotPlanning);
        }
        if self.content.rules().auto_fill_board {
            for id in self.alive() {
                self.fill_board(id);
            }
        }
        let mut round = self.current.take().expect("a round is in progress");
        round.finish(self);
        // Picks of their own that players left open are made at random.
        for id in self.alive() {
            for _ in 0..64 {
                let Some(count) = self.player(id).choices.first().map(|c| c.options.len()) else { break };
                let option = self.rng.below(count.max(1) as u64) as usize;
                self.pick_own(id, option);
            }
        }
        self.emit(GameEvent::RoundFinished { index: self.round_index });

        // Eliminations: among players who fell this round, more remaining
        // health places higher; ties go to the earlier seat.
        let teams_before = self.alive_teams().len() as u32;
        // Players who forfeited go out below everyone else.
        let mut fallen: Vec<(bool, i64, PlayerId)> = self
            .players
            .iter()
            .filter(|p| p.alive && (p.health <= 0 || p.forfeited))
            .map(|p| (!p.forfeited, p.health, p.id))
            .collect();
        fallen.sort_by_key(|(stayed, health, id)| (*stayed, *health, std::cmp::Reverse(*id)));
        // Teammates go out together and share a placement. A player who
        // forfeits while their team plays on takes the lowest placement
        // still open.
        let falling: Vec<PlayerId> = fallen.iter().map(|(_, _, id)| *id).collect();
        let mut gone: Vec<u32> = Vec::new();
        for id in falling.iter().copied() {
            let team = self.team_of(id);
            let team_out = self.alive().into_iter().filter(|p| self.team_of(*p) == team).all(|p| falling.contains(&p) || !self.player(p).alive);
            let placement = if team_out {
                if !gone.contains(&team) {
                    gone.push(team);
                }
                teams_before - gone.iter().position(|t| *t == team).unwrap() as u32
            } else {
                teams_before
            };
            self.eliminate(id, placement);
        }
        let alive = self.alive();
        if self.alive_teams().len() <= 1 {
            for last in &alive {
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

    /// Hands out what a player's augments give every round.
    fn augment_grants(&mut self, id: PlayerId) {
        let content = self.content.clone();
        let augments = self.player(id).augments.clone();
        for augment in augments.iter().filter_map(|a| content.augment(a)) {
            let grant = &augment.per_round;
            self.add_gold(id, grant.gold);
            self.add_xp(id, grant.xp);
            self.heal_player(id, grant.health);
            for (currency, amount) in &grant.currencies {
                self.add_currency(id, currency, *amount);
            }
            self.players[id.0 as usize].free_rerolls += grant.free_rerolls;
            for _ in 0..grant.components {
                self.give_random_component(id);
            }
        }
    }

    /// Moves bench units onto free board places, up to the board limit:
    /// most stars and highest cost first, into the front rows from the
    /// middle out.
    fn fill_board(&mut self, id: PlayerId) {
        let limit = self.board_limit(id);
        let rules = self.content.rules().clone();
        let (cols, rows) = (rules.board_cols as i32, rules.board_rows as i32);
        let mut places: Vec<(i32, i32)> = (rows / 2..rows).flat_map(|r| (0..cols).map(move |c| (c, r))).collect();
        places.sort_by_key(|(c, r)| (*r, (c - cols / 2).abs(), *c));
        loop {
            let p = self.player(id);
            let room = limit.saturating_sub(self.board_slots_used(id));
            if room == 0 {
                break;
            }
            let best = p
                .bench
                .iter()
                .flatten()
                .filter(|u| self.slots_of(u) <= room)
                .max_by_key(|u| (u.star, self.content.unit(&u.unit).map(|d| d.cost).unwrap_or(0), std::cmp::Reverse(u.uid)))
                .map(|u| u.uid);
            let place = places.iter().find(|pos| !p.board.contains_key(pos)).copied();
            let (Some(uid), Some((c, r))) = (best, place) else { break };
            if self.move_unit(id, uid, Slot::Board(c, r)).is_err() {
                break;
            }
        }
    }

    fn eliminate(&mut self, id: PlayerId, placement: u32) {
        let p = &mut self.players[id.0 as usize];
        p.alive = false;
        p.placement = Some(placement);
        p.choices.clear();
        let units: Vec<OwnedUnit> =
            std::mem::take(&mut p.board).into_values().chain(p.bench.iter_mut().filter_map(Option::take)).collect();
        for unit in units {
            self.return_to_pool(&unit);
        }
        self.return_shop_to_pool(id);
        self.emit(GameEvent::PlayerEliminated { player: id, placement });
    }

    // ----- commands -------------------------------------------------------

    /// Applies a player's command, or rejects it without changing anything.
    pub fn apply(&mut self, id: PlayerId, command: Command) -> Result<(), CommandError> {
        let result = self.apply_inner(id, command.clone());
        if result.is_ok() {
            self.inputs.push(Input::Command { player: id, command });
        }
        self.run_hooks();
        result
    }

    fn apply_inner(&mut self, id: PlayerId, command: Command) -> Result<(), CommandError> {
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
        if player.forfeited {
            return Err(CommandError::Forfeited);
        }
        match command {
            Command::BuyUnit { slot } => self.buy_unit(id, slot),
            Command::SellUnit { uid } => self.sell_unit(id, uid),
            Command::Action { id: action, arg } => self.use_action(id, &action, arg),
            Command::Reroll => {
                if self.player(id).free_rerolls > 0 {
                    self.players[id.0 as usize].free_rerolls -= 1;
                } else {
                    self.spend(id, self.rule(id, Rule::RerollCost))?;
                }
                self.players[id.0 as usize].stats.rerolls += 1;
                self.roll_shop(id);
                self.fire(id, When::Reroll);
                Ok(())
            }
            Command::BuyXp => {
                let rules = self.content.rules();
                if self.player(id).level as usize >= rules.levels.len() {
                    return Err(CommandError::MaxLevel);
                }
                let (cost, xp) = (self.rule(id, Rule::XpCost), self.rule(id, Rule::XpPerPurchase) as u32);
                self.spend(id, cost)?;
                self.players[id.0 as usize].stats.xp_bought += xp;
                self.add_xp(id, xp);
                self.fire(id, When::BuyXp);
                Ok(())
            }
            Command::SetShopLock { locked } => {
                self.players[id.0 as usize].shop_locked = locked;
                Ok(())
            }
            Command::MoveUnit { uid, to } => self.move_unit(id, uid, to),
            Command::EquipItem { item, uid } => self.equip_item(id, item, uid),
            Command::Pick { option } if !self.player(id).choices.is_empty() && !self.round_awaiting().contains(&id) => {
                if option >= self.player(id).choices[0].options.len() {
                    return Err(CommandError::InvalidOption);
                }
                self.pick_own(id, option);
                Ok(())
            }
            Command::Pick { option } => {
                let mut round = self.current.take().expect("a round is in progress");
                let result = round.pick(self, id, option);
                self.current = Some(round);
                result
            }
            Command::RerollPick => {
                let mut round = self.current.take().expect("a round is in progress");
                let result = round.reroll(self, id);
                self.current = Some(round);
                result
            }
            Command::Gift { uid, to } => {
                let ok = self.players.get(to.0 as usize).is_some_and(|p| p.alive) && self.teammates(id, to);
                if !ok {
                    return Err(CommandError::NotATeammate);
                }
                let free = self.player(to).bench.iter().position(Option::is_none).ok_or(CommandError::BenchFull)?;
                let p = &mut self.players[id.0 as usize];
                let slot = p.locate(uid).ok_or(CommandError::UnknownUnit)?;
                let unit = p.take(slot).expect("located");
                let name = unit.unit.clone();
                self.players[to.0 as usize].bench[free] = Some(unit);
                self.emit(GameEvent::UnitGifted { from: id, to, uid, unit: name.clone() });
                self.check_upgrades(to, &name);
                Ok(())
            }
            Command::Ready => {
                self.players[id.0 as usize].ready = true;
                self.emit(GameEvent::PlayerReady { player: id });
                Ok(())
            }
            Command::Forfeit => {
                // Make any pick that is waiting on them, so nobody else waits.
                for _ in 0..64 {
                    if !self.awaiting().contains(&id) {
                        break;
                    }
                    let open = self.options(id).iter().position(Option::is_some);
                    let Some(option) = open else { break };
                    if self.apply_inner(id, Command::Pick { option }).is_err() {
                        break;
                    }
                }
                let p = &mut self.players[id.0 as usize];
                p.forfeited = true;
                p.ready = true;
                self.emit(GameEvent::PlayerForfeited { player: id });
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
        self.players[id.0 as usize].stats.gold_spent += cost;
        Ok(())
    }

    /// Carries out one of the game's own actions: checks the limit, the
    /// target and the price, lets the handler refuse, then charges, hands
    /// over what the action gives and reports it.
    fn use_action(&mut self, id: PlayerId, action: &str, arg: i64) -> Result<(), CommandError> {
        let content = self.content.clone();
        let def = content.action(action).ok_or(CommandError::UnknownAction)?;
        self.player(id).can_use(def)?;
        if def.target == ActionTarget::Unit && !self.player(id).units().any(|u| i64::from(u.uid) == arg) {
            return Err(CommandError::UnknownUnit);
        }
        if let Some(handler) = content.action_handler(action) {
            handler(self, id, arg)?;
        }
        for price in &def.prices {
            if price.currency == "gold" {
                self.add_gold(id, -price.amount);
                self.players[id.0 as usize].stats.gold_spent += price.amount;
            } else {
                self.add_currency(id, &price.currency, -price.amount);
            }
        }
        for grant in &def.gives {
            self.give(id, grant)?;
        }
        *self.players[id.0 as usize].actions_used.entry(action.to_string()).or_insert(0) += 1;
        self.emit(GameEvent::ActionUsed { player: id, action: action.to_string(), arg });
        Ok(())
    }

    fn buy_unit(&mut self, id: PlayerId, slot: usize) -> Result<(), CommandError> {
        let content = self.content.clone();
        let p = self.player(id);
        let unit_id = p.shop.get(slot).cloned().flatten().ok_or(CommandError::EmptyShopSlot)?;
        let special = p.shop_specials.get(slot).cloned().flatten();
        let cost = self.shop_price(id, slot).unwrap_or(0);
        if p.gold < cost {
            return Err(CommandError::NotEnoughGold { need: cost, have: p.gold });
        }
        // The copy was taken out of the pool when the shop rolled it, so it is
        // always there to buy.
        // A full bench is fine if this copy completes an upgrade.
        let completes_upgrade = p.units().filter(|u| u.unit == unit_id && u.star == 1).count() as u32 + 1
            >= content.rules().copies_to_upgrade
            && content.rules().star_levels.len() > 1;
        if p.bench.iter().all(Option::is_some) && !completes_upgrade {
            return Err(CommandError::BenchFull);
        }
        self.players[id.0 as usize].shop[slot] = None;
        if let Some(s) = self.players[id.0 as usize].shop_specials.get_mut(slot) {
            *s = None;
        }
        self.spend(id, cost)?;
        self.players[id.0 as usize].stats.units_bought += 1;
        match special {
            Some(offer) => self.add_special_unit(id, &unit_id, &offer),
            None => self.add_unit(id, &unit_id, Vec::new()),
        }
        Ok(())
    }

    fn sell_unit(&mut self, id: PlayerId, uid: u32) -> Result<(), CommandError> {
        let p = &mut self.players[id.0 as usize];
        let slot = p.locate(uid).ok_or(CommandError::UnknownUnit)?;
        let unit = p.take(slot).unwrap();
        p.items.extend(unit.items.iter().cloned());
        let value = self.unit_value(&unit);
        self.return_to_pool(&unit);
        self.emit(GameEvent::UnitSold { player: id, uid, unit: unit.unit.clone() });
        self.players[id.0 as usize].stats.units_sold += 1;
        let refund = self.content.rules().sell_refund_permille as i64;
        self.add_gold(id, value * refund / 1000);
        self.fire(id, When::UnitSold);
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
        let p = &self.players[id.0 as usize];
        let from = p.locate(uid).ok_or(CommandError::UnknownUnit)?;
        if from == to {
            return Ok(());
        }
        let occupied = match to {
            Slot::Bench(i) => p.bench[i].is_some(),
            Slot::Board(c, r) => p.board.contains_key(&(c, r)),
        };
        if let (Slot::Bench(_), Slot::Board(c, r)) = (from, to) {
            // Coming onto the board, perhaps in exchange for the unit there.
            let slots = |u: &OwnedUnit| self.content.unit(&u.unit).map_or(1, |d| d.team_slots as usize);
            let used: usize = p.board.values().map(slots).sum();
            let coming = p.unit(uid).map_or(1, slots);
            let leaving = if occupied { p.board.get(&(c, r)).map_or(0, slots) } else { 0 };
            if coming > leaving && used + coming - leaving > limit {
                return Err(CommandError::BoardFull);
            }
        }
        let p = &mut self.players[id.0 as usize];
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
        if let Some(consumable) = content.item(&item_id).and_then(|i| i.consumable.clone()) {
            return self.use_item(id, item, uid, &consumable);
        }
        let unique = content.item(&item_id).is_some_and(|i| i.unique);
        let unit = p.unit_mut(uid).ok_or(CommandError::UnknownUnit)?;
        if unique && unit.items.contains(&item_id) {
            return Err(CommandError::CannotUse("a unit can hold only one of these".into()));
        }
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

    /// Uses up a single-use item on a unit, or refuses without changing
    /// anything.
    fn use_item(&mut self, id: PlayerId, item: usize, uid: u32, consumable: &crate::content::Consumable) -> Result<(), CommandError> {
        use crate::content::Consumable;
        let content = self.content.clone();
        let unit = self.player(id).unit(uid).cloned().ok_or(CommandError::UnknownUnit)?;
        let cannot = |why: &str| Err(CommandError::CannotUse(why.to_string()));
        match consumable {
            Consumable::RemoveItems => {
                if unit.items.is_empty() {
                    return cannot("that unit holds no items");
                }
                let p = &mut self.players[id.0 as usize];
                let taken = std::mem::take(&mut p.unit_mut(uid).expect("unit exists").items);
                p.items.extend(taken);
            }
            Consumable::Duplicate => {
                let p = self.player(id);
                let completes = p.units().filter(|u| u.unit == unit.unit && u.star == 1).count() as u32 + 1
                    >= content.rules().copies_to_upgrade;
                if p.bench.iter().all(Option::is_some) && !completes {
                    return Err(CommandError::BenchFull);
                }
                if !self.take_from_pool(&unit.unit) {
                    return cannot("no copies of that unit are left");
                }
                self.add_unit(id, &unit.unit, Vec::new());
            }
            Consumable::Reforge => {
                if unit.items.is_empty() {
                    return cannot("that unit holds no items");
                }
                let kind = |i: &crate::content::ItemDef| (i.recipe.is_some(), i.consumable.is_none());
                let mut reforged = Vec::new();
                for held in &unit.items {
                    let Some(def) = content.item(held) else {
                        reforged.push(held.clone());
                        continue;
                    };
                    let others: Vec<String> = content
                        .items()
                        .filter(|i| i.id != *held && i.consumable.is_none() && kind(i) == kind(def))
                        .map(|i| i.id.clone())
                        .collect();
                    if others.is_empty() {
                        reforged.push(held.clone());
                    } else {
                        let pick = self.rng.below(others.len() as u64) as usize;
                        reforged.push(others[pick].clone());
                    }
                }
                self.players[id.0 as usize].unit_mut(uid).expect("unit exists").items = reforged;
            }
            Consumable::Gold(n) => self.add_gold(id, *n),
            Consumable::Give(grant) => self.give(id, grant)?,
            Consumable::Upgrade => {
                let better = unit.items.iter().enumerate().find_map(|(i, held)| {
                    let to = content.item(held)?.upgrades_to.clone()?;
                    content.item(&to).map(|_| (i, to))
                });
                let Some((slot, to)) = better else {
                    return cannot("that unit holds nothing that can be made better");
                };
                self.players[id.0 as usize].unit_mut(uid).expect("unit exists").items[slot] = to.clone();
                self.emit(GameEvent::ItemCombined { player: id, uid, item: to });
            }
        }
        let item_id = self.players[id.0 as usize].items.remove(item);
        self.emit(GameEvent::ItemUsed { player: id, uid, item: item_id });
        if matches!(consumable, Consumable::Reforge) {
            self.combine_items_on(id, uid);
        }
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

    /// Hands a player something from outside the rules, and records it so
    /// the game can still be replayed. Ids that the content does not define
    /// are rejected.
    pub fn grant(&mut self, id: PlayerId, grant: Grant) -> Result<(), CommandError> {
        let p = self.players.get(id.0 as usize).ok_or(CommandError::UnknownPlayer)?;
        if !p.alive {
            return Err(CommandError::Eliminated);
        }
        self.give(id, &grant)?;
        self.inputs.push(Input::Grant { player: id, grant });
        self.run_hooks();
        Ok(())
    }

    /// Hands a player what a grant describes, without recording it. This
    /// is for round types, hooks and action handlers, which run again when
    /// a game is replayed; from outside the rules use [`Game::grant`].
    pub fn give(&mut self, id: PlayerId, grant: &Grant) -> Result<(), CommandError> {
        let content = self.content.clone();
        match grant {
            Grant::Gold(n) => self.add_gold(id, *n),
            Grant::Xp(n) => self.add_xp(id, *n),
            Grant::Unit(unit) if content.unit(unit).is_some() => self.add_unit(id, unit, Vec::new()),
            Grant::Item(item) if content.item(item).is_some() => self.give_item(id, item),
            Grant::Augment(a) if content.augment(a).is_some() => self.give_augment(id, a),
            Grant::Damage(n) => self.damage_player(id, (*n).max(0)),
            Grant::Heal(n) => self.heal_player(id, *n),
            Grant::Currency { currency, amount } if currency == "gold" || content.currency(currency).is_some() => {
                self.add_currency(id, currency, *amount);
            }
            Grant::Component => self.give_random_component(id),
            Grant::ItemTagged(tag) => {
                let tagged: Vec<String> = content.items().filter(|i| i.tags.contains(tag)).map(|i| i.id.clone()).collect();
                if !tagged.is_empty() {
                    let pick = self.rng.below(tagged.len() as u64) as usize;
                    self.give_item(id, &tagged[pick]);
                }
            }
            Grant::UnitOfCost(cost) => {
                let available: Vec<(String, u32)> =
                    self.available_units(None).into_iter().filter(|(u, _)| content.unit(u).is_some_and(|d| d.cost == *cost)).collect();
                let weights: Vec<u64> = available.iter().map(|(_, n)| *n as u64).collect();
                if let Some(i) = self.rng.weighted_index(&weights) {
                    let unit = available[i].0.clone();
                    self.take_from_pool(&unit);
                    self.add_unit(id, &unit, Vec::new());
                }
            }
            Grant::Rerolls(n) => self.players[id.0 as usize].free_rerolls += n,
            Grant::Loot(table) => {
                let Some(table) = content.loot(table) else { return Err(CommandError::UnknownItem) };
                if self.perk_depth >= MAX_GRANT_DEPTH {
                    return Ok(());
                }
                let weights: Vec<u64> = table.entries.iter().map(|e| e.weight as u64).collect();
                if let Some(i) = self.rng.weighted_index(&weights) {
                    self.perk_depth += 1;
                    for inner in &table.entries[i].gives {
                        let _ = self.give(id, inner);
                    }
                    self.perk_depth -= 1;
                }
            }
            Grant::Choice { options, offer } => self.offer_choice(id, *options, offer),
            Grant::Currency { .. } => return Err(CommandError::UnknownCurrency),
            Grant::Unit(_) => return Err(CommandError::UnknownUnit),
            Grant::Item(_) | Grant::Augment(_) => return Err(CommandError::UnknownItem),
        }
        Ok(())
    }

    /// The augment every player got at the start (see
    /// `GameRules::lobby_modifiers`), if any.
    pub fn lobby_modifier(&self) -> Option<&str> {
        self.lobby_modifier.as_deref()
    }

    /// The seed the game was created with.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Every input so far, in order.
    pub fn inputs(&self) -> &[Input] {
        &self.inputs
    }

    /// A fingerprint of everything the players can see and the pool. Two
    /// games with the same checksum are in the same state, so it is used to
    /// check a replayed game against the original.
    pub fn checksum(&self) -> u64 {
        let state = serde_json::to_vec(&(&self.players, &self.pool, self.round_index, self.phase == Phase::Over))
            .expect("game state serializes");
        crate::rng::fnv1a(&state)
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
        if amount > 0 {
            p.stats.gold_earned += amount;
        }
        p.gold = (p.gold + amount).max(0);
        let gold = p.gold;
        self.emit(GameEvent::GoldChanged { player: id, gold });
    }

    /// How much a player has of a currency. `"gold"` is their gold.
    pub fn currency(&self, id: PlayerId, currency: &str) -> i64 {
        self.player(id).balance(currency)
    }

    /// Gives a player some of a currency, or takes it away when negative
    /// (never below zero). `"gold"` is the same as [`Game::add_gold`].
    /// Currencies the content does not define are ignored.
    pub fn add_currency(&mut self, id: PlayerId, currency: &str, amount: i64) {
        if currency == "gold" {
            return self.add_gold(id, amount);
        }
        if amount == 0 || self.content.currency(currency).is_none() {
            return;
        }
        let balance = self.players[id.0 as usize].wallet.entry(currency.to_string()).or_insert(0);
        *balance = (*balance + amount).max(0);
        let amount = *balance;
        self.emit(GameEvent::CurrencyChanged { player: id, currency: currency.to_string(), amount });
    }

    /// Takes `cost` of a currency from a player, or fails and takes nothing
    /// if they have less. This is how a round type or a hook charges for
    /// something. `"gold"` spends gold.
    pub fn spend_currency(&mut self, id: PlayerId, currency: &str, cost: i64) -> Result<(), CommandError> {
        if currency == "gold" {
            return self.spend(id, cost.max(0));
        }
        let have = self.currency(id, currency);
        if self.content.currency(currency).is_none() {
            return Err(CommandError::UnknownCurrency);
        }
        if have < cost {
            return Err(CommandError::NotEnough { currency: currency.to_string(), need: cost, have });
        }
        self.add_currency(id, currency, -cost.max(0));
        Ok(())
    }

    pub fn add_xp(&mut self, id: PlayerId, amount: u32) {
        let levels = &self.content.rules().levels;
        let p = &mut self.players[id.0 as usize];
        if p.level as usize >= levels.len() || amount == 0 {
            return;
        }
        let before = p.level;
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
        for _ in before..level {
            self.fire(id, When::LevelUp);
        }
    }

    /// Adds an item to a player's inventory.
    pub fn give_item(&mut self, id: PlayerId, item: &str) {
        self.players[id.0 as usize].items.push(item.to_string());
        self.emit(GameEvent::ItemGained { player: id, item: item.to_string() });
    }

    /// Ids of items that are not made from other items and are not used up,
    /// ordered by id.
    pub fn component_items(&self) -> Vec<String> {
        self.content.items().filter(|i| i.recipe.is_none() && i.consumable.is_none()).map(|i| i.id.clone()).collect()
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
        let gained: Vec<Perk> =
            self.content.augment(augment).map(|a| a.perks.iter().filter(|p| p.when == When::Gained).cloned().collect()).unwrap_or_default();
        self.run_perks(id, &gained);
    }

    /// Sets off a player's [perks](crate::perk) for an occasion.
    pub fn fire(&mut self, id: PlayerId, when: When) {
        if when == When::Gained || !self.player(id).alive {
            return;
        }
        let perks: Vec<Perk> = self.perks_and_rules(id).0.into_iter().filter(|p| p.when == when).collect();
        self.run_perks(id, &perks);
    }

    fn run_perks(&mut self, id: PlayerId, perks: &[Perk]) {
        if self.perk_depth >= MAX_GRANT_DEPTH {
            return;
        }
        self.perk_depth += 1;
        for perk in perks {
            if perk.chance_permille >= 1000 || self.rng.chance(perk.chance_permille as u64, 1000) {
                for grant in &perk.gives {
                    // Checked when the content was built, so this cannot fail.
                    let _ = self.give(id, grant);
                }
            }
        }
        self.perk_depth -= 1;
    }

    /// Everything a [choice](ChoiceOffer) could offer a player, ordered by
    /// id. `AugmentTiers` lists every tier it names.
    pub fn choice_candidates(&self, id: PlayerId, offer: &ChoiceOffer) -> Vec<String> {
        let content = &self.content;
        let owned = &self.player(id).augments;
        let augments = |keep: &dyn Fn(u32) -> bool| -> Vec<String> {
            content.augments().filter(|a| keep(a.tier) && !owned.contains(&a.id)).map(|a| a.id.clone()).collect()
        };
        match offer {
            ChoiceOffer::Augments => augments(&|_| true),
            ChoiceOffer::AugmentsOfTier(tier) => augments(&|t| t == *tier),
            ChoiceOffer::AugmentTiers(tiers) => augments(&|t| tiers.iter().any(|(tier, _)| *tier == t)),
            ChoiceOffer::Items => content.items().map(|i| i.id.clone()).collect(),
            ChoiceOffer::ItemsTagged(tag) => content.items().filter(|i| i.tags.contains(tag)).map(|i| i.id.clone()).collect(),
            ChoiceOffer::Components => self.component_items(),
            ChoiceOffer::Units { cost } => self
                .available_units(None)
                .into_iter()
                .map(|(unit, _)| unit)
                .filter(|u| content.unit(u).is_some_and(|d| d.cost == *cost))
                .collect(),
        }
    }

    /// Settles which tier an [`ChoiceOffer::AugmentTiers`] offers, by its
    /// weights. Other offers come back unchanged.
    pub fn settle_offer(&mut self, offer: &ChoiceOffer) -> ChoiceOffer {
        match offer {
            ChoiceOffer::AugmentTiers(tiers) => {
                let weights: Vec<u64> = tiers.iter().map(|(_, w)| *w as u64).collect();
                match self.rng.weighted_index(&weights) {
                    Some(i) => ChoiceOffer::AugmentsOfTier(tiers[i].0),
                    None => ChoiceOffer::Augments,
                }
            }
            other => other.clone(),
        }
    }

    /// Hands a player what they picked from a [choice](ChoiceOffer).
    pub fn give_choice(&mut self, id: PlayerId, offer: &ChoiceOffer, choice: &str) {
        match offer {
            ChoiceOffer::Augments | ChoiceOffer::AugmentsOfTier(_) | ChoiceOffer::AugmentTiers(_) => self.give_augment(id, choice),
            ChoiceOffer::Items | ChoiceOffer::ItemsTagged(_) | ChoiceOffer::Components => self.give_item(id, choice),
            ChoiceOffer::Units { .. } => {
                self.take_from_pool(choice);
                self.add_unit(id, choice, Vec::new());
            }
        }
    }

    /// Opens a pick of one out of `options` for a player (see
    /// [`Grant::Choice`]). Does nothing when there is nothing to offer.
    pub fn offer_choice(&mut self, id: PlayerId, options: u32, offer: &ChoiceOffer) {
        let offer = self.settle_offer(offer);
        let mut candidates = self.choice_candidates(id, &offer);
        self.rng.shuffle(&mut candidates);
        candidates.truncate(options as usize);
        if candidates.is_empty() {
            return;
        }
        self.emit(GameEvent::ChoiceOffered { player: Some(id), options: candidates.clone() });
        self.players[id.0 as usize].choices.push(PendingChoice { offer, options: candidates });
    }

    /// Answers the oldest of a player's own picks.
    fn pick_own(&mut self, id: PlayerId, option: usize) {
        let p = &mut self.players[id.0 as usize];
        if p.choices.is_empty() {
            return;
        }
        let pending = p.choices.remove(0);
        let Some(choice) = pending.options.get(option).cloned() else { return };
        self.emit(GameEvent::ChoiceMade { player: id, option, choice: choice.clone() });
        self.give_choice(id, &pending.offer, &choice);
    }

    /// Adds to the stat changes one of a player's units keeps for good.
    /// Does nothing if they have no unit with that `uid`.
    pub fn grow_unit(&mut self, id: PlayerId, uid: u32, mods: &[StatMod]) {
        if let Some(unit) = self.players[id.0 as usize].unit_mut(uid) {
            unit.grow(mods);
        }
    }

    /// Makes one of a player's units count as having another trait.
    pub fn add_unit_trait(&mut self, id: PlayerId, uid: u32, trait_id: &str) {
        if let Some(unit) = self.players[id.0 as usize].unit_mut(uid) {
            if !unit.traits.iter().any(|t| t == trait_id) {
                unit.traits.push(trait_id.to_string());
            }
        }
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
        let unit = OwnedUnit { uid, unit: unit_id.to_string(), star: 1, items, growth: Vec::new(), traits: Vec::new() };
        let p = &mut self.players[id.0 as usize];
        p.stats.best_star = p.stats.best_star.max(1);
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

    /// Adds a unit bought from a special offer: at its star level and with
    /// its extra trait.
    fn add_special_unit(&mut self, id: PlayerId, unit_id: &str, offer: &ShopOffer) {
        self.add_unit(id, unit_id, Vec::new());
        // The newest copy is the one with the highest uid.
        let uid = self.next_uid - 1;
        let star = offer.star.max(1);
        if let Some(unit) = self.players[id.0 as usize].unit_mut(uid) {
            unit.star = unit.star.max(star);
            if let Some(t) = &offer.extra_trait {
                unit.traits.push(t.clone());
            }
            let star = unit.star;
            let stats = &mut self.players[id.0 as usize].stats;
            stats.best_star = stats.best_star.max(star);
            if star > 1 {
                self.emit(GameEvent::UnitUpgraded { player: id, uid, unit: unit_id.to_string(), star });
            }
        }
        self.check_upgrades(id, unit_id);
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
            let (mut growth, mut traits) = (Vec::new(), Vec::new());
            for uid in &matching[1..] {
                let slot = p.locate(*uid).unwrap();
                let merged = p.take(slot).unwrap();
                items.extend(merged.items);
                growth.extend(merged.growth);
                traits.extend(merged.traits);
            }
            let mut overflow = Vec::new();
            let kept = p.unit_mut(keep).unwrap();
            kept.star = star + 1;
            // What the merged copies had earned is not lost.
            kept.grow(&growth);
            for t in traits {
                if !kept.traits.contains(&t) {
                    kept.traits.push(t);
                }
            }
            for item in items {
                if kept.items.len() < max_items {
                    kept.items.push(item);
                } else {
                    overflow.push(item);
                }
            }
            p.items.extend(overflow);
            self.emit(GameEvent::UnitUpgraded { player: id, uid: keep, unit: unit_id.to_string(), star: star + 1 });
            let stats = &mut self.players[id.0 as usize].stats;
            stats.best_star = stats.best_star.max(star + 1);
            self.combine_items_on(id, keep);
            self.fire(id, When::StarUp);
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

    /// Puts the copies a player's shop is holding back into the pool and
    /// empties the shop.
    fn return_shop_to_pool(&mut self, id: PlayerId) {
        let p = &mut self.players[id.0 as usize];
        let specials = std::mem::take(&mut p.shop_specials);
        let offers: Vec<(usize, String)> = p.shop.iter_mut().enumerate().filter_map(|(i, o)| Some((i, o.take()?))).collect();
        for (slot, unit) in offers {
            let star = specials.get(slot).cloned().flatten().map_or(1, |s| s.star.max(1));
            for _ in 0..self.content.rules().copies_to_upgrade.pow(star as u32 - 1) {
                self.put_in_pool(&unit);
            }
        }
    }

    /// Fills a player's shop. Each offer takes its copy out of the shared
    /// pool, so a shop never shows a unit that cannot be bought, and the same
    /// last copy is never offered twice; the previous offers go back first.
    fn roll_shop(&mut self, id: PlayerId) {
        self.return_shop_to_pool(id);
        let content = self.content.clone();
        let rules = content.rules();
        let level = (self.player(id).level as i64 + self.rule(id, Rule::ShopLevel)).clamp(1, rules.levels.len() as i64) as usize;
        let odds = &rules.levels[level - 1].shop_odds;
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
            if let Some(unit) = &offer {
                self.take_from_pool(unit);
            }
            offers.push(offer);
        }
        // Some offers come with something extra.
        let mut specials = vec![None; offers.len()];
        let player_level = self.player(id).level;
        for (slot, offer) in offers.iter().enumerate() {
            let Some(unit) = offer else { continue };
            for special in rules.shop_specials.iter().filter(|s| player_level >= s.min_level) {
                if !self.rng.chance(special.chance_permille as u64, 1000) {
                    continue;
                }
                if let Some(made) = self.make_special(unit, special) {
                    specials[slot] = Some(made);
                    break;
                }
            }
        }
        let p = &mut self.players[id.0 as usize];
        p.shop = offers.clone();
        p.shop_specials = specials;
        self.emit(GameEvent::ShopRolled { player: id, offers });
    }

    /// Turns an offer into a special one, taking the extra copies a higher
    /// star needs out of the pool. `None` if the pool is short or the unit
    /// already has every trait the special could give.
    fn make_special(&mut self, unit: &str, special: &ShopSpecial) -> Option<ShopOffer> {
        let content = self.content.clone();
        let def = content.unit(unit)?;
        let extra_trait = if special.traits.is_empty() {
            None
        } else {
            let options: Vec<&String> = special.traits.iter().filter(|t| !def.traits.contains(t)).collect();
            if options.is_empty() {
                return None;
            }
            Some(options[self.rng.below(options.len() as u64) as usize].clone())
        };
        let star = special.star.max(1);
        let extra = content.rules().copies_to_upgrade.pow(star as u32 - 1) - 1;
        if self.pool_count(unit) < extra {
            return None;
        }
        for _ in 0..extra {
            self.take_from_pool(unit);
        }
        Some(ShopOffer { star, extra_trait })
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
        // Every fight of a round starts with the round's tiles.
        let tiles = self.round_tiles().to_vec();
        let mut combat = self.content.setup_combat_with(seed, &teams, &tiles).expect("boards are validated when they are built");
        let outcome = combat.run().clone();
        let (rewards, growth) = (combat.rewards().to_vec(), combat.growth().to_vec());
        let survivors = match outcome.winner {
            Some(team) => combat.units().iter().filter(|u| u.is_alive() && u.team() == team).count() as u32,
            None => 0,
        };
        if let Opponent::Player(other) = away {
            let keep = self.content.rules().avoid_rematch_rounds as usize;
            for (a, b) in [(home, other), (other, home)] {
                let list = self.opponents.entry(a).or_default();
                list.push(b);
                let extra = list.len().saturating_sub(keep);
                list.drain(..extra);
            }
        }
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
            tiles,
            events: combat.drain_events(),
        });
        // What units earned in the fight goes to the players who own them:
        // not to a neutral board, and not to the copy of a board.
        let owners = [Some(home), match away {
            Opponent::Player(p) => Some(p),
            _ => None,
        }];
        let record = self.combats.last().expect("just recorded");
        let sizes = [record.teams[0].units.len(), record.teams[1].units.len()];
        let places: Vec<(PlayerId, (i32, i32), Vec<StatMod>)> = growth
            .into_iter()
            .filter_map(|(unit, mods)| {
                // Units stand in the order the boards listed them.
                let index = unit.0 as usize;
                let (team, at) = if index < sizes[0] { (0, index) } else { (1, index - sizes[0]) };
                let pos = record.teams[team].units.get(at)?.pos;
                Some((owners[team]?, pos, mods))
            })
            .collect();
        for (player, pos, mods) in places {
            if let Some(unit) = self.players[player.0 as usize].board.get_mut(&pos) {
                unit.grow(&mods);
            }
        }
        for (team, grant) in rewards {
            if let Some(player) = owners.get(team.0 as usize).copied().flatten() {
                // Checked when the content was built, so this cannot fail.
                let _ = self.give(player, &grant);
            }
        }
        // Results and kills, for both sides when both are players.
        let sides = [Some(home), match away {
            Opponent::Player(p) => Some(p),
            _ => None,
        }];
        for (team, player) in sides.into_iter().enumerate() {
            let Some(player) = player else { continue };
            let kills = combat.units().iter().filter(|u| !u.is_alive() && u.team() != Team(team as u8)).count() as u32;
            let stats = &mut self.players[player.0 as usize].stats;
            stats.kills += kills;
            let won = outcome.winner == Some(Team(team as u8));
            match outcome.winner {
                Some(_) if won => stats.fights_won += 1,
                Some(_) => stats.fights_lost += 1,
                None => stats.fights_drawn += 1,
            }
            self.fire(player, if won { When::Won } else { When::Lost });
            self.fire(player, When::Fought);
        }
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
        // A player's rules can add to what they take, or take some off.
        let amount = if amount > 0 { (amount + self.rule(id, Rule::DamageTaken)).max(0) } else { amount };
        // Teammates share one health total.
        for member in self.team_members(id) {
            let p = &mut self.players[member.0 as usize];
            p.health -= amount;
            p.stats.damage_taken += amount;
            let health = p.health;
            self.emit(GameEvent::PlayerDamaged { player: member, amount, health });
        }
    }

    /// Gives a player health back, up to what players start with
    /// (`GameRules::starting_health`). Does nothing for a player who is
    /// already there.
    pub fn heal_player(&mut self, id: PlayerId, amount: i64) {
        let most = self.content.rules().starting_health;
        let amount = amount.min(most - self.player(id).health);
        if amount <= 0 {
            return;
        }
        for member in self.team_members(id) {
            let p = &mut self.players[member.0 as usize];
            p.health += amount;
            let health = p.health;
            self.emit(GameEvent::PlayerHealed { player: member, amount, health });
        }
    }

    /// A player and their teammates still in the game, in seat order. Just
    /// the player when teams are of one.
    pub fn team_members(&self, id: PlayerId) -> Vec<PlayerId> {
        let team = self.team_of(id);
        let mut members: Vec<PlayerId> = self.players.iter().filter(|p| (p.alive || p.id == id) && self.team_of(p.id) == team).map(|p| p.id).collect();
        members.sort();
        members
    }

    /// Takes a unit away from a player without paying for it: a sacrifice,
    /// a trade, a unit that leaves. Its items go back to the player's
    /// inventory and its copies back to the pool. Returns the unit, or
    /// `None` if the player has no unit with that `uid`.
    pub fn remove_unit(&mut self, id: PlayerId, uid: u32) -> Option<OwnedUnit> {
        let p = &mut self.players[id.0 as usize];
        let slot = p.locate(uid)?;
        let unit = p.take(slot)?;
        p.items.extend(unit.items.iter().cloned());
        self.return_to_pool(&unit);
        self.emit(GameEvent::UnitRemoved { player: id, uid, unit: unit.unit.clone() });
        Some(unit)
    }

    /// What a unit is worth: its price times the copies merged into it.
    /// Selling it pays this times `GameRules::sell_refund_permille`.
    pub fn unit_value(&self, unit: &OwnedUnit) -> i64 {
        let cost = self.content.unit(&unit.unit).map(|u| u.cost as i64).unwrap_or(0);
        cost * self.copies_in(unit) as i64
    }

    /// Updates a player's streak after a fight against another player and
    /// pays the win bonus.
    pub fn record_result(&mut self, id: PlayerId, won: bool) {
        let p = &mut self.players[id.0 as usize];
        p.streak = if won { p.streak.max(0) + 1 } else { p.streak.min(0) - 1 };
        p.stats.best_streak = p.stats.best_streak.max(p.streak.max(0) as u32);
        if won {
            let bonus = self.rule(id, Rule::WinBonus);
            self.add_gold(id, bonus);
        }
    }
}
