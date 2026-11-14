//! Game rules written as Rhai scripts.
//!
//! Scripts are [Rhai](https://rhai.rs) sources a game adds with
//! [`GameBuilder::script`](crate::content::GameBuilder::script), usually
//! kept in their own `.rhai` files and embedded with `include_str!`. They
//! add rules in two ways:
//!
//! - A script with a `finish(game)` function is a **round type**, named
//!   after the script. `RoundKindDef::custom("payday")` in the schedule runs
//!   the `payday` script: `begin(game)` when the round starts (optional)
//!   and `finish(game)` when it ends.
//! - A script with an `on_event(game, event)` function is called for every
//!   [`GameEvent`], so it can react to anything that happens in the lobby.
//!
//! Scripts see the game through the `game` object (see [`API`] for the full
//! list of functions, or run `autoengine api`). They cannot touch files, the network or the
//! clock, numbers are integers, and randomness comes from the game's seeded
//! generator, so a scripted game is as reproducible as any other. A script
//! that fails or runs too long is stopped and reported as a
//! [`GameEvent::Script`] error; the game carries on.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rhai::{AST, Array, Dynamic, Engine, EvalAltResult, INT, Map, Scope};

use crate::game::{CommandError, Game, GameEvent, GameHook, PlayerId};
use crate::rng::Rng;
use crate::round::{PvpRound, RoundType};

/// A script that does not compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    pub script: String,
    pub message: String,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "script `{}`: {}", self.script, self.message)
    }
}

struct Script {
    name: String,
    ast: AST,
    has_begin: bool,
    has_finish: bool,
    has_on_event: bool,
}

/// The compiled scripts of a content set.
pub struct ScriptHost {
    engine: Engine,
    scripts: Vec<Script>,
}

/// What a script asked the game to do. Requests are collected while the
/// script runs and carried out afterwards, in order.
#[derive(Clone, Debug)]
enum Action {
    Gold(INT, INT),
    Xp(INT, INT),
    Item(INT, String),
    RandomItem(INT),
    Unit(INT, String),
    Augment(INT, String),
    Damage(INT, INT),
    Log(String),
    FightAll,
}

#[derive(Clone, Debug)]
struct PlayerSnap {
    name: String,
    gold: INT,
    health: INT,
    level: INT,
    alive: bool,
    units: INT,
    items: INT,
    streak: INT,
}

struct Ctx {
    players: Vec<PlayerSnap>,
    stage: INT,
    round: INT,
    round_name: String,
    params: BTreeMap<String, i64>,
    rng: Rng,
    actions: Vec<Action>,
    vars: Arc<Mutex<BTreeMap<String, i64>>>,
}

/// The `game` object scripts receive.
#[derive(Clone)]
pub struct ScriptGame(Arc<Mutex<Ctx>>);

impl ScriptGame {
    fn with<T>(&mut self, f: impl FnOnce(&mut Ctx) -> T) -> T {
        f(&mut self.0.lock().expect("a script panicked while holding the game"))
    }

    fn player<T: Default>(&mut self, id: INT, f: impl FnOnce(&mut PlayerSnap) -> T) -> T {
        self.with(|c| usize::try_from(id).ok().and_then(|i| c.players.get_mut(i)).map(f).unwrap_or_default())
    }

    fn push(&mut self, action: Action) {
        self.with(|c| c.actions.push(action));
    }
}

