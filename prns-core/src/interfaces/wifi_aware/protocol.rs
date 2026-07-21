/// The rendezvous listener a responder binds on its NAN data-path interface, and the port an
/// initiator dials. Each family gets its own decade so it has room to grow without colliding: wifi_auto
/// sits in the 42690s (42699), wifi_direct owns the 42710s (42717 data, 42718 beacon), and Aware opens
/// the 42720s here. An NDP rides its own IPv6 link-local scope, but the port stays unique so a host
/// running several families never aliases one listener onto another.
pub const AWARE_RENDEZVOUS_PORT: u16 = 42_720;

pub const FAMILY_TAG: &[u8] = b"wifi-aware";

pub const AWARE_SERVICE_NAME: &str = "prns-mesh";

pub const AWARE_PASSPHRASE: &str = "prns-mesh-shared-key";

/// A node's stable-for-the-session identity, published in its Aware service info and doubling as the
/// initiator-election rank. The OS peer handle is ephemeral and asymmetric, so the token is the only
/// identifier both neighbors agree on for the same node; a backend picks a random one per boot so two
/// distinct nodes never collide.
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

/// The keeper duel that dedups the two paths a symmetric both-attempt forms: each node fires an
/// initiator path on sighting and a responder path when the peer's request lands, so a pair can settle
/// two. Both ends must pick the same survivor with no round-trip, and timing cannot decide it — neither
/// observes a consistent "first" — so the token rank does: keep the path the lower token initiated
/// (your initiator path when your token is lower, your responder path when higher; both name the same
/// physical path). An exact tie has no asymmetry to break, so distinct random tokens are the backend's job.
#[must_use]
pub fn is_keeper(role: NdpRole, local: RendezvousToken, peer: RendezvousToken) -> bool {
    matches!(role, NdpRole::Initiator) == (local < peer)
}

/// What a live data path exposes to the side that owns it: the link-local address this node uses on
/// the NAN interface for its role — the peer's address to dial as initiator, its own to bind as
/// responder — plus that interface's scope id and the rendezvous port. Each NDP is a distinct
/// interface, so a responder binds its scoped address rather than the wildcard, and concurrent peers
/// never contend for one port.
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
    /// The TCP rendezvous role is derived from the (symmetric, agreed) initiator election, not from
    /// whichever side happened to send the data-path request: the initiator dials, the responder
    /// listens, so the two peers always pick complementary ends.
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
        // The lower token's initiator path and the higher token's responder path name the same NDP
        // (lo -> hi); each end independently marks that one the keeper.
        assert!(is_keeper(NdpRole::Initiator, lo, hi));
        assert!(is_keeper(NdpRole::Responder, hi, lo));
        // The mirror NDP (hi -> lo) is the loser at both ends.
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
