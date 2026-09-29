//! Each node periodically multicasts a beacon whose payload is `sha256(group_id ++ <its own link-local, as a canonical string>)`; a receiver recomputes that hash from the datagram's *source* address and peers only on a match. The match classifies the beacon into a configured discovery lane; because the group material is observable and copyable, it is not peer authentication. Data is unicast to each peer's [`DEFAULT_DATA_PORT`]; discovery completes over multicast either direction plus the unicast reverse-peering channel ([`UNICAST_DISCOVERY_PORT`]).

use core::fmt::Write as _;
use core::net::Ipv6Addr;

use heapless::{String as HString, Vec as HVec};

use crate::crypto::{sha256, Sha256PrefixState};
use crate::interfaces::{DiscoveryGroupId, DiscoveryGroupSet, MacAddress, MAX_DISCOVERY_GROUPS};

pub const GROUP_NAME: &str = "reticulum";
pub const GROUP_ID: &[u8] = GROUP_NAME.as_bytes();
/// RNS derives this fixed address from [`GROUP_ID`] using group-hash bytes `[2..14]`; the literal must be recomputed if the group changes.
pub const DISCOVERY_GROUP: Ipv6Addr =
    Ipv6Addr::new(0xff12, 0x0, 0xd70b, 0xfb1c, 0x16e4, 0x5e39, 0x485e, 0x31e1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryScope {
    Link,
    Admin,
    Site,
    Organisation,
    Global,
}

impl DiscoveryScope {
    pub fn from_name(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("link") {
            Some(Self::Link)
        } else if value.eq_ignore_ascii_case("admin") {
            Some(Self::Admin)
        } else if value.eq_ignore_ascii_case("site") {
            Some(Self::Site)
        } else if value.eq_ignore_ascii_case("organisation") {
            Some(Self::Organisation)
        } else if value.eq_ignore_ascii_case("global") {
            Some(Self::Global)
        } else {
            None
        }
    }

    const fn multicast_nibble(self) -> u16 {
        match self {
            Self::Link => 0x2,
            Self::Admin => 0x4,
            Self::Site => 0x5,
            Self::Organisation => 0x8,
            Self::Global => 0xe,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MulticastAddressType {
    Temporary,
    Permanent,
}

impl MulticastAddressType {
    pub fn from_name(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("temporary") {
            Some(Self::Temporary)
        } else if value.eq_ignore_ascii_case("permanent") {
            Some(Self::Permanent)
        } else {
            None
        }
    }

    const fn multicast_nibble(self) -> u16 {
        match self {
            Self::Temporary => 0x1,
            Self::Permanent => 0x0,
        }
    }
}

pub fn discovery_group(
    group_id: &[u8],
    scope: DiscoveryScope,
    address_type: MulticastAddressType,
) -> Ipv6Addr {
    let hash = sha256(group_id);
    Ipv6Addr::new(
        0xff00 | (address_type.multicast_nibble() << 4) | scope.multicast_nibble(),
        0,
        u16::from_be_bytes([hash[2], hash[3]]),
        u16::from_be_bytes([hash[4], hash[5]]),
        u16::from_be_bytes([hash[6], hash[7]]),
        u16::from_be_bytes([hash[8], hash[9]]),
        u16::from_be_bytes([hash[10], hash[11]]),
        u16::from_be_bytes([hash[12], hash[13]]),
    )
}

pub const DEFAULT_DISCOVERY_PORT: u16 = 29716;
pub const UNICAST_DISCOVERY_PORT: u16 = DEFAULT_DISCOVERY_PORT + 1;

pub const DEFAULT_DATA_PORT: u16 = 42671;

/// A Prns extension for peers behind an isolating hotspot; it is distinct from the RNS UDP [`DEFAULT_DATA_PORT`].
pub const TCP_RENDEZVOUS_PORT: u16 = 42699;

pub const PEERING_TIMEOUT_MS: u64 = 22_000;
pub const PEERING_TOKEN_BYTES: usize = crate::crypto::SHA256_OUTPUT_LEN;

/// Reconstructs the EUI-64 link-local address peers use as the source when validating [`peering_token`].
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

/// The token is sent in cleartext, so ordinary equality matches RNS. It is a discovery filter,
/// not a secret or an authentication boundary.
#[derive(PartialEq, Eq)]
pub struct PeeringToken([u8; PEERING_TOKEN_BYTES]);

impl PeeringToken {
    pub fn from_beacon_prefix(bytes: &[u8]) -> Option<Self> {
        let prefix: [u8; PEERING_TOKEN_BYTES] =
            bytes.get(..PEERING_TOKEN_BYTES)?.try_into().ok()?;
        Some(Self(prefix))
    }

    pub fn as_bytes(&self) -> &[u8; PEERING_TOKEN_BYTES] {
        &self.0
    }
}

/// RNS hashes `group_id ++ canonical(addr)`; [`core::net::Ipv6Addr`]'s RFC 5952 display is byte-identical to the address string Python reports.
pub fn peering_token(addr: &Ipv6Addr) -> PeeringToken {
    peering_token_for_group(GROUP_ID, addr)
}

pub fn peering_token_for_group(group_id: &[u8], addr: &Ipv6Addr) -> PeeringToken {
    let mut rendered: HString<48> = HString::new();
    let _ = write!(rendered, "{addr}");
    PeeringToken(Sha256PrefixState::absorb(&[group_id]).digest_with_suffix(rendered.as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconVerdict {
    Peer(Ipv6Addr),
    SelfEcho,
    AuthenticationFailed,
    TooShort,
}

pub fn classify_beacon(bytes: &[u8], src: &Ipv6Addr, self_addr: &Ipv6Addr) -> BeaconVerdict {
    classify_beacon_for_group(bytes, src, self_addr, GROUP_ID)
}

pub fn classify_beacon_for_group(
    bytes: &[u8],
    src: &Ipv6Addr,
    self_addr: &Ipv6Addr,
    group_id: &[u8],
) -> BeaconVerdict {
    if src == self_addr {
        return BeaconVerdict::SelfEcho;
    }
    let Some(claimed) = PeeringToken::from_beacon_prefix(bytes) else {
        return BeaconVerdict::TooShort;
    };
    if claimed == peering_token_for_group(group_id, src) {
        BeaconVerdict::Peer(*src)
    } else {
        BeaconVerdict::AuthenticationFailed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerObservation {
    NewlyDiscovered,
    Refreshed,
    TableFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconObservation {
    /// Legacy API terminology: this means that the observable discovery token matched, not that
    /// the peer's identity was authenticated.
    AuthenticatedPeer {
        address: Ipv6Addr,
        peer_observation: PeerObservation,
    },
    SelfEcho,
    AuthenticationFailed,
    TooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryGroupLane(u8);

impl DiscoveryGroupLane {
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBeaconObservation {
    /// Legacy API terminology: this means that the observable discovery token matched, not that
    /// the peer's identity was authenticated.
    AuthenticatedPeer {
        address: Ipv6Addr,
        peer_observation: PeerObservation,
        lane: DiscoveryGroupLane,
    },
    SelfEcho,
    AuthenticationFailed,
    TooShort,
}

pub struct Peer {
    pub addr: Ipv6Addr,
    pub last_heard_ms: u64,
}

pub trait PeerStore {
    fn as_slice(&self) -> &[Peer];
    fn as_mut_slice(&mut self) -> &mut [Peer];
    fn push(&mut self, peer: Peer) -> Result<(), Peer>;
    fn swap_remove(&mut self, index: usize) -> Peer;
    fn clear(&mut self);
}

#[cfg(feature = "alloc")]
impl PeerStore for alloc::vec::Vec<Peer> {
    fn as_slice(&self) -> &[Peer] {
        self
    }

    fn as_mut_slice(&mut self) -> &mut [Peer] {
        self
    }

    fn push(&mut self, peer: Peer) -> Result<(), Peer> {
        alloc::vec::Vec::push(self, peer);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) -> Peer {
        alloc::vec::Vec::swap_remove(self, index)
    }

    fn clear(&mut self) {
        alloc::vec::Vec::clear(self);
    }
}

impl<const N: usize> PeerStore for HVec<Peer, N> {
    fn as_slice(&self) -> &[Peer] {
        self
    }

    fn as_mut_slice(&mut self) -> &mut [Peer] {
        self
    }

    fn push(&mut self, peer: Peer) -> Result<(), Peer> {
        HVec::push(self, peer)
    }

    fn swap_remove(&mut self, index: usize) -> Peer {
        HVec::swap_remove(self, index)
    }

    fn clear(&mut self) {
        HVec::clear(self);
    }
}

pub struct PeerTable<S> {
    peers: S,
}

impl<S: PeerStore + Default> PeerTable<S> {
    pub fn new() -> Self {
        Self {
            peers: S::default(),
        }
    }

    pub fn upsert_peer(&mut self, addr: Ipv6Addr, now_ms: u64) -> PeerObservation {
        if let Some(peer) = self
            .peers
            .as_mut_slice()
            .iter_mut()
            .find(|p| p.addr == addr)
        {
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

    pub fn refresh_known_peer(&mut self, addr: Ipv6Addr, now_ms: u64) -> bool {
        let Some(peer) = self
            .peers
            .as_mut_slice()
            .iter_mut()
            .find(|peer| peer.addr == addr)
        else {
            return false;
        };
        peer.last_heard_ms = now_ms;
        true
    }

    pub fn prune_stale_peers(&mut self, now_ms: u64) -> usize {
        let before = self.peers.as_slice().len();
        let mut i = 0;
        while i < self.peers.as_slice().len() {
            if now_ms.saturating_sub(self.peers.as_slice()[i].last_heard_ms) > PEERING_TIMEOUT_MS {
                self.peers.swap_remove(i);
            } else {
                i += 1;
            }
        }
        before - self.peers.as_slice().len()
    }

    /// Forgets every admitted peer so subsequent matching observations
    /// are admitted as [`PeerObservation::NewlyDiscovered`].
    pub fn clear_known_peers(&mut self) {
        self.peers.clear();
    }

    pub fn len(&self) -> usize {
        self.peers.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.as_slice().is_empty()
    }

    pub fn known_peer_addresses(&self) -> impl Iterator<Item = Ipv6Addr> + '_ {
        self.peers.as_slice().iter().map(|p| p.addr)
    }
}

impl<S: PeerStore + Default> Default for PeerTable<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "alloc")]
pub type HeapAutoInterfaceProtocol = AutoInterfaceProtocol<alloc::vec::Vec<Peer>>;

pub type FixedAutoInterfaceProtocol<const N: usize> = AutoInterfaceProtocol<HVec<Peer, N>>;

#[cfg(feature = "alloc")]
pub type HeapMultiAutoInterfaceProtocol = MultiAutoInterfaceProtocol<alloc::vec::Vec<Peer>>;

pub type FixedMultiAutoInterfaceProtocol<const N: usize> =
    MultiAutoInterfaceProtocol<HVec<Peer, N>>;

struct DiscoveryLane {
    id: DiscoveryGroupId,
    our_token: PeeringToken,
    token_prefix: Sha256PrefixState,
}

pub struct MultiAutoInterfaceProtocol<S> {
    our_link_local: Ipv6Addr,
    lanes: [Option<DiscoveryLane>; MAX_DISCOVERY_GROUPS],
    lane_count: u8,
    peers: PeerTable<S>,
    auth_failure_count: u32,
}

impl<S: PeerStore + Default> MultiAutoInterfaceProtocol<S> {
    pub fn new(our_mac_address: MacAddress, groups: &DiscoveryGroupSet) -> Self {
        Self::from_link_local(link_local_from_mac(our_mac_address), groups)
    }

    pub fn from_link_local(our_link_local: Ipv6Addr, groups: &DiscoveryGroupSet) -> Self {
        let mut lanes = core::array::from_fn(|_| None);
        for (slot, id) in lanes.iter_mut().zip(groups.iter()) {
            *slot = Some(DiscoveryLane {
                id: *id,
                our_token: peering_token_for_group(id.as_bytes(), &our_link_local),
                token_prefix: Sha256PrefixState::absorb(&[id.as_bytes()]),
            });
        }
        Self {
            our_link_local,
            lanes,
            lane_count: groups.len() as u8,
            peers: PeerTable::new(),
            auth_failure_count: 0,
        }
    }

    #[must_use]
    pub const fn our_link_local(&self) -> Ipv6Addr {
        self.our_link_local
    }

    pub fn lanes(
        &self,
    ) -> impl Iterator<Item = (DiscoveryGroupLane, &DiscoveryGroupId, &PeeringToken)> {
        self.lanes[..usize::from(self.lane_count)]
            .iter()
            .enumerate()
            .filter_map(|(index, lane)| {
                lane.as_ref()
                    .map(|lane| (DiscoveryGroupLane(index as u8), &lane.id, &lane.our_token))
            })
    }

    pub fn peering_token(&self, lane: DiscoveryGroupLane) -> Option<&PeeringToken> {
        self.lanes
            .get(lane.index())
            .and_then(Option::as_ref)
            .map(|lane| &lane.our_token)
    }

    pub fn peering_token_for_group(&self, group: &DiscoveryGroupId) -> Option<&PeeringToken> {
        self.lanes()
            .find(|(_, candidate, _)| *candidate == group)
            .map(|(_, _, token)| token)
    }

    pub fn observe_discovery_datagram(
        &mut self,
        src: Ipv6Addr,
        bytes: &[u8],
        now_ms: u64,
    ) -> GroupBeaconObservation {
        self.observe_discovery_datagram_on(src, bytes, now_ms, self.our_link_local)
    }

    pub fn observe_discovery_datagram_on(
        &mut self,
        src: Ipv6Addr,
        bytes: &[u8],
        now_ms: u64,
        local_link_local: Ipv6Addr,
    ) -> GroupBeaconObservation {
        if src == local_link_local {
            return GroupBeaconObservation::SelfEcho;
        }
        let Some(claimed) = PeeringToken::from_beacon_prefix(bytes) else {
            return GroupBeaconObservation::TooShort;
        };
        let mut rendered: HString<48> = HString::new();
        let _ = write!(rendered, "{src}");
        let matched = self.lanes[..usize::from(self.lane_count)]
            .iter()
            .enumerate()
            .find_map(|(index, lane)| {
                let lane = lane.as_ref()?;
                let expected =
                    PeeringToken(lane.token_prefix.digest_with_suffix(rendered.as_bytes()));
                (claimed == expected).then_some(DiscoveryGroupLane(index as u8))
            });
        let Some(lane) = matched else {
            self.auth_failure_count = self.auth_failure_count.wrapping_add(1);
            return GroupBeaconObservation::AuthenticationFailed;
        };
        GroupBeaconObservation::AuthenticatedPeer {
            address: src,
            peer_observation: self.peers.upsert_peer(src, now_ms),
            lane,
        }
    }

    pub fn replace_groups(&mut self, groups: &DiscoveryGroupSet) {
        let replacement = Self::from_link_local(self.our_link_local, groups);
        self.lanes = replacement.lanes;
        self.lane_count = replacement.lane_count;
        self.peers.clear_known_peers();
        self.auth_failure_count = 0;
    }

    pub fn prune_stale_peers(&mut self, now_ms: u64) -> usize {
        self.peers.prune_stale_peers(now_ms)
    }

    pub fn clear_known_peers(&mut self) {
        self.peers.clear_known_peers();
    }

    pub fn refresh_known_peer(&mut self, addr: Ipv6Addr, now_ms: u64) -> bool {
        self.peers.refresh_known_peer(addr, now_ms)
    }

    pub fn known_peer_addresses(&self) -> impl Iterator<Item = Ipv6Addr> + '_ {
        self.peers.known_peer_addresses()
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub const fn auth_failures(&self) -> u32 {
        self.auth_failure_count
    }
}

pub struct AutoInterfaceProtocol<S> {
    our_link_local: Ipv6Addr,
    our_token: PeeringToken,
    group_token_prefix: Sha256PrefixState,
    peers: PeerTable<S>,
    auth_failure_count: u32,
}

impl<S: PeerStore + Default> AutoInterfaceProtocol<S> {
    pub fn new(our_mac_address: MacAddress) -> Self {
        Self::from_link_local(link_local_from_mac(our_mac_address))
    }

    pub fn from_link_local(our_link_local: Ipv6Addr) -> Self {
        Self::from_link_local_with_group(our_link_local, GROUP_ID)
    }

    pub fn from_link_local_with_group(our_link_local: Ipv6Addr, group_id: &[u8]) -> Self {
        Self {
            our_token: peering_token_for_group(group_id, &our_link_local),
            our_link_local,
            group_token_prefix: Sha256PrefixState::absorb(&[group_id]),
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
        match self.observe_discovery_datagram(src, bytes, now_ms) {
            BeaconObservation::AuthenticatedPeer { address, .. } => BeaconVerdict::Peer(address),
            BeaconObservation::SelfEcho => BeaconVerdict::SelfEcho,
            BeaconObservation::AuthenticationFailed => BeaconVerdict::AuthenticationFailed,
            BeaconObservation::TooShort => BeaconVerdict::TooShort,
        }
    }

    pub fn observe_discovery_datagram(
        &mut self,
        src: Ipv6Addr,
        bytes: &[u8],
        now_ms: u64,
    ) -> BeaconObservation {
        let verdict = classify_beacon_with_prefix(
            bytes,
            &src,
            &self.our_link_local,
            &self.group_token_prefix,
        );
        match verdict {
            BeaconVerdict::Peer(addr) => {
                let peer_observation = self.peers.upsert_peer(addr, now_ms);
                BeaconObservation::AuthenticatedPeer {
                    address: addr,
                    peer_observation,
                }
            }
            BeaconVerdict::AuthenticationFailed => {
                self.auth_failure_count = self.auth_failure_count.wrapping_add(1);
                BeaconObservation::AuthenticationFailed
            }
            BeaconVerdict::SelfEcho => BeaconObservation::SelfEcho,
            BeaconVerdict::TooShort => BeaconObservation::TooShort,
        }
    }

    pub fn prune_stale_peers(&mut self, now_ms: u64) -> usize {
        self.peers.prune_stale_peers(now_ms)
    }

    pub fn clear_known_peers(&mut self) {
        self.peers.clear_known_peers();
    }

    pub fn refresh_known_peer(&mut self, addr: Ipv6Addr, now_ms: u64) -> bool {
        self.peers.refresh_known_peer(addr, now_ms)
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

fn classify_beacon_with_prefix(
    bytes: &[u8],
    src: &Ipv6Addr,
    self_addr: &Ipv6Addr,
    group_token_prefix: &Sha256PrefixState,
) -> BeaconVerdict {
    if src == self_addr {
        return BeaconVerdict::SelfEcho;
    }
    let Some(claimed) = PeeringToken::from_beacon_prefix(bytes) else {
        return BeaconVerdict::TooShort;
    };
    let mut rendered: HString<48> = HString::new();
    let _ = write!(rendered, "{src}");
    if claimed == PeeringToken(group_token_prefix.digest_with_suffix(rendered.as_bytes())) {
        BeaconVerdict::Peer(*src)
    } else {
        BeaconVerdict::AuthenticationFailed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_PEERS: usize = 8;

    fn group_set(names: &[&str]) -> DiscoveryGroupSet {
        let ids: std::vec::Vec<_> = names
            .iter()
            .map(|name| DiscoveryGroupId::parse(name).expect("valid test group"))
            .collect();
        DiscoveryGroupSet::try_from_slice(&ids).expect("valid test group set")
    }

    #[test]
    fn multi_group_protocol_matches_each_upstream_lane_and_deduplicates_peers() {
        let local = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0211, 0x22ff, 0xfe33, 0x4455);
        let peer = nth_peer(4);
        let groups = group_set(&["alpha", "crossover", "zulu"]);
        let mut protocol =
            FixedMultiAutoInterfaceProtocol::<MAX_PEERS>::from_link_local(local, &groups);

        for expected_lane in 0..groups.len() {
            let group = groups.iter().nth(expected_lane).expect("configured lane");
            let token = peering_token_for_group(group.as_bytes(), &peer);
            assert!(matches!(
                protocol.observe_discovery_datagram(peer, token.as_bytes(), expected_lane as u64),
                GroupBeaconObservation::AuthenticatedPeer { address, lane, .. }
                    if address == peer && lane.index() == expected_lane
            ));
        }
        assert_eq!(protocol.peer_count(), 1);
    }

    #[test]
    fn multi_group_replies_use_the_lane_that_matched_the_beacon() {
        let local = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0211, 0x22ff, 0xfe33, 0x4455);
        let peer = nth_peer(5);
        let groups = group_set(&["alpha", "bravo"]);
        let mut protocol =
            FixedMultiAutoInterfaceProtocol::<MAX_PEERS>::from_link_local(local, &groups);
        let second = groups.iter().nth(1).expect("second lane");
        let token = peering_token_for_group(second.as_bytes(), &peer);
        let GroupBeaconObservation::AuthenticatedPeer { lane, .. } =
            protocol.observe_discovery_datagram(peer, token.as_bytes(), 1)
        else {
            panic!("second lane matches");
        };
        assert_eq!(
            protocol.peering_token(lane).map(PeeringToken::as_bytes),
            Some(peering_token_for_group(second.as_bytes(), &local).as_bytes())
        );
    }

    #[test]
    fn from_link_local_token_hashes_over_the_given_address() {
        let addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0211, 0x22ff, 0xfe33, 0x4455);
        let brain = FixedAutoInterfaceProtocol::<MAX_PEERS>::from_link_local(addr);
        assert_eq!(brain.our_link_local(), addr);
        assert_eq!(
            brain.our_peering_token().as_bytes(),
            peering_token(&addr).as_bytes(),
        );
    }

    #[test]
    fn configured_group_changes_both_multicast_address_and_discovery_token() {
        let addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0211, 0x22ff, 0xfe33, 0x4455);
        let custom_group = b"field-team";
        let mut brain =
            FixedAutoInterfaceProtocol::<MAX_PEERS>::from_link_local_with_group(addr, custom_group);
        let peer = nth_peer(4);
        let custom_token = peering_token_for_group(custom_group, &peer);

        assert!(matches!(
            brain.ingest_discovery_datagram(peer, custom_token.as_bytes(), 10),
            BeaconVerdict::Peer(observed) if observed == peer,
        ));
        assert!(matches!(
            brain.ingest_discovery_datagram(peer, peering_token(&peer).as_bytes(), 11),
            BeaconVerdict::AuthenticationFailed,
        ));
        assert_ne!(
            discovery_group(
                custom_group,
                DiscoveryScope::Link,
                MulticastAddressType::Temporary,
            ),
            DISCOVERY_GROUP,
        );
        assert_eq!(
            discovery_group(
                custom_group,
                DiscoveryScope::Site,
                MulticastAddressType::Temporary,
            ),
            Ipv6Addr::new(0xff15, 0, 0x5b71, 0x4c9f, 0x5d9a, 0xaa08, 0x8834, 0x28c8,),
        );
        assert_eq!(
            discovery_group(
                GROUP_ID,
                DiscoveryScope::Link,
                MulticastAddressType::Temporary,
            ),
            DISCOVERY_GROUP,
        );
        assert_eq!(
            DiscoveryScope::from_name("OrGaNiSaTiOn"),
            Some(DiscoveryScope::Organisation),
        );
        assert_eq!(
            MulticastAddressType::from_name("PERMANENT"),
            Some(MulticastAddressType::Permanent),
        );
    }

    #[test]
    fn mac_constructor_still_derives_link_local_via_eui64() {
        let mac = MacAddress::new([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let expected = link_local_from_mac(mac);
        let brain = FixedAutoInterfaceProtocol::<MAX_PEERS>::new(mac);
        assert_eq!(brain.our_link_local(), expected);
        assert_eq!(
            brain.our_peering_token().as_bytes(),
            peering_token(&expected).as_bytes(),
        );
    }

    fn nth_peer(n: u16) -> Ipv6Addr {
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, n + 1)
    }

    #[test]
    fn heap_peer_table_grows_past_the_old_fixed_cap() {
        let mut table = PeerTable::<alloc::vec::Vec<Peer>>::new();
        for n in 0..(MAX_PEERS as u16 * 8) {
            assert!(matches!(
                table.upsert_peer(nth_peer(n), 0),
                PeerObservation::NewlyDiscovered
            ));
        }
        assert_eq!(table.len(), MAX_PEERS * 8);
    }

    #[test]
    fn fixed_peer_table_reports_full_past_capacity() {
        let mut table = PeerTable::<HVec<Peer, 2>>::new();
        assert!(matches!(
            table.upsert_peer(nth_peer(0), 0),
            PeerObservation::NewlyDiscovered
        ));
        assert!(matches!(
            table.upsert_peer(nth_peer(1), 0),
            PeerObservation::NewlyDiscovered
        ));
        assert!(matches!(
            table.upsert_peer(nth_peer(2), 0),
            PeerObservation::TableFull
        ));
        assert!(matches!(
            table.upsert_peer(nth_peer(0), 1),
            PeerObservation::Refreshed
        ));
    }

    #[test]
    fn matching_observation_names_peer_admission_outcomes() {
        let local = nth_peer(10);
        let first_peer = nth_peer(11);
        let second_peer = nth_peer(12);
        let mut brain = FixedAutoInterfaceProtocol::<1>::from_link_local(local);
        let first_token = peering_token(&first_peer);
        let second_token = peering_token(&second_peer);

        assert_eq!(
            brain.observe_discovery_datagram(first_peer, first_token.as_bytes(), 1),
            BeaconObservation::AuthenticatedPeer {
                address: first_peer,
                peer_observation: PeerObservation::NewlyDiscovered,
            }
        );
        assert_eq!(
            brain.observe_discovery_datagram(first_peer, first_token.as_bytes(), 2),
            BeaconObservation::AuthenticatedPeer {
                address: first_peer,
                peer_observation: PeerObservation::Refreshed,
            }
        );
        assert_eq!(
            brain.observe_discovery_datagram(second_peer, second_token.as_bytes(), 3),
            BeaconObservation::AuthenticatedPeer {
                address: second_peer,
                peer_observation: PeerObservation::TableFull,
            }
        );
        assert_eq!(brain.peer_count(), 1);
    }

    #[test]
    fn known_peer_refresh_updates_liveness_without_admitting_unknown_sources() {
        let local = nth_peer(20);
        let peer = nth_peer(21);
        let unknown = nth_peer(22);
        let mut brain = FixedAutoInterfaceProtocol::<2>::from_link_local(local);

        assert_eq!(
            brain.observe_discovery_datagram(peer, peering_token(&peer).as_bytes(), 0),
            BeaconObservation::AuthenticatedPeer {
                address: peer,
                peer_observation: PeerObservation::NewlyDiscovered,
            }
        );
        assert!(!brain.refresh_known_peer(unknown, 21_000));
        assert_eq!(brain.peer_count(), 1);
        assert!(brain.refresh_known_peer(peer, 21_000));
        assert_eq!(brain.prune_stale_peers(PEERING_TIMEOUT_MS + 1), 0);
        assert_eq!(brain.prune_stale_peers(21_000 + PEERING_TIMEOUT_MS + 1), 1);
    }

    fn assert_clearing_known_peers_readmits_every_peer<S: PeerStore + Default>() {
        let local = nth_peer(30);
        let first_peer = nth_peer(31);
        let second_peer = nth_peer(32);
        let mut brain = AutoInterfaceProtocol::<S>::from_link_local(local);
        let first_token = peering_token(&first_peer);
        let second_token = peering_token(&second_peer);

        assert_eq!(
            brain.observe_discovery_datagram(first_peer, first_token.as_bytes(), 0),
            BeaconObservation::AuthenticatedPeer {
                address: first_peer,
                peer_observation: PeerObservation::NewlyDiscovered,
            }
        );
        assert_eq!(
            brain.observe_discovery_datagram(second_peer, second_token.as_bytes(), 1),
            BeaconObservation::AuthenticatedPeer {
                address: second_peer,
                peer_observation: PeerObservation::NewlyDiscovered,
            }
        );
        assert_eq!(brain.peer_count(), 2);

        brain.clear_known_peers();
        assert_eq!(brain.peer_count(), 0);
        assert_eq!(
            brain.observe_discovery_datagram(first_peer, first_token.as_bytes(), 2),
            BeaconObservation::AuthenticatedPeer {
                address: first_peer,
                peer_observation: PeerObservation::NewlyDiscovered,
            }
        );
        assert_eq!(
            brain.observe_discovery_datagram(second_peer, second_token.as_bytes(), 3),
            BeaconObservation::AuthenticatedPeer {
                address: second_peer,
                peer_observation: PeerObservation::NewlyDiscovered,
            }
        );
    }

    #[test]
    fn fixed_and_heap_peer_stores_clear_every_known_peer() {
        assert_clearing_known_peers_readmits_every_peer::<HVec<Peer, 2>>();
        #[cfg(feature = "alloc")]
        assert_clearing_known_peers_readmits_every_peer::<alloc::vec::Vec<Peer>>();
    }
}
