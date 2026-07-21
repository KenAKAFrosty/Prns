/// Distinct from the WiFi Auto and WiFi Direct rendezvous ports so their listeners can coexist.
pub const AWARE_RENDEZVOUS_PORT: u16 = 42_720;

pub const FAMILY_TAG: &[u8] = b"wifi-aware";

pub const AWARE_SERVICE_NAME: &str = "prns-mesh";

pub const AWARE_PASSPHRASE: &str = "prns-mesh-shared-key";

/// The backend publishes one random token per boot because OS peer handles are ephemeral and asymmetric; both peers use it as their shared initiator-election rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RendezvousToken(u32);

impl RendezvousToken {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NdpRole {
    Initiator,
    Responder,
}

/// Both peers keep the path initiated by the lower token, selecting the same survivor without relying on inconsistent arrival order.
#[must_use]
pub fn is_keeper(role: NdpRole, local: RendezvousToken, peer: RendezvousToken) -> bool {
    matches!(role, NdpRole::Initiator) == (local < peer)
}

/// Each NDP has its own IPv6 scope, so responders bind the scoped address rather than a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwareEndpoint {
    pub addr: core::net::Ipv6Addr,
    pub scope: u32,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwareDataPlan {
    Dial {
        addr: core::net::Ipv6Addr,
        scope: u32,
        port: u16,
    },
    Listen {
        addr: core::net::Ipv6Addr,
        scope: u32,
        port: u16,
    },
}

impl NdpRole {
    #[must_use]
    pub fn data_plane(self, endpoint: AwareEndpoint) -> AwareDataPlan {
        match self {
            NdpRole::Initiator => AwareDataPlan::Dial {
                addr: endpoint.addr,
                scope: endpoint.scope,
                port: endpoint.port,
            },
            NdpRole::Responder => AwareDataPlan::Listen {
                addr: endpoint.addr,
                scope: endpoint.scope,
                port: endpoint.port,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ends_keep_the_path_the_lower_token_initiated() {
        let lo = RendezvousToken::new(3);
        let hi = RendezvousToken::new(7);
        assert!(is_keeper(NdpRole::Initiator, lo, hi));
        assert!(is_keeper(NdpRole::Responder, hi, lo));
        assert!(!is_keeper(NdpRole::Responder, lo, hi));
        assert!(!is_keeper(NdpRole::Initiator, hi, lo));
    }

    #[test]
    fn the_initiator_dials_the_peer_and_the_responder_binds_its_own_address() {
        let endpoint = AwareEndpoint {
            addr: core::net::Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
            scope: 42,
            port: AWARE_RENDEZVOUS_PORT,
        };
        assert_eq!(
            NdpRole::Initiator.data_plane(endpoint),
            AwareDataPlan::Dial {
                addr: endpoint.addr,
                scope: 42,
                port: AWARE_RENDEZVOUS_PORT,
            }
        );
        assert_eq!(
            NdpRole::Responder.data_plane(endpoint),
            AwareDataPlan::Listen {
                addr: endpoint.addr,
                scope: 42,
                port: AWARE_RENDEZVOUS_PORT,
            }
        );
    }
}
