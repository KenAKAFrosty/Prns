//! The platform-agnostic brain of the RNS AutoInterface, wire-exact against
//! Python RNS 1.3.1
//! ([`AutoInterface.py`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py)).
//!
//! The protocol: each node periodically multicasts a beacon whose payload is
//! `sha256(group_id ++ <its own link-local, as a canonical string>)`. A
//! receiver recomputes that hash from the datagram's *source* address and peers
//! only on a match; the token authenticates the sender's source address as
//! a member of the shared group ([`AutoInterface.py` L364-L366](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py#L364-L366), [L491-L494](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py#L491-L494)). Data
//! is unicast to each discovered peer's [`DEFAULT_DATA_PORT`]; discovery completes over
//! multicast either direction plus the unicast reverse-peering channel
//! ([`UNICAST_DISCOVERY_PORT`]).
//!

use core::fmt::Write as _;
use core::net::Ipv6Addr;

use heapless::{String as HString, Vec as HVec};

use crate::crypto::sha256;
use crate::interfaces::MacAddress;

/// RNS's default group id
pub const GROUP_ID: &[u8] = b"reticulum";
/// Discovery multicast group for the default "reticulum" group id:
/// `ff12:0:d70b:fb1c:16e4:5e39:485e:31e1`.
///
/// RNS builds it as `"ff" + type"1" + scope"2" + ":0:"` followed by group hash
/// bytes `[2..14]` rendered as six big-endian hextets
/// ([`AutoInterface.py` L202-L212](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py#L202-L212);
/// type = `MULTICAST_TEMPORARY_ADDRESS_TYPE`, scope = `SCOPE_LINK`).
/// Constant because the group id is fixed; recompute the literal if [`GROUP_ID`]
/// ever changes.
pub const DISCOVERY_GROUP: Ipv6Addr =
    Ipv6Addr::new(0xff12, 0x0, 0xd70b, 0xfb1c, 0x16e4, 0x5e39, 0x485e, 0x31e1);

pub const DEFAULT_DISCOVERY_PORT: u16 = 29716;
pub const UNICAST_DISCOVERY_PORT: u16 = DEFAULT_DISCOVERY_PORT + 1;

pub const DEFAULT_DATA_PORT: u16 = 42671;

/// The interface's hardware (link-layer) MTU, distinct from Reticulum's
/// logical 500-byte `MTU`. RNS pins it at 1196 for the AutoInterface
/// ([`AutoInterface.py` L44](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py#L44)).
pub const HARDWARE_MTU: usize = 1196;
pub const PEERING_TIMEOUT_MS: u64 = 22_000;

/// Reconstruct the IPv6 link-local address an SLAAC stack derives from `mac` via
/// EUI-64: `fe80::` over the 64-bit interface id formed by flipping the U/L bit
/// of the first octet and splicing `ff:fe` into the middle. This is the address
/// a peer sees as our packet source, so it is the address our own
/// [`peering_token`] must hash over.
pub fn link_local_from_mac(mac: MacAddress) -> Ipv6Addr {
    let mac = mac.octets();
    Ipv6Addr::new(
        0xfe80,
        0,
        0,
        0,
        (((mac[0] ^ 0x02) as u16) << 8) | mac[1] as u16,
        ((mac[2] as u16) << 8) | 0x00ff,
        0xfe00 | mac[3] as u16,
        ((mac[4] as u16) << 8) | mac[5] as u16,
    )
}

/// The 32-byte peering token a beacon carries (in the clear) in its first 32
/// bytes: `sha256(group_id ++ canonical(source_addr))`. not a secret; it
/// authenticates only that the sender's source address belongs to the shared
/// group, and it travels on the wire unencrypted, so a plain (non-constant-time)
/// `==` matches RNS and leaks nothing an eavesdropper couldn't already read.
#[derive(PartialEq, Eq)]
pub struct PeeringToken([u8; 32]);