/// Everything scripts can call on `game`, as signature and description.
/// This is what `autoengine api` prints; a test keeps it in step with
/// the functions the engine registers.
pub const API: &[(&str, &str)] = &[
    ("players()", "ids of the players still alive"),
    ("all_players()", "ids of every player, including eliminated ones"),
    ("name(p)", "a player's name"),
    ("gold(p)", "a player's gold"),
    ("health(p)", "a player's health"),
    ("level(p)", "a player's level"),
    ("is_alive(p)", "whether a player is still in the game"),
    ("unit_count(p)", "how many units a player owns, board and bench"),
    ("item_count(p)", "how many unequipped items a player holds"),
    ("streak(p)", "a player's streak: wins are positive, losses negative"),
    ("stage()", "the current stage number"),
    ("round()", "the current round, counting from 0"),
    ("round_name()", "the current round's name from the schedule"),
    ("param(\"name\")", "a number from the round's params in the schedule (`.param(name, value)`), or 0"),
    ("random(n)", "a number from 0 to n-1, from the game's seed"),
    ("get(\"key\")", "a number saved with `set`, or 0"),
    ("set(\"key\", n)", "saves a number for the rest of the game, shared by all scripts"),
    ("give_gold(p, n)", "gives a player gold (negative takes it away)"),
    ("give_xp(p, n)", "gives a player experience"),
    ("give_item(p, \"id\")", "gives a player an item"),
    ("give_random_item(p)", "gives a player a random component item"),
    ("give_unit(p, \"id\")", "puts a unit on a player's bench, if there is room"),
    ("give_augment(p, \"id\")", "gives a player an augment"),
    ("damage(p, n)", "takes health from a player"),
    ("log(\"text\")", "shows a line in every client's log"),
    ("fight_all()", "pairs everyone up to fight, as a Pvp round does"),
];

/// The events `on_event` receives: `event.kind` and the other fields the
/// event carries. A test keeps this in step with [`GameEvent`].
pub const EVENTS: &[(&str, &str)] = &[
    ("RoundStarted", "index, name, kind, stage"),
    ("RoundFinished", "index"),
    ("ShopRolled", "player, offers"),
    ("GoldChanged", "player, gold"),
    ("XpChanged", "player, level, xp"),
    ("UnitGained", "player, uid, unit"),
    ("UnitSold", "player, uid, unit"),
    ("UnitUpgraded", "player, uid, unit, star"),
    ("UnitMoved", "player, uid, to"),
    ("ItemGained", "player, item"),
    ("ItemEquipped", "player, uid, item"),
    ("ItemCombined", "player, uid, item"),
    ("ChoiceOffered", "player, options"),
    ("ChoiceMade", "player, option, choice"),
    ("AugmentGained", "player, augment"),
    ("PlayerReady", "player"),
    ("CombatFinished", "combat, home, away, winner"),
    ("PlayerDamaged", "player, amount, health"),
    ("PlayerEliminated", "player, placement"),
    ("GameOver", "winner"),
    ("Script", "script, message"),
];

/// The scripting reference as text, printed by `autoengine api`.
pub fn api_reference() -> String {
    use std::fmt::Write;
    let mut out = String::new();
    out.push_str(
        "Scripts are .rhai sources added with ContentSet::builder().script(name, source).\n\n\
         A script can define any of these functions:\n  \
         begin(game)            runs when a round of this script's kind starts\n  \
         finish(game)           runs when it ends; makes the script a round kind named after it\n  \
         on_event(game, event)  runs for everything that happens in the game\n\n\
         Functions on `game` (p is a player id):\n",
    );
    for (signature, description) in API {
        let _ = writeln!(out, "  game.{signature:<24} {description}");
    }
    out.push_str("\nEvents passed to on_event, as event.kind and its other fields:\n");
    for (kind, fields) in EVENTS {
        let _ = writeln!(out, "  {kind:<18} {fields}");
    }
    out.push_str("\nNumbers are whole numbers. Players in events are their ids.\n");
    out
}

