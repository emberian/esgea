use petgraph::{
    graph::{NodeIndex, UnGraph},
    visit::EdgeRef,
};
use serde::{Deserialize, Serialize};
use vecmap::{VecMap};

pub type Intel = u32;
pub type PlayerId = usize;

const COLORS: &[&str] = &["red", "blue", "green", "yellow"];

pub enum GameError {
    NotEnoughIntel,
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
            return Err(GameError::NotEnoughIntel)
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

    fn note(&mut self, pid: PlayerId, obs: Observation) {
        self.event.note(pid, obs)
    }
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
        if self.players[pid].active_scan {}
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
                p.concealed = false; // TODO(N-player): visibility sets
            }
            if p.id == pid {
                p.intel += intel_income;
                p.invisible = false;
            }
        }
    }

    pub fn render(&self, perspective: PlayerId) -> String {
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
    fn intel_reveal(
        &mut self,
        pid: PlayerId,
        intel_kind: IntelKind,
    ) {
        let kind = if self.players[pid].hidden_signals {
            None
        } else {
            Some(intel_kind)
        };
        self.broadcast(Observation::Intel { by: Some(pid), kind });
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

    pub fn wait(&mut self, pid: PlayerId)  {
        self.broadcast(Observation::WaitMove { by: Some(pid) });
    }

    /// Try to capture the location for yourself.
    pub fn capture(&mut self, pid: PlayerId) -> Vec<(Option<PlayerId>, Observation)> {
        self.cities
            .node_weight_mut(self.players[pid].location)
            .unwrap()
            .control = Some(pid);
        vec![(
            None,
            Observation::Capture {
                by: pid,
                at: self.players[pid].location,
            },
        )]
    }

    /// Hide your intel emissions.
    pub fn hide_signals(&mut self, pid: PlayerId) -> GameResult {
        if self.players[pid].hidden_signals {
            return Err(GameError::WouldNoop)
        }
        self.players[pid].purchase(IntelKind::HideSignals)?;
        self.intel_reveal(pid, IntelKind::HideSignals);
        self.players[pid].hidden_signals = true;
        Ok(())
    }

    /// Attempt to become invisible.
    pub fn invisible_action(&mut self, pid: PlayerId) -> GameResult {
        if self.players[pid].invisible {
            return Err(GameError::WouldNoop)
        }
        self.players[pid].purchase(IntelKind::Invisible)?;
        self.intel_reveal(pid, IntelKind::Invisible);
        self.players[pid].invisible = true;
        Ok(())
    }


    /// Attempt to reveal the existence - of either anyone where you are, or a particular player!
    pub fn reveal_action(
        &mut self,
        pid: PlayerId,
        reveal: Option<PlayerId>,
    ) -> GameResult {
        self.players[pid].purchase(IntelKind::Reveal)?;
        if let Some(reveal) = reveal {
            if !self.players[reveal].invisible {
                self.note( pid,
                    Observation::Reveal {
                        who: reveal,
                        at: self.players[reveal].location,
                    }
                );
            } else {
                self.note(pid, Observation::RevealFailure { who: reveal });
            }
        } else {
            let mut reveals = vec![];
            for reveal in &self.players {
                if reveal.id != pid {
                    if !reveal.invisible && reveal.location == self.players[pid].location {

                        reveals.push(
                            Observation::Reveal {
                                who: reveal.id,
                                at: reveal.location,
                            }
                        );
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