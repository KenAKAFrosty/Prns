//! WIP — unreviewed (PipeInterface, rns_parity). API, naming, and structure may still change.

use crate::interfaces::{
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceConfig, InterfaceId,
    InterfaceMode, MediumKind, TransportCapability,
};
use crate::wire::MTU;

pub const PIPE_MTU: usize = MTU;

pub fn descriptor(id: InterfaceId) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        medium: MediumKind::DirectPeer,
        announce_rate_limit: None,
    }
}
