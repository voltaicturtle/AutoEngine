//! What a client knows, built only from what the server tells it.
//!
//! [`ClientState`] folds [`ServerMsg`]s into the latest picture of the game:
//! the seat, the content definitions, the lobby, the current [`GameView`],
//! fights waiting to be shown and a short text log. It holds no rules and
//! makes no decisions, which is the point: the client is display and input
//! only. Rendering code reads this state; it is kept free of any graphics
//! dependency so it can be tested and reused.

use std::collections::VecDeque;

use crate::combat::Team;
use crate::content::ContentSet;
use crate::game::{CombatRecord, GameEvent, Opponent, Player, PlayerId};
use crate::protocol::{GameView, ServerMsg};

/// Lines kept in the event log.
const LOG_LINES: usize = 50;

#[derive(Default)]
pub struct ClientState {
    /// Our seat, or `None` while not joined or when spectating.
    pub seat: Option<PlayerId>,
    /// Secret for taking our seat back after a disconnect.
    pub token: Option<String>,
    pub welcomed: bool,
    pub content: Option<ContentSet>,
    pub lobby: Vec<Option<String>>,
    pub started: bool,
    pub view: Option<GameView>,
    /// Fights received and not yet shown, oldest first.
    pub combats: VecDeque<CombatRecord>,
    /// Recent happenings as text, oldest first.
    pub log: VecDeque<String>,
    /// The last refusal from the server, until the client clears it.
    pub error: Option<String>,
}

impl ClientState {
    pub fn new() -> Self {
        ClientState::default()
    }

    /// Folds one server message into the state.
    pub fn apply(&mut self, message: ServerMsg) {
        match message {
            ServerMsg::Welcome { seat, token, content, .. } => {
                self.seat = seat;
                self.token = token.or(self.token.take());
                self.welcomed = true;
                match content.into_content() {
                    Ok(content) => self.content = Some(content),
                    Err(e) => self.error = Some(format!("the server sent content this client cannot use: {e}")),
                }
            }
            ServerMsg::Lobby { seats, started } => {
                self.lobby = seats;
                self.started = started;
            }
            ServerMsg::State(view) => self.view = Some(view),
            ServerMsg::Events(events) => {
                for event in &events {
                    if let Some(line) = self.describe(event) {
                        self.log.push_back(line);
                    }
                }
                while self.log.len() > LOG_LINES {
                    self.log.pop_front();
                }
            }
            ServerMsg::Combat(record) => self.combats.push_back(record),
            ServerMsg::Error { message } => self.error = Some(message),
        }
    }

    /// Our own player, if seated and the game has started.
    pub fn me(&self) -> Option<&Player> {
        let seat = self.seat?;
        self.view.as_ref()?.players.get(seat.0 as usize)
    }

    pub fn player_name(&self, id: PlayerId) -> String {
        self.view
            .as_ref()
            .and_then(|v| v.players.get(id.0 as usize))
            .map(|p| p.name.clone())
            .or_else(|| self.lobby.get(id.0 as usize).cloned().flatten())
            .unwrap_or_else(|| format!("Player {}", id.0 + 1))
    }

    /// The display name of a unit, item, trait or augment id.
    pub fn display_name(&self, id: &str) -> String {
        let Some(c) = &self.content else { return id.to_string() };
        c.unit(id)
            .map(|u| u.name.clone())
            .or_else(|| c.item(id).map(|i| i.name.clone()))
            .or_else(|| c.augment(id).map(|a| a.name.clone()))
            .or_else(|| c.trait_def(id).map(|t| t.name.clone()))
            .unwrap_or_else(|| id.to_string())
    }

    /// A line of text for events worth telling the player about.
    pub fn describe(&self, event: &GameEvent) -> Option<String> {
        let name = |id: &PlayerId| self.player_name(*id);
        Some(match event {
            GameEvent::RoundStarted { name, kind, .. } => format!("Round {name} ({kind})"),
            GameEvent::UnitUpgraded { player, unit, star, .. } => {
                format!("{} upgraded {} to {} stars", name(player), self.display_name(unit), star)
            }
            GameEvent::ChoiceMade { player, choice, .. } => {
                let pretty: Vec<String> = choice.split(" + ").map(|part| self.display_name(part)).collect();
                format!("{} picked {}", name(player), pretty.join(" with "))
            }
            GameEvent::ItemCombined { player, item, .. } => {
                format!("{} completed {}", name(player), self.display_name(item))
            }
            GameEvent::CombatFinished { home, away, winner, .. } => {
                let away = match away {
                    Opponent::Player(p) => name(p),
                    Opponent::Ghost(p) => format!("a copy of {}", name(p)),
                    Opponent::Neutral => "the neutral camp".to_string(),
                };
                let verb = match winner {
                    Some(Team(0)) => "beat",
                    Some(_) => "lost to",
                    None => "drew with",
                };
                format!("{} {verb} {away}", name(home))
            }
            GameEvent::PlayerDamaged { player, amount, health } => {
                format!("{} took {amount} damage ({} left)", name(player), (*health).max(0))
            }
            GameEvent::PlayerEliminated { player, placement } => {
                format!("{} was eliminated in place {placement}", name(player))
            }
            GameEvent::GameOver { winner } => format!("{} wins the game", name(winner)),
            GameEvent::Script { message, .. } => message.clone(),
            _ => return None,
        })
    }
}
