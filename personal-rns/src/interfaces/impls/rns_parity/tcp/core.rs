//WIP NEEDS REVIEW
//! RNS `TCPClientInterface` / `TCPServerInterface` parity. Framing is the shared
//! HDLC octet-stuffing of [`crate::interfaces::rns_serial_framing`] (the
//! `TCPInterface.py` `HDLC` class, FLAG `0x7E` / ESC `0x7D` / mask `0x20`).
//!
//! Reference (pinned): <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/TCPInterface.py>
use core::time::Duration;

use crate::interfaces::{
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceConfig, InterfaceId,
    InterfaceMode, MediumKind, TransportCapability,
};
use crate::wire::MTU;

/// We carry the engine's `wire::MTU` (500) packets. RNS's `HW_MTU` (262144) and
/// `AUTOCONFIGURE_MTU` link-MTU negotiation (`TCPInterface.py` L42, L78) are not
/// yet mirrored: a same-MTU peer interoperates; cross-MTU negotiation is a gap.
pub const TCP_MTU: usize = MTU;

// `TCPClientInterface` connect/reconnect constants (`TCPInterface.py` L80, L89).
pub const RECONNECT_WAIT: Duration = Duration::from_secs(5);
pub const INITIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

// Dead-peer detection, applied per platform in `set_timeouts_linux` /
// `set_timeouts_osx` (`TCPInterface.py` L84-L87, L181-L205): `TCP_USER_TIMEOUT`,
// then keepalive `TCP_PROBE_AFTER` / `TCP_PROBE_INTERVAL` / `TCP_PROBES`.
pub const TCP_USER_TIMEOUT: Duration = Duration::from_secs(24);
pub const TCP_KEEPIDLE: Duration = Duration::from_secs(5);
pub const TCP_KEEPINTVL: Duration = Duration::from_secs(2);
pub const TCP_KEEPCNT: u32 = 12;

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
