use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroUsize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyConfig {
    FullyConnected,
    Explicit { max_neighbors: NonZeroUsize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    Reachable,
    Isolated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyMutation {
    Applied,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyError<Id> {
    UnknownNode(Id),
    SelfLink(Id),
    ImplicitTopology,
    NeighborCapacityReached { node: Id, maximum: usize },
}

impl<Id> TopologyError<Id> {
    pub(crate) fn map_node<Other>(self, map: impl FnOnce(Id) -> Other) -> TopologyError<Other> {
        match self {
            Self::UnknownNode(node) => TopologyError::UnknownNode(map(node)),
            Self::SelfLink(node) => TopologyError::SelfLink(map(node)),
            Self::ImplicitTopology => TopologyError::ImplicitTopology,
            Self::NeighborCapacityReached { node, maximum } => {
                TopologyError::NeighborCapacityReached {
                    node: map(node),
                    maximum,
                }
            }
        }
    }
}

impl<Id: fmt::Debug> fmt::Display for TopologyError<Id> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNode(node) => write!(formatter, "topology node {node:?} is unknown"),
            Self::SelfLink(node) => write!(formatter, "topology node {node:?} cannot reach itself"),
            Self::ImplicitTopology => formatter.write_str("fully connected topology is implicit"),
            Self::NeighborCapacityReached { node, maximum } => {
                write!(
                    formatter,
                    "topology node {node:?} reached its {maximum} neighbor limit"
                )
            }
        }
    }
}

impl<Id: fmt::Debug> std::error::Error for TopologyError<Id> {}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Topology<Id> {
    config: TopologyConfig,
    neighbors: BTreeMap<Id, BTreeSet<Id>>,
}

impl<Id: Copy + Ord> Topology<Id> {
    pub(crate) fn new(config: TopologyConfig) -> Self {
        Self {
            config,
            neighbors: BTreeMap::new(),
        }
    }

    pub(crate) fn attach(&mut self, node: Id) {
        let _ = self.neighbors.entry(node).or_default();
    }

    pub(crate) fn detach(&mut self, node: Id) {
        let Some(neighbors) = self.neighbors.remove(&node) else {
            return;
        };
        for neighbor in neighbors {
            if let Some(reverse) = self.neighbors.get_mut(&neighbor) {
                let _ = reverse.remove(&node);
            }
        }
    }

    pub(crate) fn neighbors(&self, node: Id) -> impl Iterator<Item = Id> + '_ {
        let (all, explicit) = match (self.config, self.neighbors.get(&node)) {
            (TopologyConfig::FullyConnected, Some(_)) => (Some(self.neighbors.keys()), None),
            (TopologyConfig::Explicit { .. }, Some(neighbors)) => (None, Some(neighbors.iter())),
            (_, None) => (None, None),
        };
        all.into_iter()
            .flatten()
            .chain(explicit.into_iter().flatten())
            .copied()
            .filter(move |neighbor| *neighbor != node)
    }

    pub(crate) fn reaches(&self, first: Id, second: Id) -> bool {
        let Some(neighbors) = self.neighbors.get(&first) else {
            return false;
        };
        if first == second {
            return false;
        }
        match self.config {
            TopologyConfig::FullyConnected => self.neighbors.contains_key(&second),
            TopologyConfig::Explicit { .. } => neighbors.contains(&second),
        }
    }

    pub(crate) fn set_reachability(
        &mut self,
        first: Id,
        second: Id,
        reachability: Reachability,
    ) -> Result<TopologyMutation, TopologyError<Id>> {
        for node in [first, second] {
            if !self.neighbors.contains_key(&node) {
                return Err(TopologyError::UnknownNode(node));
            }
        }
        if first == second {
            return Err(TopologyError::SelfLink(first));
        }
        let TopologyConfig::Explicit { max_neighbors } = self.config else {
            return Err(TopologyError::ImplicitTopology);
        };
        if self.reaches(first, second) == (reachability == Reachability::Reachable) {
            return Ok(TopologyMutation::Unchanged);
        }
        if reachability == Reachability::Reachable {
            for node in [first, second] {
                if self
                    .neighbors
                    .get(&node)
                    .is_some_and(|peers| peers.len() == max_neighbors.get())
                {
                    return Err(TopologyError::NeighborCapacityReached {
                        node,
                        maximum: max_neighbors.get(),
                    });
                }
            }
        }
        for (node, peer) in [(first, second), (second, first)] {
            if let Some(neighbors) = self.neighbors.get_mut(&node) {
                match reachability {
                    Reachability::Reachable => {
                        let _ = neighbors.insert(peer);
                    }
                    Reachability::Isolated => {
                        let _ = neighbors.remove(&peer);
                    }
                }
            }
        }
        Ok(TopologyMutation::Applied)
    }
}

#[cfg(test)]
mod scenario_tests;
#[cfg(test)]
mod tests;