/// Registers everything scripts can call on `game`: see [`API`].
fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<ScriptGame>("Game");
    engine.register_fn("players", |g: &mut ScriptGame| -> Array {
        g.with(|c| c.players.iter().enumerate().filter(|(_, p)| p.alive).map(|(i, _)| Dynamic::from(i as INT)).collect())
    });
    engine.register_fn("all_players", |g: &mut ScriptGame| -> Array {
        g.with(|c| (0..c.players.len()).map(|i| Dynamic::from(i as INT)).collect())
    });
    engine.register_fn("name", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.name.clone()));
    engine.register_fn("gold", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.gold));
    engine.register_fn("health", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.health));
    engine.register_fn("level", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.level));
    engine.register_fn("is_alive", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.alive));
    engine.register_fn("unit_count", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.units));
    engine.register_fn("item_count", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.items));
    engine.register_fn("streak", |g: &mut ScriptGame, p: INT| g.player(p, |s| s.streak));
    engine.register_fn("stage", |g: &mut ScriptGame| g.with(|c| c.stage));
    engine.register_fn("round", |g: &mut ScriptGame| g.with(|c| c.round));
    engine.register_fn("round_name", |g: &mut ScriptGame| g.with(|c| c.round_name.clone()));
    engine.register_fn("param", |g: &mut ScriptGame, name: &str| g.with(|c| c.params.get(name).copied().unwrap_or(0)));
    engine.register_fn("random", |g: &mut ScriptGame, n: INT| g.with(|c| if n > 0 { c.rng.below(n as u64) as INT } else { 0 }));
    engine.register_fn("get", |g: &mut ScriptGame, key: &str| g.with(|c| c.vars.lock().unwrap().get(key).copied().unwrap_or(0)));
    engine.register_fn("set", |g: &mut ScriptGame, key: &str, value: INT| {
        g.with(|c| c.vars.lock().unwrap().insert(key.to_string(), value));
    });
    // Gold and health are adjusted in the script's view straight away, so a
    // script reads back what it just did.
    engine.register_fn("give_gold", |g: &mut ScriptGame, p: INT, n: INT| {
        g.player(p, |s| s.gold = (s.gold + n).max(0));
        g.push(Action::Gold(p, n));
    });
    engine.register_fn("damage", |g: &mut ScriptGame, p: INT, n: INT| {
        g.player(p, |s| s.health -= n.max(0));
        g.push(Action::Damage(p, n));
    });
    engine.register_fn("give_xp", |g: &mut ScriptGame, p: INT, n: INT| g.push(Action::Xp(p, n)));
    engine.register_fn("give_item", |g: &mut ScriptGame, p: INT, id: &str| g.push(Action::Item(p, id.to_string())));
    engine.register_fn("give_random_item", |g: &mut ScriptGame, p: INT| g.push(Action::RandomItem(p)));
    engine.register_fn("give_unit", |g: &mut ScriptGame, p: INT, id: &str| g.push(Action::Unit(p, id.to_string())));
    engine.register_fn("give_augment", |g: &mut ScriptGame, p: INT, id: &str| g.push(Action::Augment(p, id.to_string())));
    engine.register_fn("log", |g: &mut ScriptGame, text: &str| g.push(Action::Log(text.to_string())));
    engine.register_fn("fight_all", |g: &mut ScriptGame| g.push(Action::FightAll));
}

