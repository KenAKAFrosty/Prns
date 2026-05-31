//! RNS AutoInterface discovery protocol, wire-exact against Python RNS 1.3.1
//! ([`AutoInterface.py`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py)).
//!
//! This milestone (M4) implements the **discovery handshake only**: the
//! SHA-256 peering token, the RFC 5952 link-local rendering the token hashes
//! over, and a fixed-capacity peer table. The data plane (unicast RNS packets
//! on [`DATA_PORT`]) and the `SharedBroadcastInterface` engine wiring are M5.
//!
//! The protocol: each node periodically multicasts a beacon whose payload is
//! `sha256(group_id ++ <its own link-local, as a canonical string>)`. A
//! receiver recomputes that hash from the datagram's *source* address and
//! peers only on a match — so the token authenticates the sender's source
//! address as a member of the shared group (`AutoInterface.py` L364-L366,
//! L491-L494).

use core::fmt::Write as _;
use core::net::Ipv6Addr;

use heapless::{String as HString, Vec as HVec};
use personal_rns::crypto::sha256;

/// RNS default group id; the discovery multicast group and every peering token
/// derive from it (`AutoInterface.py` L49).
pub const GROUP_ID: &[u8] = b"reticulum";

/// Multicast discovery port (`AutoInterface.py` L47).
pub const DISCOVERY_PORT: u16 = 29716;

/// Unicast reverse-peering port (`AutoInterface.py` L173 = `discovery_port + 1`).
/// A node that *heard* a peer's multicast beacon unicasts its own token back
/// here (`reverse_announce`, L477-489, driven by `peer_jobs` L394-401), so
/// discovery completes even when the reverse multicast path is blocked — e.g. a
/// mesh AP that won't forward the group across its backhaul. This is what makes
/// reference RNS AutoInterface robust to one-way multicast.
pub const UNICAST_DISCOVERY_PORT: u16 = DISCOVERY_PORT + 1;

/// Unicast data port (`AutoInterface.py` L48). The data plane lands in M5;
/// declared here so the module stays the single home for the wire spec.
#[allow(dead_code)]
pub const DATA_PORT: u16 = 42671;

/// Fixed hardware MTU RNS pins for the AutoInterface (`AutoInterface.py` L44).
/// Used by M5's data plane; declared here with [`DATA_PORT`].
#[allow(dead_code)]
pub const HW_MTU: usize = 1196;

/// Discovery multicast group for the default "reticulum" group id:
/// `ff12:0:d70b:fb1c:16e4:5e39:485e:31e1`.
///
/// RNS builds it as `"ff" + type"1" + scope"2" + ":0:"` followed by group
/// hash bytes `[2..14]` rendered as six big-endian hextets (`AutoInterface.py`
/// L202-L212; type = `MULTICAST_TEMPORARY_ADDRESS_TYPE`, scope = `SCOPE_LINK`).
/// Constant because the group id is fixed; recompute the literal if
/// [`GROUP_ID`] ever changes.
pub const DISCOVERY_GROUP: Ipv6Addr =
    Ipv6Addr::new(0xff12, 0x0, 0xd70b, 0xfb1c, 0x16e4, 0x5e39, 0x485e, 0x31e1);

/// RNS prunes a peer it has not heard from within this window
/// (`PEERING_TIMEOUT = 22.0` s, `AutoInterface.py` L61).
pub const PEERING_TIMEOUT_MS: u64 = 22_000;

