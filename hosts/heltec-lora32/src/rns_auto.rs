//! RNS AutoInterface discovery protocol, wire-exact against Python RNS 1.3.1
//! ([`AutoInterface.py`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/AutoInterface.py)).
//!
//! M4 implemented the **discovery handshake**: the SHA-256 peering token, the
//! RFC 5952 link-local rendering the token hashes over, and a fixed-capacity
//! peer table. M5 adds the **data plane** — [`RnsAutoInterface`], a regular
//! engine [`Interface`] (and [`SharedMulticastInterface`]) that carries
//! unicast RNS packets on [`DATA_PORT`]: the async loop fans the engine's
//! writes out to every discovered peer and feeds inbound datagrams back in.
//! Discovery is multicast-found, data is unicast-delivered — hence
//! shared-*multicast*, not shared-*broadcast*.
//!
//! The protocol: each node periodically multicasts a beacon whose payload is
//! `sha256(group_id ++ <its own link-local, as a canonical string>)`. A
//! receiver recomputes that hash from the datagram's *source* address and
//! peers only on a match — so the token authenticates the sender's source
//! address as a member of the shared group (`AutoInterface.py` L364-L366,
//! L491-L494).

use core::fmt::Write as _;
use core::net::Ipv6Addr;

use heapless::{Deque, String as HString, Vec as HVec};
use personal_rns::crypto::sha256;
use personal_rns::interfaces::{
    Capabilities, ConnectionState, Interface, InterfaceId, InterfaceMode, MediumKind,
    SharedMulticastInterface,
};

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

/// Unicast data port (`AutoInterface.py` L48): inbound RNS packets arrive
/// here, and [`RnsAutoInterface`]'s writes are unicast to each peer's data
/// port (`process_outgoing`, L636-647).
pub const DATA_PORT: u16 = 42671;

/// Fixed hardware MTU RNS pins for the AutoInterface (`AutoInterface.py` L44).
/// Sizes the scratch buffers the engine reads/writes packets through.
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

/// Stable engine-facing id for this host's single RNS AutoInterface. Opaque to
/// the engine (it only compares ids); a readable label rather than a hash so
/// it is obvious in logs which interface a `fire_on` entry names.
pub const INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltec-s3-rnsaut");

/// Per-packet capacity of the data-plane queues. M5 carries announces, which
/// sit well under this; raise toward [`HW_MTU`] once the data plane carries
/// link and resource packets. Kept small so the whole interface fits a stack
/// local without crowding the other socket buffers.
const DATA_PACKET_CAP: usize = 512;

/// How many packets each direction buffers between turns of the async loop.
const DATA_QUEUE_DEPTH: usize = 4;

/// What went wrong moving a packet across the data-plane queues.
#[derive(Debug)]
pub enum DataPlaneError {
    /// The packet exceeded [`DATA_PACKET_CAP`] — too large to queue (on write)
    /// or to hand to the caller's buffer (on read).
    PacketTooLarge,
    /// The outbound queue was full; the packet was dropped rather than block.
    OutboundFull,
}

/// The engine-facing half of the RNS AutoInterface: a regular [`Interface`]
/// (and [`SharedMulticastInterface`]) backed by two small packet queues.
///
/// The async discovery loop owns the actual UDP sockets and the peer set; it
/// [`inject_inbound`](Self::inject_inbound)s datagrams that arrive on the data
/// port and drains [`take_outbound`](Self::take_outbound) to fan the engine's
/// writes out as unicast to every discovered peer. This keeps the synchronous,
/// non-blocking [`Interface`] surface the engine drives cleanly separated from
/// embassy-net's async socket I/O — the same queue decoupling the in-core
/// `NoAllocLoopback` uses between its two ends. The peer fanout lives in the
/// loop, not here, so this stays a faithful actor and not a routing decision.
pub struct RnsAutoInterface {
    inbound: Deque<HVec<u8, DATA_PACKET_CAP>, DATA_QUEUE_DEPTH>,
    outbound: Deque<HVec<u8, DATA_PACKET_CAP>, DATA_QUEUE_DEPTH>,
}

impl RnsAutoInterface {
    pub fn new() -> Self {
        Self {
            inbound: Deque::new(),
            outbound: Deque::new(),
        }
    }

    /// Queue a datagram that arrived on the data port for the engine to read on
    /// its next [`try_read`](Interface::try_read). Returns whether it was
    /// accepted — dropped if it exceeds [`DATA_PACKET_CAP`] or the inbound
    /// queue is full.
    pub fn inject_inbound(&mut self, bytes: &[u8]) -> bool {
        let Ok(packet) = HVec::from_slice(bytes) else {
            return false; // larger than DATA_PACKET_CAP
        };
        self.inbound.push_back(packet).is_ok()
    }

    /// Pop one packet the engine wrote, copying it into `buf` (which must be at
    /// least [`DATA_PACKET_CAP`]). Returns the byte length, or `None` when the
    /// outbound queue is empty.
    pub fn take_outbound(&mut self, buf: &mut [u8]) -> Option<usize> {
        let packet = self.outbound.pop_front()?;
        let n = packet.len();
        buf[..n].copy_from_slice(&packet);
        Some(n)
    }
}

impl Interface for RnsAutoInterface {
    type Error = DataPlaneError;

    fn id(&self) -> InterfaceId {
        INTERFACE_ID
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            // Switched fabric: a packet reaches the next hop by per-peer unicast
            // fanout the engine drives via its fire_on lists, not by the medium
            // re-propagating a transmission — so repeats is false (unlike a
            // shared-broadcast medium, where retransmission IS the propagation).
            repeats: false,
        }
    }

    fn mode(&self) -> InterfaceMode {
        InterfaceMode::Full
    }

    fn medium_kind(&self) -> MediumKind {
        MediumKind::Multicast
    }

    fn state(&self) -> ConnectionState {
        // The interface exists only once WiFi + the IP stack are up, and the
        // loop never tears it down, so it is always Connected from the engine's
        // view; per-peer reachability is the peer table's concern, not this.
        ConnectionState::Connected
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        let Some(packet) = self.inbound.pop_front() else {
            return Ok(None);
        };
        if packet.len() > buf.len() {
            return Err(DataPlaneError::PacketTooLarge);
        }
        buf[..packet.len()].copy_from_slice(&packet);
        Ok(Some(packet.len()))
    }

    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error> {
        let queued: HVec<u8, DATA_PACKET_CAP> =
            HVec::from_slice(packet).map_err(|_| DataPlaneError::PacketTooLarge)?;
        self.outbound
            .push_back(queued)
            .map_err(|_| DataPlaneError::OutboundFull)
    }
}

impl SharedMulticastInterface for RnsAutoInterface {}

impl Default for RnsAutoInterface {
    fn default() -> Self {
        Self::new()
    }
}
