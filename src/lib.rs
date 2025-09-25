pub use petgraph::graph::NodeIndex;
use petgraph::{graph::UnGraph, visit::EdgeRef};
use serde::{Deserialize, Serialize};
use vecmap::VecMap;

pub type Intel = u32;
pub type PlayerId = usize;

const COLORS: &[&str] = &["red", "blue", "green", "yellow"];

#[derive(Debug)]
pub enum GameError {
    NotEnoughIntel,
    NotYourTurn,
    WouldNoop,
}

pub type GameResult = Result<(), GameError>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    /// On starting a turn with a pending powerup, the additional intel is income.
    pub pending_powerup: Option<Intel>,
    /// On starting a turn with boost, three actions are available.
    pub boost: bool,
    /// Controling this location entitles this intel per turn.
    pub base_income: Intel,
    pub name: String,
    /// Convenience, index in game graph.
    pub index: NodeIndex,
    /// Controling player, if any.
    pub control: Option<PlayerId>,
}

#[derive(Debug, Default, Copy, Clone, Serialize, Deserialize)]
pub struct Player {
    pub alive: bool,
    pub intel: Intel,
    /// Cause intel-spending events to be vague to the enemy
    pub hidden_signals: bool,
    /// Enemy attack locations are visible.
    pub visible_violence: bool,
    /// When actively scanning, you will reveal any concealed players on locations you pass through.
    pub active_scan: bool,
    /// If concealed, the peg is not observed by the enemy.
    pub concealed: bool,
    /// If invisible, concealment is ignored and the peg is never observed.
    pub invisible: bool,
    /// Convenience, index in player array.
    pub id: PlayerId,
    /// Location of peg in game graph.
    pub location: NodeIndex,
}

