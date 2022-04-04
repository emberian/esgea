use petgraph::{
    graph::{NodeIndex, UnGraph},
    visit::EdgeRef,
};
use serde::{Deserialize, Serialize};

pub type Intel = u32;
pub type PlayerId = usize;

const COLORS: &[&str] = &["red", "blue", "green", "yellow"];

#[derive(Clone, Serialize, Deserialize)]
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

#[derive(Default, Copy, Clone, Serialize, Deserialize)]
pub struct Player {
    pub intel: Intel,
    /// Cause intel-spending events to be vague to the enemy
    pub hidden_signals: bool,
    /// Enemy attack locations are visible.
    pub visible_violence: bool,
    /// If you walk into an enemy during turn, you will reveal them if they are concealed.
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

#[derive(Clone, Serialize, Deserialize)]
pub struct Game {
    pub cities: UnGraph<Location, ()>,
    pub players: Vec<Player>,
}

impl Game {
    pub fn new() -> Game {
        Game {
            cities: UnGraph::new_undirected(),
            players: vec![],
        }
    }

    pub fn try_move(&mut self, pid: PlayerId, to: NodeIndex) -> bool {
        if self
            .cities
            .find_edge(self.players[pid].location, to)
            .is_none()
        {
            return false;
        }
        self.players[pid].location = to;
        true
    }

    /// Collect intel and reveal anyone on the current node.
    pub fn start_turn(&mut self, pid: PlayerId) {
        let cur_city = self
            .cities
            .node_weight(self.players[pid].location)
            .expect("moved OOB");
        let intel_add = self
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
                p.intel += intel_add;
                p.invisible = false;
            }
        }
    }

    pub fn strike(&mut self, pid: PlayerId) {}
    pub fn wait(&mut self, pid: PlayerId) {}
    pub fn capture(&mut self, pid: PlayerId) {}
    pub fn hide(&mut self, pid: PlayerId) {}
    pub fn reveal(&mut self, pid: PlayerId, loc: NodeIndex) {}
    pub fn invisible(&mut self, pid: PlayerId) {}
    pub fn prepare(&mut self, pid: PlayerId) {}

    pub fn render(&self, perspective: PlayerId) -> String {
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
}