/// Reconstruct the IPv6 link-local address an embassy-net SLAAC stack derives
/// from `mac` via EUI-64: `fe80::` over the 64-bit interface id formed by
/// flipping the U/L bit of the first octet and splicing `ff:fe` into the
/// middle. This is the address a peer sees as our packet source, so it is the
/// address our own [`peering_token`] must hash over.
pub fn link_local_from_mac(mac: [u8; 6]) -> Ipv6Addr {
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

/// The RNS peering token authenticating a beacon from `addr`:
/// `sha256(group_id ++ canonical(addr))` (`AutoInterface.py` L491-L494).
///
/// The address is rendered in RFC 5952 canonical form via
/// [`core::net::Ipv6Addr`]'s `Display` (lowercase, longest zero-run compressed
/// to `::`) — byte-identical to the string Python's `socket.recvfrom` reports
/// for the same source — so our token equals the peer's `expected_hash`
/// (`AutoInterface.py` L364-L366).
pub fn peering_token(addr: &Ipv6Addr) -> [u8; 32] {
    // Any IPv6 address renders in <= 45 chars; this write never truncates.
    let mut rendered: HString<48> = HString::new();
    let _ = write!(rendered, "{addr}");

    // group_id (9) + canonical address (<= 45) fits comfortably in 64.
    let mut material: HVec<u8, 64> = HVec::new();
    let _ = material.extend_from_slice(GROUP_ID);
    let _ = material.extend_from_slice(rendered.as_bytes());
    sha256(&material)
}

/// What an inbound discovery datagram turned out to be.
pub enum BeaconVerdict {
    /// A valid peering beacon from another node — peer with its source address.
    Peer,
    /// Our own multicast beacon echoed back. RNS treats this as a carrier
    /// liveness signal, not a peer (`AutoInterface.py` L514-L524).
    SelfEcho,
    /// 32+ bytes, but the token did not match `sha256(group_id ++ src)` — wrong
    /// group, a spoofed source, or a stray datagram (`AutoInterface.py` L368).
    AuthenticationFailed,
    /// Fewer than the 32 token bytes RNS reads (`HASHLENGTH // 8`).
    TooShort,
}

/// Classify an inbound discovery datagram of `bytes` from `src`, given our own
/// link-local `self_addr` (so we recognise multicast echoes of our own beacon).
pub fn classify_beacon(bytes: &[u8], src: &Ipv6Addr, self_addr: &Ipv6Addr) -> BeaconVerdict {
    if src == self_addr {
        return BeaconVerdict::SelfEcho;
    }
    if bytes.len() < 32 {
        return BeaconVerdict::TooShort;
    }
    if peering_token(src)[..] == bytes[..32] {
        BeaconVerdict::Peer
    } else {
        BeaconVerdict::AuthenticationFailed
    }
}

/// Outcome of recording a beacon against the [`PeerTable`].
pub enum PeerObservation {
    /// First time we have heard this peer.
    NewlyDiscovered,
    /// A peer already in the table; its last-heard stamp was refreshed.
    Refreshed,
    /// The table is at capacity and this is a new address — the beacon was
    /// dropped rather than evicting an existing peer.
    TableFull,
}

/// One discovered AutoInterface peer.
pub struct Peer {
    pub addr: Ipv6Addr,
    pub last_heard_ms: u64,
}

/// Fixed-capacity table of discovered peers, keyed by link-local address.
/// Capacity is small by design — a desk LAN carries a handful of nodes, and
/// the S3 keeps no allocator headroom for an unbounded map.
pub struct PeerTable<const N: usize> {
    peers: HVec<Peer, N>,
}

impl<const N: usize> PeerTable<N> {
    pub fn new() -> Self {
        Self { peers: HVec::new() }
    }

    /// Record a valid beacon from `addr` at `now_ms`, returning whether this
    /// added a new peer, refreshed a known one, or was dropped (table full).
    pub fn observe(&mut self, addr: Ipv6Addr, now_ms: u64) -> PeerObservation {
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

    /// Drop peers not heard within [`PEERING_TIMEOUT_MS`]; returns how many
    /// were removed. `saturating_sub` keeps a backwards clock from evicting
    /// everyone.
    pub fn prune(&mut self, now_ms: u64) -> usize {
        let before = self.peers.len();
        let mut i = 0;
        while i < self.peers.len() {
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

    /// The link-local address of every known peer — reverse-peering targets.
    pub fn addrs(&self) -> impl Iterator<Item = Ipv6Addr> + '_ {
        self.peers.iter().map(|p| p.addr)
    }
}

impl<const N: usize> Default for PeerTable<N> {
    fn default() -> Self {
        Self::new()
    }
}