impl Player {
    fn purchase(&mut self, which: IntelKind) -> GameResult {
        if which.cost() > self.intel {
            return Err(GameError::NotEnoughIntel);
        }
        self.intel = self.intel.saturating_sub(which.cost());
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Game {
    pub cities: UnGraph<Location, ()>,
    pub players: Vec<Player>,
    pub event: Event,
}

impl Game {
    pub fn new() -> Game {
        Game {
            cities: UnGraph::new_undirected(),
            players: vec![],
            event: Event::default(),
        }
    }

    pub fn add_location(&mut self, name: impl Into<String>, base_income: Intel) -> NodeIndex {
        let index = self.cities.add_node(Location {
            pending_powerup: None,
            boost: false,
            base_income,
            name: name.into(),
            index: NodeIndex::new(0),
            control: None,
        });
        if let Some(location) = self.cities.node_weight_mut(index) {
            location.index = index;
        }
        index
    }

    pub fn connect_locations(&mut self, a: NodeIndex, b: NodeIndex) {
        if self.cities.find_edge(a, b).is_none() {
            self.cities.add_edge(a, b, ());
        }
    }

    pub fn spawn_player(&mut self, start_at: NodeIndex) -> PlayerId {
        let id = self.players.len();
        let mut player = Player::default();
        player.alive = true;
        player.id = id;
        player.location = start_at;
        self.players.push(player);
        self.event.private_observations.entry(id).or_default();
        id
    }

    pub fn neighbors(&self, index: NodeIndex) -> Vec<NodeIndex> {
        self.cities.neighbors(index).collect::<Vec<_>>()
    }

    pub fn locations(&self) -> Vec<Location> {
        self.cities.node_weights().cloned().collect()
    }

    pub fn reset_event(&mut self) {
        self.event = Event::default();
    }

    pub fn do_action(&mut self, pid: PlayerId, action: Action) -> GameResult {
        match action {
            Action::Strike => self.strike(pid),
            Action::Wait => self.wait(pid),
            Action::Capture => self.capture(pid),
            Action::HideSignals => self.hide_signals(pid)?,
            Action::Invisible => self.invisible_action(pid)?,
            Action::Prepare => self.prepare(pid),
            Action::Move(to) => {
                if !self.try_move(pid, to) {
                    return Err(GameError::WouldNoop);
                }
            }
            Action::Reveal(target) => self.reveal_action(pid, target)?,
        }
        Ok(())
    }

    /// A private note for a player to know.
    fn note(&mut self, pid: PlayerId, obs: Observation) {
        self.event.note(pid, obs)
    }

    /// Public information for everyone to learn.
    fn broadcast(&mut self, obs: Observation) {
        self.event.broadcast(obs)
    }

    /// Attempt a move, returning true if the move completed.
    pub fn try_move(&mut self, pid: PlayerId, to: NodeIndex) -> bool {
        if self
            .cities
            .find_edge(self.players[pid].location, to)
            .is_none()
        {
            return false;
        }
        self.players[pid].location = to;
        let mut obs = vec![];
        if self.players[pid].active_scan {
            for pl in &self.players {
                if to == pl.location && pl.id != pid && !pl.invisible {
                    obs.push(Observation::Reveal {
                        who: pl.id,
                        at: pl.location,
                    });
                }
            }
        }
        for obs in obs {
            self.note(pid, obs);
        }
        true
    }

    /// Collect intel and reveal anyone on the current node.
    pub fn start_turn(&mut self, pid: PlayerId) {
        let cur_city = self
            .cities
            .node_weight(self.players[pid].location)
            .expect("moved OOB");
        let intel_income = self
            .cities
            .node_weights()
            .filter_map(|c| {
                if c.control == Some(pid) {
                    Some(c.base_income)
                } else {
                    None
                }
            })
            .sum::<u32>()
            + cur_city.pending_powerup.unwrap_or(0);
        for p in &mut self.players {
            if p.id != pid && !p.invisible && cur_city.index == p.location {
                p.concealed = false; // TODO: N-player, make this a set?
                self.event.note(
                    pid,
                    Observation::Reveal {
                        who: p.id,
                        at: p.location,
                    },
                )
            }
            if p.id == pid {
                p.intel += intel_income;
                p.invisible = false; // invisibility expires, sadly!
            }
        }
    }

    pub fn render(&self, _perspective: PlayerId) -> String {
        // TODO: use `perspective` to conceal other players.
        let mut d = vec![String::from("graph {")];

        for location in self.cities.node_weights() {
            let size = location.base_income as f32 * 0.25;
            let color = match location.control {
                Some(idx) => COLORS[idx],
                None => "white",
            };
            let pending_powerup = location
                .pending_powerup
                .map(|x| x.to_string())
                .unwrap_or(String::new());
            let boost = if location.boost { "⚡" } else { "" };
            d.push(format!(
                "{} [ size={size} style=filled fillcolor={color} label={pending_powerup}{boost} ]",
                location.index.index()
            ))
        }
        for edge in self.cities.edge_references() {
            d.push(format!(
                "{} -- {};",
                edge.source().index(),
                edge.target().index()
            ));
        }

        d.push(String::from("}"));

        d.concat()
    }

    /// Broadcast some intel unless signals are hidden
    fn intel_reveal(&mut self, pid: PlayerId, intel_kind: IntelKind) {
        let kind = if self.players[pid].hidden_signals {
            None
        } else {
            Some(intel_kind)
        };
        self.broadcast(Observation::Intel {
            by: Some(pid),
            kind,
        });
    }

    pub fn strike(&mut self, pid: PlayerId) {
        for pl in 0..self.players.len() {
            if pl != pid {
                if self.players[pid].location == self.players[pl].location {
                    self.players[pl].alive = false;
                    let ded = Observation::Death { by: pid, of: pl };
                    self.note(pid, ded);
                    self.note(pl, ded);
                }
                if self.players[pl].visible_violence || !self.players[pl].alive {
                    self.note(
                        pl,
                        Observation::Strike {
                            by: Some(pid),
                            at: Some(self.players[pid].location),
                        },
                    );
                } else {
                    self.note(
                        pl,
                        Observation::Strike {
                            by: Some(pid),
                            at: None,
                        },
                    );
                }
            }
        }
    }

    pub fn wait(&mut self, pid: PlayerId) {
        self.broadcast(Observation::WaitMove { by: Some(pid) });
    }

    /// Try to capture the location for yourself.
    pub fn capture(&mut self, pid: PlayerId) {
        self.cities
            .node_weight_mut(self.players[pid].location)
            .unwrap()
            .control = Some(pid);
        self.broadcast(Observation::Capture {
            by: pid,
            at: self.players[pid].location,
        });
    }

    /// Hide your intel emissions.
    pub fn hide_signals(&mut self, pid: PlayerId) -> GameResult {
        if self.players[pid].hidden_signals {
            return Err(GameError::WouldNoop);
        }
        self.players[pid].purchase(IntelKind::HideSignals)?;
        self.intel_reveal(pid, IntelKind::HideSignals);
        self.players[pid].hidden_signals = true;
        Ok(())
    }

    /// Attempt to become invisible.
    pub fn invisible_action(&mut self, pid: PlayerId) -> GameResult {
        if self.players[pid].invisible {
            return Err(GameError::WouldNoop);
        }
        self.players[pid].purchase(IntelKind::Invisible)?;
        self.intel_reveal(pid, IntelKind::Invisible);
        self.players[pid].invisible = true;
        Ok(())
    }

    /// Attempt to reveal the existence - of either anyone where you are, or a particular player!
    pub fn reveal_action(&mut self, pid: PlayerId, reveal: Option<PlayerId>) -> GameResult {
        self.players[pid].purchase(IntelKind::Reveal)?;
        if let Some(reveal) = reveal {
            if !self.players[reveal].invisible {
                self.note(
                    pid,
                    Observation::Reveal {
                        who: reveal,
                        at: self.players[reveal].location,
                    },
                );
            } else {
                self.note(pid, Observation::RevealFailure { who: reveal });
            }
        } else {
            let mut reveals = vec![];
            for reveal in &self.players {
                if reveal.id != pid {
                    if !reveal.invisible && reveal.location == self.players[pid].location {
                        reveals.push(Observation::Reveal {
                            who: reveal.id,
                            at: reveal.location,
                        });
                    } else {
                        reveals.push(Observation::RevealFailure { who: reveal.id });
                    }
                }
            }
            for reveal in reveals {
                self.note(pid, reveal);
            }
        }
        self.intel_reveal(pid, IntelKind::Reveal);
        Ok(())
    }

    pub fn prepare(&mut self, pid: PlayerId) {
        self.intel_reveal(pid, IntelKind::Prepare);
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
/// Subjective information about changes to the game state.
pub enum Observation {
    Death {
        by: PlayerId,
        of: PlayerId,
    },
    Strike {
        by: Option<PlayerId>,
        at: Option<NodeIndex>,
    },
    WaitMove {
        by: Option<PlayerId>,
    },
    Capture {
        by: PlayerId,
        at: NodeIndex,
    },
    Intel {
        by: Option<PlayerId>,
        kind: Option<IntelKind>,
    },
    Reveal {
        who: PlayerId,
        at: NodeIndex,
    },
    RevealFailure {
        who: PlayerId,
    },
}

impl Observation {
    pub fn describe(&self) -> String {
        match self {
            Observation::Death { by, of } => format!("Player {} eliminated player {}", by, of),
            Observation::Strike { by, at } => match (by, at) {
                (Some(by), Some(at)) => format!("Player {} struck location {}", by, at.index()),
                (Some(by), None) => format!("Player {} launched a covert strike", by),
                (None, _) => "A mysterious strike occurred".to_string(),
            },
            Observation::WaitMove { by } => match by {
                Some(pid) => format!("Player {} waited", pid),
                None => "An unknown player waited".to_string(),
            },
            Observation::Capture { by, at } => {
                format!("Player {} captured location {}", by, at.index())
            }
            Observation::Intel { by, kind } => match (by, kind) {
                (Some(pid), Some(kind)) => format!("Player {} spent intel on {:?}", pid, kind),
                (Some(pid), None) => format!("Player {} spent intel", pid),
                (None, _) => "Intel activity detected".to_string(),
            },
            Observation::Reveal { who, at } => {
                format!("Player {} was revealed at {}", who, at.index())
            }
            Observation::RevealFailure { who } => {
                format!("Attempted reveal on player {} failed", who)
            }
        }
    }
}

/// An Event records the observations that occur between successive game states.
///
/// These are used by the server to inform players about the new state of the game,
/// without sending information that would let them cheat (hopefully!)
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Event {
    pub private_observations: VecMap<PlayerId, Vec<Observation>>,
    pub public_observations: Vec<Observation>,
}

impl Event {
    pub fn note(&mut self, pid: PlayerId, obs: Observation) {
        self.private_observations.entry(pid).or_default().push(obs);
    }

    pub fn broadcast(&mut self, obs: Observation) {
        self.public_observations.push(obs);
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum IntelKind {
    HideSignals,
    Reveal,
    Invisible,
    Prepare,
}

impl IntelKind {
    fn cost(&self) -> u32 {
        match self {
            IntelKind::HideSignals => 2,
            IntelKind::Reveal => 1,
            IntelKind::Invisible => 2,
            IntelKind::Prepare => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A player's action for a turn.
pub enum Action {
    Strike,
    Wait,
    Capture,
    HideSignals,
    Invisible,
    Prepare,
    Move(NodeIndex),
    Reveal(Option<PlayerId>),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_game() -> (Game, NodeIndex, NodeIndex) {
        let mut game = Game::new();
        let a = game.add_location("Alpha", 2);
        let b = game.add_location("Bravo", 1);
        game.connect_locations(a, b);
        (game, a, b)
    }

    #[test]
    fn start_turn_grants_income() {
        let (mut game, a, _) = demo_game();
        let player = game.spawn_player(a);
        if let Some(loc) = game.cities.node_weight_mut(a) {
            loc.control = Some(player);
            loc.pending_powerup = Some(3);
        }

        game.start_turn(player);

        assert_eq!(game.players[player].intel, 5);
        assert!(game
            .cities
            .node_weight(a)
            .unwrap()
            .pending_powerup
            .is_some());
    }

    #[test]
    fn try_move_rejects_disconnected_nodes() {
        let (mut game, a, b) = demo_game();
        let c = game.add_location("Charlie", 1);
        let player = game.spawn_player(a);
        assert!(game.try_move(player, b));
        assert!(!game.try_move(player, c));
    }

    #[test]
    fn reveal_respects_invisibility() {
        let (mut game, a, b) = demo_game();
        let spy = game.spawn_player(a);
        let observer = game.spawn_player(b);
        game.players[spy].invisible = true;
        game.players[spy].intel = 10;
        game.players[observer].intel = 10;

        // Observer attempts reveal of invisible spy, should receive failure observation.
        game.reveal_action(observer, Some(spy)).unwrap();
        let private = game
            .event
            .private_observations
            .get(&observer)
            .cloned()
            .unwrap_or_default();
        assert!(matches!(private.last(), Some(Observation::RevealFailure { who }) if *who == spy));

        game.reset_event();
        game.players[spy].invisible = false;
        game.reveal_action(observer, Some(spy)).unwrap();
        let private = game
            .event
            .private_observations
            .get(&observer)
            .cloned()
            .unwrap_or_default();
        assert!(matches!(private.last(), Some(Observation::Reveal { who, .. }) if *who == spy));
    }
}
