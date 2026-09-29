use super::*;
use personal_rns::manifold::interface_seam::Interface;
use prns_simulation::{EndpointId, VirtualInterface};

pub(super) const LEAVES: usize = 4;
pub(super) const NODE_COUNT: usize = 6;
pub(super) const SEGMENTS: [[usize; 2]; 5] = [[0, 4], [1, 4], [4, 5], [5, 2], [5, 3]];
pub(super) const PORT_COUNT: usize = SEGMENTS.len() * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Port(usize);

impl Port {
    pub(super) fn owner(self) -> usize {
        SEGMENTS[self.0 / 2][self.0 % 2]
    }

    pub(super) fn peer(self) -> Self {
        Self(self.0 ^ 1)
    }

    fn tag(self) -> [u8; 8] {
        (self.0 as u64).to_be_bytes()
    }
}

pub(super) struct BoundPort {
    pub port: Port,
    pub endpoint: EndpointId,
    pub interface: InterfaceId,
}

pub(super) fn attach(
    medium: &VirtualMedium,
    node: usize,
) -> (Vec<BoundPort>, Vec<VirtualInterface>) {
    (0..PORT_COUNT)
        .map(Port)
        .filter(|port| port.owner() == node)
        .map(|port| {
            let interface = medium
                .attach(&port.tag())
                .unwrap_or_else(|error| unreachable!("port attachment: {error}"));
            (
                BoundPort {
                    port,
                    endpoint: interface.endpoint_id(),
                    interface: interface.descriptor().id,
                },
                interface,
            )
        })
        .unzip()
}

pub(super) fn binding(nodes: &[LiveNode], port: Port) -> &BoundPort {
    nodes[port.owner()]
        .ports
        .iter()
        .find(|bound| bound.port == port)
        .unwrap_or_else(|| unreachable!("every topology port has one live owner"))
}

pub(super) fn connect_all(medium: &VirtualMedium, nodes: &[LiveNode]) {
    for index in (0..PORT_COUNT).step_by(2) {
        connect(medium, nodes, Port(index));
    }
}

pub(super) fn connect_node(medium: &VirtualMedium, nodes: &[LiveNode], node: usize) {
    for bound in &nodes[node].ports {
        connect(medium, nodes, bound.port);
    }
}

fn connect(medium: &VirtualMedium, nodes: &[LiveNode], port: Port) {
    assert_eq!(
        medium.set_reachability(
            binding(nodes, port).endpoint,
            binding(nodes, port.peer()).endpoint,
            Reachability::Reachable
        ),
        Ok(TopologyMutation::Applied)
    );
}

pub(super) fn expected_route(nodes: &[LiveNode], from: usize, to: usize) -> (u8, InterfaceId) {
    assert_ne!(from, to);
    if from < LEAVES {
        let hops = if from / 2 == to / 2 { 2 } else { 3 };
        return (hops, nodes[from].ports[0].interface);
    }
    let target_transport = LEAVES + to / 2;
    let next = if from == target_transport {
        to
    } else {
        target_transport
    };
    let port = nodes[from]
        .ports
        .iter()
        .find(|bound| bound.port.peer().owner() == next)
        .unwrap_or_else(|| unreachable!("tree next hop exists"));
    (if from == target_transport { 1 } else { 2 }, port.interface)
}

#[derive(Clone, Copy)]
pub(super) enum Side {
    Left,
    Right,
}

impl Side {
    pub(super) fn transport(self) -> usize {
        match self {
            Self::Left => 4,
            Self::Right => 5,
        }
    }

    pub(super) fn unaffected_pair(self) -> [usize; 2] {
        match self {
            Self::Left => [2, 3],
            Self::Right => [0, 1],
        }
    }
}
