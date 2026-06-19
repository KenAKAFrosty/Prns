use heapless::Vec as HVec;

use crate::interfaces::{
    AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};
use crate::routing::links::MAX_LINK_MTU;

pub const FRAGMENT_HEADER_LEN: usize = 5;
pub const MAX_ADVERTISEMENT_LEN: usize = 31;

pub const BLE_BITRATE_GUESS_BPS: u32 = 700_000;
pub const BLE_HW_MTU: usize = if 500 < MAX_LINK_MTU {
    500
} else {
    MAX_LINK_MTU
};

const fn ble_reticulum_uuid(last: u8) -> [u8; 16] {
    [
        0x37, 0x14, 0x5b, 0x00, 0x44, 0x2d, 0x4a, 0x94, 0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28,
        last,
    ]
}

pub const BLE_SERVICE_UUID_BYTES: [u8; 16] = ble_reticulum_uuid(0xe3);
pub const BLE_SERVICE_UUID: BleUuid = BleUuid::Bit128(BLE_SERVICE_UUID_BYTES);
pub const COLUMBA_RX_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe5));
pub const COLUMBA_TX_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe4));
pub const COLUMBA_IDENTITY_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe6));
pub const NATIVE_CONTROL_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe7));

const AD_FLAGS: u8 = 0x01;
const AD_INCOMPLETE_SERVICE_UUID128: u8 = 0x06;
const AD_SERVICE_UUID128: u8 = 0x07;
const FLAGS_LE_GENERAL_DISCOVERABLE: u8 = 0x06;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleUuid {
    Bit16(u16),
    Bit128([u8; 16]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Psm(u16);

impl Psm {
    pub const DYNAMIC_LE: core::ops::RangeInclusive<u16> = 0x0080..=0x00FF;

    pub fn new(raw: u16) -> Option<Self> {
        if Self::DYNAMIC_LE.contains(&raw) {
            Some(Self(raw))
        } else {
            None
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn as_byte(self) -> u8 {
        self.0.to_be_bytes()[1]
    }

    pub fn from_byte(byte: u8) -> Option<Self> {
        Self::new(u16::from(byte))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BleAddress([u8; 6]);

impl BleAddress {
    pub const fn new(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    pub const fn octets(&self) -> &[u8; 6] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BleIdentity([u8; 16]);

impl BleIdentity {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

pub fn encode_advertisement(out: &mut [u8]) -> Option<usize> {
    let mut writer = AdWriter::new(out);
    writer.put(AD_FLAGS, &[FLAGS_LE_GENERAL_DISCOVERABLE])?;
    let mut little_endian = BLE_SERVICE_UUID_BYTES;
    little_endian.reverse();
    writer.put(AD_SERVICE_UUID128, &little_endian)?;
    Some(writer.len())
}

pub fn contains_service(adv: &[u8]) -> bool {
    let mut little_endian = BLE_SERVICE_UUID_BYTES;
    little_endian.reverse();
    AdReader::new(adv).any(|(ad_type, body)| {
        (ad_type == AD_SERVICE_UUID128 || ad_type == AD_INCOMPLETE_SERVICE_UUID128)
            && body == little_endian
    })
}

struct AdWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> AdWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn put(&mut self, ad_type: u8, body: &[u8]) -> Option<()> {
        let field_len = 1 + body.len();
        let end = self.pos + 1 + field_len;
        let slot = self.buf.get_mut(self.pos..end)?;
        slot[0] = u8::try_from(field_len).ok()?;
        slot[1] = ad_type;
        slot[2..].copy_from_slice(body);
        self.pos = end;
        Some(())
    }

    fn len(&self) -> usize {
        self.pos
    }
}

struct AdReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> AdReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
}

impl<'a> Iterator for AdReader<'a> {
    type Item = (u8, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let field_len = *self.buf.get(self.pos)? as usize;
        if field_len == 0 {
            return None;
        }
        let ad_type = *self.buf.get(self.pos + 1)?;
        let body = self.buf.get(self.pos + 2..self.pos + 1 + field_len)?;
        self.pos += 1 + field_len;
        Some((ad_type, body))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Native,
    Columba,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkCapabilities {
    pub l2cap: Option<Psm>,
    pub link_mtu: u16,
}

const CONTROL_HELLO: u8 = 0x01;
const CONTROL_WELCOME: u8 = 0x02;
const CONTROL_CLOSE: u8 = 0x03;
const CONTROL_IDENTITY_LEN: usize = 16;
const CONTROL_CAP_LEN: usize = 3;
pub const CONTROL_MAX_LEN: usize = 1 + CONTROL_IDENTITY_LEN + CONTROL_CAP_LEN;

impl LinkCapabilities {
    fn encode(&self, out: &mut [u8; CONTROL_CAP_LEN]) {
        out[0] = match self.l2cap {
            Some(psm) => psm.as_byte(),
            None => 0,
        };
        out[1..3].copy_from_slice(&self.link_mtu.to_be_bytes());
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let psm_byte = *bytes.first()?;
        let link_mtu = u16::from_be_bytes(bytes.get(1..3)?.try_into().ok()?);
        let l2cap = if psm_byte == 0 {
            None
        } else {
            Some(Psm::from_byte(psm_byte)?)
        };
        Some(Self { l2cap, link_mtu })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    L2cap { psm: Psm },
    Gatt,
}

impl Transport {
    pub fn select(local: &LinkCapabilities, peer: &LinkCapabilities, role: HandshakeRole) -> Self {
        match (local.l2cap, peer.l2cap) {
            (Some(own), Some(theirs)) => Transport::L2cap {
                psm: match role {
                    HandshakeRole::Dialer => theirs,
                    HandshakeRole::Listener => own,
                },
            },
            _ => Transport::Gatt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeRole {
    Dialer,
    Listener,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    SelfConnection,
    DuplicateLink,
    Incompatible,
}

impl CloseReason {
    const fn as_u8(self) -> u8 {
        match self {
            CloseReason::SelfConnection => 0x01,
            CloseReason::DuplicateLink => 0x02,
            CloseReason::Incompatible => 0x03,
        }
    }

    const fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(CloseReason::SelfConnection),
            0x02 => Some(CloseReason::DuplicateLink),
            0x03 => Some(CloseReason::Incompatible),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Hello {
        identity: BleIdentity,
        capabilities: LinkCapabilities,
    },
    Welcome {
        identity: BleIdentity,
        capabilities: LinkCapabilities,
    },
    Close {
        reason: CloseReason,
    },
}

impl Control {
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        match self {
            Control::Hello {
                identity,
                capabilities,
            } => encode_greeting(CONTROL_HELLO, identity, capabilities, out),
            Control::Welcome {
                identity,
                capabilities,
            } => encode_greeting(CONTROL_WELCOME, identity, capabilities, out),
            Control::Close { reason } => {
                let slot = out.get_mut(..2)?;
                slot[0] = CONTROL_CLOSE;
                slot[1] = reason.as_u8();
                Some(2)
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (tag, body) = bytes.split_first()?;
        match *tag {
            CONTROL_HELLO => {
                let (identity, capabilities) = decode_greeting(body)?;
                Some(Control::Hello {
                    identity,
                    capabilities,
                })
            }
            CONTROL_WELCOME => {
                let (identity, capabilities) = decode_greeting(body)?;
                Some(Control::Welcome {
                    identity,
                    capabilities,
                })
            }
            CONTROL_CLOSE => Some(Control::Close {
                reason: CloseReason::from_u8(*body.first()?)?,
            }),
            _ => None,
        }
    }
}

fn encode_greeting(
    tag: u8,
    identity: &BleIdentity,
    capabilities: &LinkCapabilities,
    out: &mut [u8],
) -> Option<usize> {
    let slot = out.get_mut(..CONTROL_MAX_LEN)?;
    slot[0] = tag;
    slot[1..1 + CONTROL_IDENTITY_LEN].copy_from_slice(identity.as_bytes());
    let mut caps = [0u8; CONTROL_CAP_LEN];
    capabilities.encode(&mut caps);
    slot[1 + CONTROL_IDENTITY_LEN..CONTROL_MAX_LEN].copy_from_slice(&caps);
    Some(CONTROL_MAX_LEN)
}

fn decode_greeting(body: &[u8]) -> Option<(BleIdentity, LinkCapabilities)> {
    let identity_bytes: [u8; CONTROL_IDENTITY_LEN] =
        body.get(..CONTROL_IDENTITY_LEN)?.try_into().ok()?;
    let capabilities = LinkCapabilities::decode(
        body.get(CONTROL_IDENTITY_LEN..CONTROL_IDENTITY_LEN + CONTROL_CAP_LEN)?,
    )?;
    Some((BleIdentity::new(identity_bytes), capabilities))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub identity: BleIdentity,
    pub capabilities: LinkCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Established {
    pub identity: BleIdentity,
    pub transport: Transport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Settled(Established),
    Aborted(CloseReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    pub reply: Option<Control>,
    pub outcome: Outcome,
}

pub struct Handshake {
    role: HandshakeRole,
    local: Local,
}

impl Handshake {
    pub fn begin(role: HandshakeRole, local: Local) -> (Self, Option<Control>) {
        let opening = match role {
            HandshakeRole::Dialer => Some(Control::Hello {
                identity: local.identity,
                capabilities: local.capabilities,
            }),
            HandshakeRole::Listener => None,
        };
        (Self { role, local }, opening)
    }

    pub fn absorb(&mut self, msg: Control) -> Reaction {
        match (self.role, msg) {
            (
                HandshakeRole::Listener,
                Control::Hello {
                    identity,
                    capabilities,
                },
            ) => {
                if identity == self.local.identity {
                    return self.we_close(CloseReason::SelfConnection);
                }
                let transport =
                    Transport::select(&self.local.capabilities, &capabilities, self.role);
                Reaction {
                    reply: Some(Control::Welcome {
                        identity: self.local.identity,
                        capabilities: self.local.capabilities,
                    }),
                    outcome: Outcome::Settled(Established {
                        identity,
                        transport,
                    }),
                }
            }
            (
                HandshakeRole::Dialer,
                Control::Welcome {
                    identity,
                    capabilities,
                },
            ) => {
                if identity == self.local.identity {
                    return self.we_close(CloseReason::SelfConnection);
                }
                let transport =
                    Transport::select(&self.local.capabilities, &capabilities, self.role);
                Reaction {
                    reply: None,
                    outcome: Outcome::Settled(Established {
                        identity,
                        transport,
                    }),
                }
            }
            (_, Control::Close { reason }) => Reaction {
                reply: None,
                outcome: Outcome::Aborted(reason),
            },
            _ => self.we_close(CloseReason::Incompatible),
        }
    }

    fn we_close(&self, reason: CloseReason) -> Reaction {
        Reaction {
            reply: Some(Control::Close { reason }),
            outcome: Outcome::Aborted(reason),
        }
    }
}

pub fn keeps_duplicate(ours: &BleIdentity, theirs: &BleIdentity, our_role: HandshakeRole) -> bool {
    let we_are_lower = ours < theirs;
    matches!(our_role, HandshakeRole::Dialer) == we_are_lower
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentKind {
    Start,
    Continue,
    End,
}

impl FragmentKind {
    const fn as_u8(self) -> u8 {
        match self {
            FragmentKind::Start => 0x01,
            FragmentKind::Continue => 0x02,
            FragmentKind::End => 0x03,
        }
    }

    const fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(FragmentKind::Start),
            0x02 => Some(FragmentKind::Continue),
            0x03 => Some(FragmentKind::End),
            _ => None,
        }
    }
}

pub struct Fragment<'a> {
    pub kind: FragmentKind,
    pub seq: u16,
    pub total: u16,
    pub data: &'a [u8],
}

impl<'a> Fragment<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let need = FRAGMENT_HEADER_LEN + self.data.len();
        let slot = out.get_mut(..need)?;
        slot[0] = self.kind.as_u8();
        slot[1..3].copy_from_slice(&self.seq.to_be_bytes());
        slot[3..5].copy_from_slice(&self.total.to_be_bytes());
        slot[FRAGMENT_HEADER_LEN..].copy_from_slice(self.data);
        Some(need)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Fragment<'a>> {
        let kind = FragmentKind::from_u8(*bytes.first()?)?;
        let seq = u16::from_be_bytes(bytes.get(1..3)?.try_into().ok()?);
        let total = u16::from_be_bytes(bytes.get(3..5)?.try_into().ok()?);
        let data = bytes.get(FRAGMENT_HEADER_LEN..)?;
        Some(Fragment {
            kind,
            seq,
            total,
            data,
        })
    }
}

pub fn fragments_of(payload: &[u8], mtu: usize) -> impl Iterator<Item = Fragment<'_>> {
    let cap = mtu.saturating_sub(FRAGMENT_HEADER_LEN).max(1);
    let total = payload.len().div_ceil(cap).max(1);
    payload.chunks(cap).enumerate().map(move |(index, chunk)| {
        let kind = if total == 1 {
            FragmentKind::End
        } else if index == 0 {
            FragmentKind::Start
        } else if index + 1 == total {
            FragmentKind::End
        } else {
            FragmentKind::Continue
        };
        Fragment {
            kind,
            seq: index as u16,
            total: total as u16,
            data: chunk,
        }
    })
}

pub struct Reassembler<const N: usize> {
    buf: HVec<u8, N>,
    next_seq: u16,
    total: u16,
    active: bool,
}

impl<const N: usize> Reassembler<N> {
    pub fn new() -> Self {
        Self {
            buf: HVec::new(),
            next_seq: 0,
            total: 0,
            active: false,
        }
    }

    pub fn absorb(&mut self, fragment: &Fragment<'_>) -> Option<&[u8]> {
        if fragment.seq == 0 {
            self.buf.clear();
            self.total = fragment.total;
            self.next_seq = 0;
            self.active = true;
        }
        if !self.active || fragment.seq != self.next_seq || fragment.total != self.total {
            self.active = false;
            return None;
        }
        if self.buf.extend_from_slice(fragment.data).is_err() {
            self.active = false;
            return None;
        }
        self.next_seq += 1;
        if matches!(fragment.kind, FragmentKind::End) && self.next_seq == self.total {
            self.active = false;
            return Some(&self.buf[..]);
        }
        None
    }
}

impl<const N: usize> Default for Reassembler<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub const STREAM_FRAME_PREFIX_LEN: usize = 2;

pub fn encode_stream_frame(frame: &[u8], out: &mut [u8]) -> Option<usize> {
    let len = u16::try_from(frame.len()).ok()?;
    let total = STREAM_FRAME_PREFIX_LEN + frame.len();
    let slot = out.get_mut(..total)?;
    slot[..STREAM_FRAME_PREFIX_LEN].copy_from_slice(&len.to_be_bytes());
    slot[STREAM_FRAME_PREFIX_LEN..].copy_from_slice(frame);
    Some(total)
}

pub struct StreamDeframer<const N: usize> {
    buf: HVec<u8, N>,
}

impl<const N: usize> StreamDeframer<N> {
    pub fn new() -> Self {
        Self { buf: HVec::new() }
    }

    pub fn absorb(&mut self, bytes: &[u8]) -> bool {
        self.buf.extend_from_slice(bytes).is_ok()
    }

    pub fn next_frame(&mut self, out: &mut [u8]) -> Option<usize> {
        let prefix: [u8; STREAM_FRAME_PREFIX_LEN] =
            self.buf.get(..STREAM_FRAME_PREFIX_LEN)?.try_into().ok()?;
        let len = u16::from_be_bytes(prefix) as usize;
        let total = STREAM_FRAME_PREFIX_LEN + len;
        if self.buf.len() < total {
            return None;
        }
        let dst = out.get_mut(..len)?;
        dst.copy_from_slice(&self.buf[STREAM_FRAME_PREFIX_LEN..total]);
        self.buf.copy_within(total.., 0);
        self.buf.truncate(self.buf.len() - total);
        Some(len)
    }
}

impl<const N: usize> Default for StreamDeframer<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn descriptor(id: InterfaceId, bitrate_bps: u32) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        bitrate_bps: Some(bitrate_bps),
        hardware_mtu: Some(BLE_HW_MTU),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(byte: u8) -> BleIdentity {
        BleIdentity::new([byte; 16])
    }

    fn caps(l2cap: Option<u16>) -> LinkCapabilities {
        LinkCapabilities {
            l2cap: l2cap.and_then(Psm::new),
            link_mtu: 247,
        }
    }

    #[test]
    fn psm_admits_only_the_le_dynamic_range() {
        assert!(Psm::new(0x0080).is_some());
        assert!(Psm::new(0x00FF).is_some());
        assert!(Psm::new(0x007F).is_none());
        assert!(Psm::new(0x0100).is_none());
    }

    #[test]
    fn an_advertisement_carries_the_shared_reticulum_ble_service() {
        let mut buf = [0u8; MAX_ADVERTISEMENT_LEN];
        let len = encode_advertisement(&mut buf).unwrap();
        assert!(len <= MAX_ADVERTISEMENT_LEN);
        assert!(contains_service(&buf[..len]));
        assert!(!contains_service(&[]));
        assert!(!contains_service(&[0x02, 0x01, 0x06]));
    }

    #[test]
    fn a_criss_cross_handshake_settles_both_sides_on_the_same_l2cap_channel() {
        let lower = Local {
            identity: identity(1),
            capabilities: caps(Some(0x0081)),
        };
        let higher = Local {
            identity: identity(2),
            capabilities: caps(Some(0x0082)),
        };

        let (mut dialer, opening) = Handshake::begin(HandshakeRole::Dialer, lower);
        let (mut listener, silent) = Handshake::begin(HandshakeRole::Listener, higher);
        assert!(silent.is_none());

        let listener_reaction = listener.absorb(opening.unwrap());
        let dialer_reaction = dialer.absorb(listener_reaction.reply.unwrap());

        assert!(matches!(listener_reaction.outcome, Outcome::Settled(_)));
        assert!(matches!(dialer_reaction.outcome, Outcome::Settled(_)));
        if let (Outcome::Settled(at_listener), Outcome::Settled(at_dialer)) =
            (listener_reaction.outcome, dialer_reaction.outcome)
        {
            assert_eq!(at_listener.identity, identity(1));
            assert_eq!(at_dialer.identity, identity(2));
            let agreed = Transport::L2cap {
                psm: Psm::new(0x0082).unwrap(),
            };
            assert_eq!(at_listener.transport, agreed);
            assert_eq!(at_dialer.transport, agreed);
        }
    }

    #[test]
    fn a_gatt_only_peer_pulls_both_sides_down_to_gatt() {
        let dialer_local = Local {
            identity: identity(1),
            capabilities: caps(Some(0x0081)),
        };
        let gatt_only = Local {
            identity: identity(2),
            capabilities: caps(None),
        };
        let (mut dialer, opening) = Handshake::begin(HandshakeRole::Dialer, dialer_local);
        let (mut listener, _) = Handshake::begin(HandshakeRole::Listener, gatt_only);
        let listener_reaction = listener.absorb(opening.unwrap());
        let dialer_reaction = dialer.absorb(listener_reaction.reply.unwrap());
        assert!(matches!(listener_reaction.outcome, Outcome::Settled(_)));
        assert!(matches!(dialer_reaction.outcome, Outcome::Settled(_)));
        if let (Outcome::Settled(at_listener), Outcome::Settled(at_dialer)) =
            (listener_reaction.outcome, dialer_reaction.outcome)
        {
            assert_eq!(at_listener.transport, Transport::Gatt);
            assert_eq!(at_dialer.transport, Transport::Gatt);
        }
    }

    #[test]
    fn a_self_connection_aborts_and_closes() {
        let local = Local {
            identity: identity(5),
            capabilities: caps(Some(0x0090)),
        };
        let (mut listener, _) = Handshake::begin(HandshakeRole::Listener, local);
        let reaction = listener.absorb(Control::Hello {
            identity: identity(5),
            capabilities: caps(Some(0x0090)),
        });
        assert_eq!(
            reaction.outcome,
            Outcome::Aborted(CloseReason::SelfConnection)
        );
        assert_eq!(
            reaction.reply,
            Some(Control::Close {
                reason: CloseReason::SelfConnection
            })
        );
    }

    #[test]
    fn the_keep_rule_picks_the_same_duplicate_on_both_sides() {
        let low = identity(1);
        let high = identity(9);
        let low_dialed = keeps_duplicate(&low, &high, HandshakeRole::Dialer);
        let low_listened = keeps_duplicate(&low, &high, HandshakeRole::Listener);
        let high_listened = keeps_duplicate(&high, &low, HandshakeRole::Listener);
        let high_dialed = keeps_duplicate(&high, &low, HandshakeRole::Dialer);
        assert!(low_dialed && !low_listened);
        assert!(high_listened && !high_dialed);
        assert_eq!(low_dialed, high_listened);
    }

    #[test]
    fn a_payload_round_trips_through_fragmentation() {
        let payload: [u8; 500] = core::array::from_fn(|i| i as u8);
        let mut reassembler = Reassembler::<512>::new();
        let mut completed = None;
        for fragment in fragments_of(&payload, 64) {
            let mut buf = [0u8; 64];
            let len = fragment.encode(&mut buf).unwrap();
            let decoded = Fragment::decode(&buf[..len]).unwrap();
            if let Some(done) = reassembler.absorb(&decoded) {
                completed = Some(done.to_vec());
            }
        }
        assert_eq!(completed.as_deref(), Some(&payload[..]));
    }

    #[test]
    fn a_small_payload_is_a_single_end_fragment() {
        let payload = [1u8, 2, 3];
        let mut fragments = fragments_of(&payload, 64);
        let only = fragments.next().unwrap();
        assert_eq!(only.kind, FragmentKind::End);
        assert_eq!(only.total, 1);
        assert!(fragments.next().is_none());
    }

    #[test]
    fn a_hello_round_trips_through_the_control_codec() {
        let hello = Control::Hello {
            identity: identity(7),
            capabilities: caps(Some(0x0081)),
        };
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = hello.encode(&mut buf).unwrap();
        assert_eq!(Control::decode(&buf[..len]), Some(hello));
    }

    #[test]
    fn a_gatt_only_welcome_round_trips_with_no_psm() {
        let welcome = Control::Welcome {
            identity: identity(9),
            capabilities: LinkCapabilities {
                l2cap: None,
                link_mtu: 23,
            },
        };
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = welcome.encode(&mut buf).unwrap();
        let decoded = Control::decode(&buf[..len]).unwrap();
        assert_eq!(decoded, welcome);
        if let Control::Welcome { capabilities, .. } = decoded {
            assert!(capabilities.l2cap.is_none());
        }
    }

    #[test]
    fn every_close_reason_round_trips() {
        for reason in [
            CloseReason::SelfConnection,
            CloseReason::DuplicateLink,
            CloseReason::Incompatible,
        ] {
            let close = Control::Close { reason };
            let mut buf = [0u8; CONTROL_MAX_LEN];
            let len = close.encode(&mut buf).unwrap();
            assert_eq!(Control::decode(&buf[..len]), Some(close));
        }
    }

    #[test]
    fn the_control_codec_rejects_garbage() {
        assert_eq!(Control::decode(&[]), None);
        assert_eq!(Control::decode(&[0xFF]), None);
        assert_eq!(Control::decode(&[CONTROL_HELLO, 0x00]), None);
        assert_eq!(Control::decode(&[CONTROL_CLOSE, 0x00]), None);
    }

    #[test]
    fn control_encode_refuses_a_short_buffer() {
        let hello = Control::Hello {
            identity: identity(1),
            capabilities: caps(Some(0x0090)),
        };
        let mut tiny = [0u8; 4];
        assert_eq!(hello.encode(&mut tiny), None);
    }

    #[test]
    fn a_frame_round_trips_through_stream_framing() {
        let frame = [0x10u8, 0x20, 0x30, 0x40, 0x50];
        let mut wire = [0u8; 64];
        let n = encode_stream_frame(&frame, &mut wire).unwrap();
        assert_eq!(n, STREAM_FRAME_PREFIX_LEN + frame.len());
        let mut deframer = StreamDeframer::<256>::new();
        assert!(deframer.absorb(&wire[..n]));
        let mut out = [0u8; 64];
        let got = deframer.next_frame(&mut out).unwrap();
        assert_eq!(&out[..got], &frame);
        assert!(deframer.next_frame(&mut out).is_none());
    }

    #[test]
    fn two_frames_in_one_chunk_pop_individually() {
        let mut wire = [0u8; 64];
        let mut total = 0;
        for frame in [&[1u8, 2, 3][..], &[9u8, 8][..]] {
            total += encode_stream_frame(frame, &mut wire[total..]).unwrap();
        }
        let mut deframer = StreamDeframer::<256>::new();
        assert!(deframer.absorb(&wire[..total]));
        let mut out = [0u8; 64];
        let a = deframer.next_frame(&mut out).unwrap();
        assert_eq!(&out[..a], &[1, 2, 3]);
        let b = deframer.next_frame(&mut out).unwrap();
        assert_eq!(&out[..b], &[9, 8]);
        assert!(deframer.next_frame(&mut out).is_none());
    }

    #[test]
    fn a_frame_split_across_chunks_reassembles() {
        let frame = [7u8; 40];
        let mut wire = [0u8; 64];
        let n = encode_stream_frame(&frame, &mut wire).unwrap();
        let mut deframer = StreamDeframer::<256>::new();
        let mut out = [0u8; 64];
        assert!(deframer.absorb(&wire[..10]));
        assert!(deframer.next_frame(&mut out).is_none());
        assert!(deframer.absorb(&wire[10..n]));
        let got = deframer.next_frame(&mut out).unwrap();
        assert_eq!(&out[..got], &frame);
    }

    #[test]
    fn the_stream_deframer_reports_overflow() {
        let mut deframer = StreamDeframer::<4>::new();
        assert!(deframer.absorb(&[1, 2, 3, 4]));
        assert!(!deframer.absorb(&[5]));
    }
}