fn new_engine() -> Engine {
    let mut engine = Engine::new();
    // Keep runaway scripts from hanging the server.
    engine.set_max_operations(2_000_000);
    engine.set_max_call_levels(48);
    // The defaults are much lower in debug builds; use one limit everywhere.
    engine.set_max_expr_depths(64, 64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(10_000);
    engine.on_print(|text| eprintln!("script: {text}"));
    engine.on_debug(|text, _, pos| eprintln!("script {pos}: {text}"));
    register_api(&mut engine);
    engine
}

/// An event as a script sees it: a map with `kind` set to the event's name
/// and the event's fields beside it, with players as their numbers.
fn event_map(event: &GameEvent) -> Map {
    fn convert(value: serde_json::Value) -> Dynamic {
        match value {
            serde_json::Value::Null => Dynamic::UNIT,
            serde_json::Value::Bool(b) => b.into(),
            serde_json::Value::Number(n) => Dynamic::from(n.as_i64().unwrap_or(0) as INT),
            serde_json::Value::String(s) => s.into(),
            serde_json::Value::Array(items) => Dynamic::from_array(items.into_iter().map(convert).collect()),
            serde_json::Value::Object(fields) => {
                Dynamic::from_map(fields.into_iter().map(|(k, v)| (k.as_str().into(), convert(v))).collect())
            }
        }
    }
    let mut map = Map::new();
    match serde_json::to_value(event) {
        Ok(serde_json::Value::Object(outer)) => {
            for (kind, fields) in outer {
                map.insert("kind".into(), kind.into());
                if let serde_json::Value::Object(fields) = fields {
                    for (k, v) in fields {
                        map.insert(k.as_str().into(), convert(v));
                    }
                }
            }
        }
        Ok(serde_json::Value::String(kind)) => {
            map.insert("kind".into(), kind.into());
        }
        _ => {}
    }
    map
}

impl ScriptHost {
    /// Compiles scripts given as name and source.
    pub fn compile(sources: &BTreeMap<String, String>) -> Result<ScriptHost, ScriptError> {
        let engine = new_engine();
        let mut scripts = Vec::new();
        for (name, source) in sources {
            let ast = engine.compile(source).map_err(|e| ScriptError { script: name.clone(), message: e.to_string() })?;
            let has = |function: &str, params: usize| ast.iter_functions().any(|f| f.name == function && f.params.len() == params);
            let script = Script {
                name: name.clone(),
                has_begin: has("begin", 1),
                has_finish: has("finish", 1),
                has_on_event: has("on_event", 2),
                ast,
            };
            if !script.has_finish && !script.has_on_event {
                return Err(ScriptError {
                    script: name.clone(),
                    message: "defines neither finish(game) nor on_event(game, event), so it would never run".into(),
                });
            }
            scripts.push(script);
        }
        Ok(ScriptHost { engine, scripts })
    }

    /// Names of the round types the scripts define.
    pub fn round_kinds(&self) -> Vec<&str> {
        self.scripts.iter().filter(|s| s.has_finish).map(|s| s.name.as_str()).collect()
    }

    /// Hooks the scripts into a game: registers their round types and
    /// subscribes their event handlers.
    pub fn install(self: &Arc<Self>, game: &mut Game) {
        let vars = Arc::new(Mutex::new(BTreeMap::new()));
        for (index, script) in self.scripts.iter().enumerate() {
            if script.has_finish {
                let (host, vars) = (self.clone(), vars.clone());
                game.register_round_type(
                    script.name.clone(),
                    Box::new(move |params| {
                        Box::new(ScriptRound { host: host.clone(), index, params: params.clone(), vars: vars.clone() })
                    }),
                );
            }
        }
        if self.scripts.iter().any(|s| s.has_on_event) {
            game.add_hook(Box::new(ScriptHook { host: self.clone(), vars }));
        }
    }

    /// Runs one function of one script against the game, then carries out
    /// what it asked for.
    fn run(
        &self,
        game: &mut Game,
        index: usize,
        function: &str,
        event: Option<&GameEvent>,
        params: &BTreeMap<String, i64>,
        vars: &Arc<Mutex<BTreeMap<String, i64>>>,
    ) {
        let script = &self.scripts[index];
        let ctx = Ctx {
            players: game
                .players()
                .iter()
                .map(|p| PlayerSnap {
                    name: p.name.clone(),
                    gold: p.gold,
                    health: p.health,
                    level: p.level as INT,
                    alive: p.alive,
                    units: p.units().count() as INT,
                    items: p.items.len() as INT,
                    streak: p.streak as INT,
                })
                .collect(),
            stage: game.stage() as INT,
            round: game.round_index() as INT + 1,
            round_name: game.round_name().to_string(),
            params: params.clone(),
            rng: game.rng().fork(0x005C_2197),
            actions: Vec::new(),
            vars: vars.clone(),
        };
        let handle = ScriptGame(Arc::new(Mutex::new(ctx)));
        let mut scope = Scope::new();
        let result: Result<Dynamic, Box<EvalAltResult>> = match event {
            Some(event) => self.engine.call_fn(&mut scope, &script.ast, function, (handle.clone(), event_map(event))),
            None => self.engine.call_fn(&mut scope, &script.ast, function, (handle.clone(),)),
        };
        let actions = std::mem::take(&mut handle.0.lock().expect("the script has finished").actions);
        let mut problems: Vec<String> = Vec::new();
        if let Err(e) = result {
            problems.push(format!("error in {function}: {e}"));
        }
        for action in actions {
            if let Err(problem) = apply(game, action, &script.name) {
                problems.push(format!("error in {function}: {problem}"));
            }
        }
        for message in problems {
            eprintln!("script `{}`: {message}", script.name);
            game.emit(GameEvent::Script { script: script.name.clone(), message });
        }
    }
}

fn apply(game: &mut Game, action: Action, script: &str) -> Result<(), String> {
    let count = game.players().len();
    let player = |id: INT| -> Result<PlayerId, String> {
        usize::try_from(id).ok().filter(|i| *i < count).map(|i| PlayerId(i as u8)).ok_or(format!("there is no player {id}"))
    };
    match action {
        Action::Gold(p, n) => game.add_gold(player(p)?, n),
        Action::Xp(p, n) => game.add_xp(player(p)?, n.clamp(0, u32::MAX as INT) as u32),
        Action::Item(p, item) => {
            if game.content().item(&item).is_none() {
                return Err(format!("there is no item \"{item}\""));
            }
            game.give_item(player(p)?, &item);
        }
        Action::RandomItem(p) => game.give_random_component(player(p)?),
        Action::Unit(p, unit) => {
            if game.content().unit(&unit).is_none() {
                return Err(format!("there is no unit \"{unit}\""));
            }
            let p = player(p)?;
            game.take_from_pool(&unit);
            game.add_unit(p, &unit, Vec::new());
        }
        Action::Augment(p, augment) => {
            if game.content().augment(&augment).is_none() {
                return Err(format!("there is no augment \"{augment}\""));
            }
            game.give_augment(player(p)?, &augment);
        }
        Action::Damage(p, n) => game.damage_player(player(p)?, n.max(0)),
        Action::Log(message) => game.emit(GameEvent::Script { script: script.to_string(), message }),
        Action::FightAll => PvpRound.finish(game),
    }
    Ok(())
}

/// A round whose behaviour is a script.
struct ScriptRound {
    host: Arc<ScriptHost>,
    index: usize,
    params: BTreeMap<String, i64>,
    vars: Arc<Mutex<BTreeMap<String, i64>>>,
}

impl RoundType for ScriptRound {
    fn kind(&self) -> &str {
        &self.host.scripts[self.index].name
    }

    fn begin(&mut self, game: &mut Game) {
        if self.host.scripts[self.index].has_begin {
            self.host.run(game, self.index, "begin", None, &self.params, &self.vars);
        }
    }

    fn pick(&mut self, _game: &mut Game, _player: PlayerId, _option: usize) -> Result<(), CommandError> {
        Err(CommandError::NothingToPick)
    }

    fn finish(&mut self, game: &mut Game) {
        self.host.run(game, self.index, "finish", None, &self.params, &self.vars);
    }
}

/// Feeds game events to the scripts that listen for them.
struct ScriptHook {
    host: Arc<ScriptHost>,
    vars: Arc<Mutex<BTreeMap<String, i64>>>,
}

impl GameHook for ScriptHook {
    fn on_event(&mut self, game: &mut Game, event: &GameEvent) {
        // Scripts do not hear each other's log lines and errors, which
        // would make it easy to loop.
        if matches!(event, GameEvent::Script { .. }) {
            return;
        }
        let none = BTreeMap::new();
        for index in 0..self.host.scripts.len() {
            if self.host.scripts[index].has_on_event {
                self.host.run(game, index, "on_event", Some(event), &none, &self.vars);
            }
        }
    }
}