impl PeeringToken {
    pub fn from_beacon_prefix(bytes: &[u8]) -> Option<Self> {
        let prefix: [u8; 32] = bytes.get(..32)?.try_into().ok()?;
        Some(Self(prefix))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The RNS peering token authenticating a beacon from `addr`:
/// `sha256(group_id ++ canonical(addr))` ([`AutoInterface.py` L491-L494](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py#L491-L494)).
///
/// The address is rendered in RFC 5952 canonical form via
/// [`core::net::Ipv6Addr`]'s `Display` (lowercase, longest zero-run compressed
/// to `::`) which is byte-identical to the string Python's `socket.recvfrom` reports
/// for the same source. This makes our token equal the peer's `expected_hash`
/// ([`AutoInterface.py` L364-L366](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py#L364-L366)).
pub fn peering_token(addr: &Ipv6Addr) -> PeeringToken {
    // Any IPv6 address renders in <= 45 chars; this write never truncates.
    let mut rendered: HString<48> = HString::new();
    let _ = write!(rendered, "{addr}");

    // group_id (9) + canonical address (<= 45) fits comfortably in 64.
    let mut material: HVec<u8, 64> = HVec::new();
    let _ = material.extend_from_slice(GROUP_ID);
    let _ = material.extend_from_slice(rendered.as_bytes());
    PeeringToken(sha256(&material))
}

pub enum BeaconVerdict {
    Peer(Ipv6Addr),
    SelfEcho,
    AuthenticationFailed,
    TooShort,
}

pub fn classify_beacon(bytes: &[u8], src: &Ipv6Addr, self_addr: &Ipv6Addr) -> BeaconVerdict {
    if src == self_addr {
        return BeaconVerdict::SelfEcho;
    }
    let Some(claimed) = PeeringToken::from_beacon_prefix(bytes) else {
        return BeaconVerdict::TooShort;
    };
    if claimed == peering_token(src) {
        BeaconVerdict::Peer(*src)
    } else {
        BeaconVerdict::AuthenticationFailed
    }
}

pub enum PeerObservation {
    NewlyDiscovered,
    Refreshed,
    TableFull,
}

pub struct Peer {
    pub addr: Ipv6Addr,
    pub last_heard_ms: u64,
}

/// Fixed-capacity table of discovered peers, keyed by link-local address.
/// Capacity is small by design — a desk LAN carries a handful of nodes, and a
/// constrained node keeps no allocator headroom for an unbounded map.
pub struct PeerTable<const N: usize> {
    peers: HVec<Peer, N>,
}

impl<const N: usize> PeerTable<N> {
    pub fn new() -> Self {
        Self { peers: HVec::new() }
    }

    pub fn upsert_peer(&mut self, addr: Ipv6Addr, now_ms: u64) -> PeerObservation {
        if let Some(peer) = self.peers.iter_mut().find(|p| p.addr == addr) {
            peer.last_heard_ms = now_ms;
            return PeerObservation::Refreshed;
        }
        match self.peers.push(Peer {
            addr,
            last_heard_ms: now_ms,
        }) {
            Ok(()) => PeerObservation::NewlyDiscovered,
            Err(_) => PeerObservation::TableFull,
        }
    }

    /// returns how many were pruned
    pub fn prune_stale_peers(&mut self, now_ms: u64) -> usize {
        let before = self.peers.len();
        let mut i = 0;
        while i < self.peers.len() {
            //`saturating_sub` prevents a backwards clock from evicting everyone.
            if now_ms.saturating_sub(self.peers[i].last_heard_ms) > PEERING_TIMEOUT_MS {
                self.peers.swap_remove(i);
            } else {
                i += 1;
            }
        }
        before - self.peers.len()
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub fn known_peer_addresses(&self) -> impl Iterator<Item = Ipv6Addr> + '_ {
        self.peers.iter().map(|p| p.addr)
    }
}

impl<const N: usize> Default for PeerTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AutoInterfaceProtocol<const MAX_PEER_COUNT: usize> {
    our_link_local: Ipv6Addr,
    our_token: PeeringToken,
    peers: PeerTable<MAX_PEER_COUNT>,
    auth_failure_count: u32,
}

impl<const MAX_PEER_COUNT: usize> AutoInterfaceProtocol<MAX_PEER_COUNT> {
    pub fn new(our_mac_address: MacAddress) -> Self {
        let our_link_local = link_local_from_mac(our_mac_address);
        Self {
            our_token: peering_token(&our_link_local),
            our_link_local,
            peers: PeerTable::new(),
            auth_failure_count: 0,
        }
    }

    pub fn our_link_local(&self) -> Ipv6Addr {
        self.our_link_local
    }

    pub fn our_peering_token(&self) -> &PeeringToken {
        &self.our_token
    }

    pub fn ingest_discovery_datagram(
        &mut self,
        src: Ipv6Addr,
        bytes: &[u8],
        now_ms: u64,
    ) -> BeaconVerdict {
        let verdict = classify_beacon(bytes, &src, &self.our_link_local);
        match verdict {
            BeaconVerdict::Peer(addr) => {
                self.peers.upsert_peer(addr, now_ms);
            }
            BeaconVerdict::AuthenticationFailed => {
                self.auth_failure_count = self.auth_failure_count.wrapping_add(1);
            }
            BeaconVerdict::SelfEcho | BeaconVerdict::TooShort => {}
        }
        verdict
    }

    /// returns how many were pruned
    pub fn prune_stale_peers(&mut self, now_ms: u64) -> usize {
        self.peers.prune_stale_peers(now_ms)
    }

    pub fn known_peer_addresses(&self) -> impl Iterator<Item = Ipv6Addr> + '_ {
        self.peers.known_peer_addresses()
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub fn auth_failures(&self) -> u32 {
        self.auth_failure_count
    }
}
