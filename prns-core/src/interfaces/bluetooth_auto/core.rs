use heapless::Vec as HVec;

use crate::crypto::sha256_chunks;
use crate::interfaces::{
    AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};
use crate::routing::links::MAX_LINK_MTU;

pub const GROUP_ID: &[u8] = b"bluetooth-auto";

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
pub const NATIVE_DATA_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe8));

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

    pub fn from_radio_address(address: &[u8; 6]) -> Self {
        let digest = sha256_chunks(&[b"prns ble identity", address]);
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    CoreBluetooth(AppleHost),
    BlueZ(BlueZHost),
    Android(AndroidHost),
    WinRt(WinRtHost),
    Esp32(Esp32Host),
    Nrf52(Nrf52Host),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppleHost {
    MacOs,
    Ios,
    IpadOs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlueZHost {
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AndroidHost {
    Android,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WinRtHost {
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Esp32Host {
    Esp32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nrf52Host {
    Nrf52,
}

impl AppleHost {
    fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::MacOs),
            1 => Some(Self::Ios),
            2 => Some(Self::IpadOs),
            _ => None,
        }
    }
}

impl BlueZHost {
    fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Linux),
            _ => None,
        }
    }
}

impl AndroidHost {
    fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Android),
            _ => None,
        }
    }
}

impl WinRtHost {
    fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Windows),
            _ => None,
        }
    }
}

impl Esp32Host {
    fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Esp32),
            _ => None,
        }
    }
}

impl Nrf52Host {
    fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Nrf52),
            _ => None,
        }
    }
}

fn endpoint_bytes(endpoint: Endpoint) -> [u8; ENDPOINT_LEN] {
    match endpoint {
        Endpoint::CoreBluetooth(host) => [1, host as u8],
        Endpoint::BlueZ(host) => [2, host as u8],
        Endpoint::Android(host) => [3, host as u8],
        Endpoint::WinRt(host) => [4, host as u8],
        Endpoint::Esp32(host) => [5, host as u8],
        Endpoint::Nrf52(host) => [6, host as u8],
    }
}

fn decode_endpoint(bytes: &[u8]) -> Option<Endpoint> {
    let stack = *bytes.first()?;
    let host = *bytes.get(1)?;
    Some(match stack {
        1 => Endpoint::CoreBluetooth(AppleHost::from_u8(host)?),
        2 => Endpoint::BlueZ(BlueZHost::from_u8(host)?),
        3 => Endpoint::Android(AndroidHost::from_u8(host)?),
        4 => Endpoint::WinRt(WinRtHost::from_u8(host)?),
        5 => Endpoint::Esp32(Esp32Host::from_u8(host)?),
        6 => Endpoint::Nrf52(Nrf52Host::from_u8(host)?),
        _ => return None,
    })
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
const ENDPOINT_LEN: usize = 2;
const CONTROL_CAP_LEN: usize = 3;
const CONTROL_RSSI_LEN: usize = 1;
const GREETING_ID_AT: usize = 1;
const GREETING_ENDPOINT_AT: usize = GREETING_ID_AT + CONTROL_IDENTITY_LEN;
const GREETING_CAP_AT: usize = GREETING_ENDPOINT_AT + ENDPOINT_LEN;
const GREETING_RSSI_AT: usize = GREETING_CAP_AT + CONTROL_CAP_LEN;
pub const CONTROL_MAX_LEN: usize = GREETING_RSSI_AT + CONTROL_RSSI_LEN;

fn encode_rssi(rssi: Option<i8>) -> u8 {
    rssi.filter(|&dbm| dbm != i8::MIN).unwrap_or(i8::MIN) as u8
}

fn decode_rssi(byte: u8) -> Option<i8> {
    let dbm = byte as i8;
    (dbm != i8::MIN).then_some(dbm)
}

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
pub enum Arrangement {
    GattOnly,
    EitherOpens,
    Opens(Endpoint),
}

pub fn arrangement(local: Endpoint, peer: Endpoint) -> Arrangement {
    known_arrangement(local, peer)
        .or_else(|| known_arrangement(peer, local))
        .unwrap_or(Arrangement::GattOnly)
}

fn known_arrangement(a: Endpoint, b: Endpoint) -> Option<Arrangement> {
    use AppleHost::{Ios, IpadOs, MacOs};
    use Endpoint::{Android, BlueZ, CoreBluetooth, Esp32, Nrf52};
    match (a, b) {
        (CoreBluetooth(MacOs), BlueZ(host)) => Some(Arrangement::Opens(BlueZ(host))),
        (CoreBluetooth(MacOs), Android(host)) => Some(Arrangement::Opens(Android(host))),
        (CoreBluetooth(Ios | IpadOs), Android(_)) => Some(Arrangement::Opens(a)),
        (BlueZ(_), Android(_)) => Some(Arrangement::EitherOpens),
        (BlueZ(_), Nrf52(_)) => Some(Arrangement::EitherOpens),
        (Android(_), Nrf52(_)) => Some(Arrangement::EitherOpens),
        (Esp32(_), Esp32(_)) => Some(Arrangement::EitherOpens),
        (Esp32(_), Nrf52(_)) => Some(Arrangement::EitherOpens),
        (BlueZ(_), Esp32(_)) => Some(Arrangement::EitherOpens),
        (Android(_), Esp32(_)) => Some(Arrangement::EitherOpens),
        _ => None,
    }
}

pub fn we_should_be_central(
    arrangement: Arrangement,
    ours: BleIdentity,
    our_endpoint: Endpoint,
    theirs: BleIdentity,
) -> bool {
    match arrangement {
        Arrangement::Opens(opener) => opener == our_endpoint,
        Arrangement::GattOnly | Arrangement::EitherOpens => ours < theirs,
    }
}

pub fn is_keeper(
    arrangement: Arrangement,
    our_role: HandshakeRole,
    ours: BleIdentity,
    our_endpoint: Endpoint,
    theirs: BleIdentity,
) -> bool {
    matches!(our_role, HandshakeRole::Dialer)
        == we_should_be_central(arrangement, ours, our_endpoint, theirs)
}

pub fn needs_redial(
    arrangement: Arrangement,
    our_role: HandshakeRole,
    our_endpoint: Endpoint,
) -> bool {
    let we_open = matches!(arrangement, Arrangement::Opens(opener) if opener == our_endpoint);
    we_open && matches!(our_role, HandshakeRole::Listener)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2capPlan {
    Open { psm: Psm },
    Accept,
    None,
}

pub fn l2cap_plan(
    arrangement: Arrangement,
    our_role: HandshakeRole,
    our_endpoint: Endpoint,
    our_capabilities: &LinkCapabilities,
    peer_capabilities: &LinkCapabilities,
) -> L2capPlan {
    let we_are_central = matches!(our_role, HandshakeRole::Dialer);
    let we_open = match arrangement {
        Arrangement::GattOnly => return L2capPlan::None,
        Arrangement::EitherOpens => we_are_central,
        Arrangement::Opens(opener) => opener == our_endpoint,
    };
    if we_open {
        if !we_are_central {
            return L2capPlan::None;
        }
        match (our_capabilities.l2cap, peer_capabilities.l2cap) {
            (Some(_), Some(psm)) => L2capPlan::Open { psm },
            _ => L2capPlan::None,
        }
    } else {
        match (our_capabilities.l2cap, peer_capabilities.l2cap) {
            (Some(_), Some(_)) => L2capPlan::Accept,
            _ => L2capPlan::None,
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
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
        peer_rssi: Option<i8>,
    },
    Welcome {
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
        peer_rssi: Option<i8>,
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
                endpoint,
                capabilities,
                peer_rssi,
            } => encode_greeting(
                CONTROL_HELLO,
                identity,
                *endpoint,
                capabilities,
                *peer_rssi,
                out,
            ),
            Control::Welcome {
                identity,
                endpoint,
                capabilities,
                peer_rssi,
            } => encode_greeting(
                CONTROL_WELCOME,
                identity,
                *endpoint,
                capabilities,
                *peer_rssi,
                out,
            ),
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
                let (identity, endpoint, capabilities, peer_rssi) = decode_greeting(body)?;
                Some(Control::Hello {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi,
                })
            }
            CONTROL_WELCOME => {
                let (identity, endpoint, capabilities, peer_rssi) = decode_greeting(body)?;
                Some(Control::Welcome {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi,
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
    endpoint: Endpoint,
    capabilities: &LinkCapabilities,
    peer_rssi: Option<i8>,
    out: &mut [u8],
) -> Option<usize> {
    let slot = out.get_mut(..CONTROL_MAX_LEN)?;
    slot[0] = tag;
    slot[GREETING_ID_AT..GREETING_ENDPOINT_AT].copy_from_slice(identity.as_bytes());
    slot[GREETING_ENDPOINT_AT..GREETING_CAP_AT].copy_from_slice(&endpoint_bytes(endpoint));
    let mut caps = [0u8; CONTROL_CAP_LEN];
    capabilities.encode(&mut caps);
    slot[GREETING_CAP_AT..GREETING_RSSI_AT].copy_from_slice(&caps);
    slot[GREETING_RSSI_AT] = encode_rssi(peer_rssi);
    Some(CONTROL_MAX_LEN)
}

fn decode_greeting(body: &[u8]) -> Option<(BleIdentity, Endpoint, LinkCapabilities, Option<i8>)> {
    let id_end = CONTROL_IDENTITY_LEN;
    let endpoint_end = id_end + ENDPOINT_LEN;
    let cap_end = endpoint_end + CONTROL_CAP_LEN;
    let identity_bytes: [u8; CONTROL_IDENTITY_LEN] = body.get(..id_end)?.try_into().ok()?;
    let endpoint = decode_endpoint(body.get(id_end..endpoint_end)?)?;
    let capabilities = LinkCapabilities::decode(body.get(endpoint_end..cap_end)?)?;
    let peer_rssi = body.get(cap_end).copied().and_then(decode_rssi);
    Some((
        BleIdentity::new(identity_bytes),
        endpoint,
        capabilities,
        peer_rssi,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub identity: BleIdentity,
    pub endpoint: Endpoint,
    pub capabilities: LinkCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Established {
    pub identity: BleIdentity,
    pub endpoint: Endpoint,
    pub capabilities: LinkCapabilities,
    pub peer_rssi: Option<i8>,
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
    measured_rssi: Option<i8>,
}

impl Handshake {
    pub fn begin(
        role: HandshakeRole,
        local: Local,
        measured_rssi: Option<i8>,
    ) -> (Self, Option<Control>) {
        let opening = match role {
            HandshakeRole::Dialer => Some(Control::Hello {
                identity: local.identity,
                endpoint: local.endpoint,
                capabilities: local.capabilities,
                peer_rssi: measured_rssi,
            }),
            HandshakeRole::Listener => None,
        };
        (
            Self {
                role,
                local,
                measured_rssi,
            },
            opening,
        )
    }

    pub fn absorb(&mut self, msg: Control) -> Reaction {
        match (self.role, msg) {
            (
                HandshakeRole::Listener,
                Control::Hello {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi,
                },
            ) => {
                if identity == self.local.identity {
                    return self.we_close(CloseReason::SelfConnection);
                }
                Reaction {
                    reply: Some(Control::Welcome {
                        identity: self.local.identity,
                        endpoint: self.local.endpoint,
                        capabilities: self.local.capabilities,
                        peer_rssi: self.measured_rssi,
                    }),
                    outcome: Outcome::Settled(Established {
                        identity,
                        endpoint,
                        capabilities,
                        peer_rssi,
                    }),
                }
            }
            (
                HandshakeRole::Dialer,
                Control::Welcome {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi,
                },
            ) => {
                if identity == self.local.identity {
                    return self.we_close(CloseReason::SelfConnection);
                }
                Reaction {
                    reply: None,
                    outcome: Outcome::Settled(Established {
                        identity,
                        endpoint,
                        capabilities,
                        peer_rssi,
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

    fn mac() -> Endpoint {
        Endpoint::CoreBluetooth(AppleHost::MacOs)
    }

    fn ios() -> Endpoint {
        Endpoint::CoreBluetooth(AppleHost::Ios)
    }

    fn ipad() -> Endpoint {
        Endpoint::CoreBluetooth(AppleHost::IpadOs)
    }

    fn linux() -> Endpoint {
        Endpoint::BlueZ(BlueZHost::Linux)
    }

    fn android() -> Endpoint {
        Endpoint::Android(AndroidHost::Android)
    }

    fn nrf() -> Endpoint {
        Endpoint::Nrf52(Nrf52Host::Nrf52)
    }

    fn esp32() -> Endpoint {
        Endpoint::Esp32(Esp32Host::Esp32)
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
    fn mac_and_linux_open_when_linux_opens() {
        assert_eq!(arrangement(mac(), linux()), Arrangement::Opens(linux()));
        assert_eq!(arrangement(linux(), mac()), Arrangement::Opens(linux()));
    }

    #[test]
    fn mac_and_android_only_open_when_android_opens() {
        assert_eq!(arrangement(mac(), android()), Arrangement::Opens(android()));
        assert_eq!(arrangement(android(), mac()), Arrangement::Opens(android()));
    }

    #[test]
    fn apple_mobile_and_linux_stay_on_the_gatt_floor() {
        assert_eq!(arrangement(ios(), linux()), Arrangement::GattOnly);
        assert_eq!(arrangement(linux(), ios()), Arrangement::GattOnly);
        assert_eq!(arrangement(ipad(), linux()), Arrangement::GattOnly);
        assert_eq!(arrangement(linux(), ipad()), Arrangement::GattOnly);
    }

    #[test]
    fn apple_mobile_and_android_only_open_when_apple_mobile_opens() {
        assert_eq!(arrangement(ios(), android()), Arrangement::Opens(ios()));
        assert_eq!(arrangement(android(), ios()), Arrangement::Opens(ios()));
        assert_eq!(arrangement(ipad(), android()), Arrangement::Opens(ipad()));
        assert_eq!(arrangement(android(), ipad()), Arrangement::Opens(ipad()));
    }

    #[test]
    fn two_apple_devices_stay_on_the_gatt_floor() {
        assert_eq!(arrangement(ios(), mac()), Arrangement::GattOnly);
        assert_eq!(arrangement(mac(), ios()), Arrangement::GattOnly);
        assert_eq!(arrangement(ios(), ios()), Arrangement::GattOnly);
    }

    #[test]
    fn bluez_and_android_either_open_the_fast_lane() {
        let arr = arrangement(linux(), android());
        assert_eq!(arr, Arrangement::EitherOpens);
        assert_eq!(arrangement(android(), linux()), Arrangement::EitherOpens);

        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Dialer,
                linux(),
                &caps(Some(0x0083)),
                &caps(Some(0x0080)),
            ),
            L2capPlan::Open {
                psm: Psm::new(0x0080).unwrap()
            }
        );
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Listener,
                android(),
                &caps(Some(0x0080)),
                &caps(Some(0x0083)),
            ),
            L2capPlan::Accept
        );
    }

    #[test]
    fn the_nrf_either_opens_the_fast_lane_with_bluez_and_android() {
        assert_eq!(arrangement(linux(), nrf()), Arrangement::EitherOpens);
        assert_eq!(arrangement(nrf(), linux()), Arrangement::EitherOpens);
        assert_eq!(arrangement(android(), nrf()), Arrangement::EitherOpens);
        assert_eq!(arrangement(nrf(), android()), Arrangement::EitherOpens);

        let arr = arrangement(nrf(), linux());
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Dialer,
                nrf(),
                &caps(Some(0x0080)),
                &caps(Some(0x0083)),
            ),
            L2capPlan::Open {
                psm: Psm::new(0x0083).unwrap()
            }
        );
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Listener,
                nrf(),
                &caps(Some(0x0080)),
                &caps(Some(0x0083)),
            ),
            L2capPlan::Accept
        );
    }

    #[test]
    fn the_esp32_either_opens_the_fast_lane_with_its_peers() {
        assert_eq!(arrangement(esp32(), esp32()), Arrangement::EitherOpens);
        assert_eq!(arrangement(esp32(), nrf()), Arrangement::EitherOpens);
        assert_eq!(arrangement(nrf(), esp32()), Arrangement::EitherOpens);
        assert_eq!(arrangement(linux(), esp32()), Arrangement::EitherOpens);
        assert_eq!(arrangement(esp32(), linux()), Arrangement::EitherOpens);
        assert_eq!(arrangement(android(), esp32()), Arrangement::EitherOpens);
        assert_eq!(arrangement(esp32(), android()), Arrangement::EitherOpens);

        let arr = arrangement(esp32(), esp32());
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Dialer,
                esp32(),
                &caps(Some(0x0080)),
                &caps(Some(0x0080)),
            ),
            L2capPlan::Open {
                psm: Psm::new(0x0080).unwrap()
            }
        );
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Listener,
                esp32(),
                &caps(Some(0x0080)),
                &caps(Some(0x0080)),
            ),
            L2capPlan::Accept
        );
    }

    #[test]
    fn the_esp32_stays_on_the_gatt_floor_with_windows_and_apple() {
        assert_eq!(
            arrangement(esp32(), Endpoint::WinRt(WinRtHost::Windows)),
            Arrangement::GattOnly
        );
        assert_eq!(arrangement(esp32(), mac()), Arrangement::GattOnly);
    }

    #[test]
    fn an_untested_pair_falls_to_the_gatt_floor() {
        assert_eq!(arrangement(mac(), mac()), Arrangement::GattOnly);
        assert_eq!(arrangement(android(), android()), Arrangement::GattOnly);
        assert_eq!(
            arrangement(mac(), Endpoint::WinRt(WinRtHost::Windows)),
            Arrangement::GattOnly
        );
    }

    #[test]
    fn the_arrangement_table_is_order_independent() {
        let endpoints = [
            mac(),
            linux(),
            android(),
            Endpoint::CoreBluetooth(AppleHost::Ios),
            Endpoint::Esp32(Esp32Host::Esp32),
            Endpoint::WinRt(WinRtHost::Windows),
        ];
        for &a in &endpoints {
            for &b in &endpoints {
                assert_eq!(arrangement(a, b), arrangement(b, a));
            }
        }
    }

    #[test]
    fn opens_always_names_one_of_the_pair() {
        let endpoints = [
            mac(),
            linux(),
            android(),
            Endpoint::CoreBluetooth(AppleHost::Ios),
            Endpoint::Esp32(Esp32Host::Esp32),
            Endpoint::WinRt(WinRtHost::Windows),
        ];
        for &a in &endpoints {
            for &b in &endpoints {
                if let Arrangement::Opens(opener) = arrangement(a, b) {
                    assert!(opener == a || opener == b);
                }
            }
        }
    }

    #[test]
    fn both_ends_keep_the_same_connection_for_an_opens_pair() {
        let arr = arrangement(mac(), android());
        let mac_id = identity(1);
        let android_id = identity(2);

        let mac_dials_mac_view = is_keeper(arr, HandshakeRole::Dialer, mac_id, mac(), android_id);
        let mac_dials_android_view =
            is_keeper(arr, HandshakeRole::Listener, android_id, android(), mac_id);
        assert_eq!(mac_dials_mac_view, mac_dials_android_view);
        assert!(!mac_dials_mac_view);

        let android_dials_mac_view =
            is_keeper(arr, HandshakeRole::Listener, mac_id, mac(), android_id);
        let android_dials_android_view =
            is_keeper(arr, HandshakeRole::Dialer, android_id, android(), mac_id);
        assert_eq!(android_dials_mac_view, android_dials_android_view);
        assert!(android_dials_mac_view);
    }

    #[test]
    fn both_ends_keep_the_same_connection_for_an_either_opens_pair() {
        let arr = Arrangement::EitherOpens;
        let low = identity(1);
        let high = identity(9);

        let low_dials_low_view = is_keeper(arr, HandshakeRole::Dialer, low, mac(), high);
        let low_dials_high_view = is_keeper(arr, HandshakeRole::Listener, high, linux(), low);
        assert_eq!(low_dials_low_view, low_dials_high_view);
        assert!(low_dials_low_view);

        let high_dials_low_view = is_keeper(arr, HandshakeRole::Listener, low, mac(), high);
        let high_dials_high_view = is_keeper(arr, HandshakeRole::Dialer, high, linux(), low);
        assert_eq!(high_dials_low_view, high_dials_high_view);
        assert!(!high_dials_low_view);
    }

    #[test]
    fn only_the_designated_opener_stuck_as_peripheral_redials() {
        let opens_android = arrangement(mac(), android());
        assert!(needs_redial(
            opens_android,
            HandshakeRole::Listener,
            android()
        ));
        assert!(!needs_redial(
            opens_android,
            HandshakeRole::Dialer,
            android()
        ));
        assert!(!needs_redial(opens_android, HandshakeRole::Listener, mac()));
        assert!(!needs_redial(opens_android, HandshakeRole::Dialer, mac()));

        let either = arrangement(mac(), linux());
        assert!(!needs_redial(either, HandshakeRole::Listener, mac()));
        assert!(!needs_redial(either, HandshakeRole::Dialer, mac()));
    }

    #[test]
    fn either_opens_central_opens_and_peripheral_accepts() {
        let arr = Arrangement::EitherOpens;
        let mine = caps(Some(0x00c0));
        let theirs = caps(Some(0x0083));
        assert_eq!(
            l2cap_plan(arr, HandshakeRole::Dialer, mac(), &mine, &theirs),
            L2capPlan::Open {
                psm: Psm::new(0x0083).unwrap()
            }
        );
        assert_eq!(
            l2cap_plan(arr, HandshakeRole::Listener, mac(), &mine, &theirs),
            L2capPlan::Accept
        );
    }

    #[test]
    fn opens_lets_only_the_named_side_open_and_only_as_central() {
        let arr = arrangement(mac(), android());
        let android_caps = caps(Some(0x0080));
        let mac_caps = caps(Some(0x00c0));

        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Dialer,
                android(),
                &android_caps,
                &mac_caps
            ),
            L2capPlan::Open {
                psm: Psm::new(0x00c0).unwrap()
            }
        );
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Listener,
                android(),
                &android_caps,
                &mac_caps
            ),
            L2capPlan::None
        );
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Listener,
                mac(),
                &mac_caps,
                &android_caps
            ),
            L2capPlan::Accept
        );
        assert_eq!(
            l2cap_plan(arr, HandshakeRole::Dialer, mac(), &mac_caps, &android_caps),
            L2capPlan::Accept
        );
    }

    #[test]
    fn apple_mobile_and_bluez_never_plan_l2cap() {
        let apple_caps = caps(Some(0x00c0));
        let linux_caps = caps(Some(0x0083));

        assert_eq!(
            l2cap_plan(
                arrangement(ios(), linux()),
                HandshakeRole::Dialer,
                ios(),
                &apple_caps,
                &linux_caps,
            ),
            L2capPlan::None
        );
        assert_eq!(
            l2cap_plan(
                arrangement(linux(), ios()),
                HandshakeRole::Listener,
                linux(),
                &linux_caps,
                &apple_caps,
            ),
            L2capPlan::None
        );
        assert_eq!(
            l2cap_plan(
                arrangement(ipad(), linux()),
                HandshakeRole::Dialer,
                ipad(),
                &apple_caps,
                &linux_caps,
            ),
            L2capPlan::None
        );
    }

    #[test]
    fn ios_keeps_android_on_gatt_when_it_withholds_l2cap() {
        let ios_caps = caps(None);
        let android_caps = caps(Some(0x0080));

        assert_eq!(
            l2cap_plan(
                arrangement(ios(), android()),
                HandshakeRole::Dialer,
                ios(),
                &ios_caps,
                &android_caps,
            ),
            L2capPlan::None
        );
    }

    #[test]
    fn the_acceptor_stands_down_when_the_peer_has_no_listener() {
        let arr = arrangement(mac(), android());
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Listener,
                mac(),
                &caps(Some(0x00c0)),
                &caps(None),
            ),
            L2capPlan::None
        );
    }

    #[test]
    fn gatt_only_never_plans_l2cap() {
        assert_eq!(
            l2cap_plan(
                Arrangement::GattOnly,
                HandshakeRole::Dialer,
                mac(),
                &caps(Some(0x00c0)),
                &caps(Some(0x0080))
            ),
            L2capPlan::None
        );
    }

    #[test]
    fn an_opener_whose_peer_has_no_listener_cannot_open() {
        let arr = Arrangement::EitherOpens;
        assert_eq!(
            l2cap_plan(
                arr,
                HandshakeRole::Dialer,
                mac(),
                &caps(Some(0x00c0)),
                &caps(None)
            ),
            L2capPlan::None
        );
    }

    #[test]
    fn a_dialer_and_listener_settle_exchanging_endpoints_and_caps() {
        let dialer_local = Local {
            identity: identity(1),
            endpoint: mac(),
            capabilities: caps(Some(0x00c0)),
        };
        let listener_local = Local {
            identity: identity(2),
            endpoint: android(),
            capabilities: caps(Some(0x0080)),
        };
        let (mut dialer, opening) =
            Handshake::begin(HandshakeRole::Dialer, dialer_local, Some(-40));
        let (mut listener, silent) =
            Handshake::begin(HandshakeRole::Listener, listener_local, Some(-55));
        assert!(silent.is_none());

        let listener_reaction = listener.absorb(opening.unwrap());
        let dialer_reaction = dialer.absorb(listener_reaction.reply.unwrap());

        if let (Outcome::Settled(at_listener), Outcome::Settled(at_dialer)) =
            (listener_reaction.outcome, dialer_reaction.outcome)
        {
            assert_eq!(at_listener.identity, identity(1));
            assert_eq!(at_listener.endpoint, mac());
            assert_eq!(at_listener.peer_rssi, Some(-40));
            assert_eq!(at_dialer.identity, identity(2));
            assert_eq!(at_dialer.endpoint, android());
            assert_eq!(at_dialer.peer_rssi, Some(-55));
        } else {
            panic!("expected both sides to settle");
        }
    }

    #[test]
    fn a_self_connection_aborts_and_closes() {
        let local = Local {
            identity: identity(5),
            endpoint: mac(),
            capabilities: caps(Some(0x0090)),
        };
        let (mut listener, _) = Handshake::begin(HandshakeRole::Listener, local, None);
        let reaction = listener.absorb(Control::Hello {
            identity: identity(5),
            endpoint: mac(),
            capabilities: caps(Some(0x0090)),
            peer_rssi: None,
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
            endpoint: android(),
            capabilities: caps(Some(0x0081)),
            peer_rssi: Some(-63),
        };
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = hello.encode(&mut buf).unwrap();
        assert_eq!(Control::decode(&buf[..len]), Some(hello));
    }

    #[test]
    fn every_endpoint_round_trips_through_the_greeting() {
        for endpoint in [
            mac(),
            linux(),
            android(),
            Endpoint::CoreBluetooth(AppleHost::Ios),
            Endpoint::CoreBluetooth(AppleHost::IpadOs),
            Endpoint::WinRt(WinRtHost::Windows),
            Endpoint::Esp32(Esp32Host::Esp32),
        ] {
            let hello = Control::Hello {
                identity: identity(3),
                endpoint,
                capabilities: caps(None),
                peer_rssi: None,
            };
            let mut buf = [0u8; CONTROL_MAX_LEN];
            let len = hello.encode(&mut buf).unwrap();
            match Control::decode(&buf[..len]) {
                Some(Control::Hello {
                    endpoint: decoded, ..
                }) => assert_eq!(decoded, endpoint),
                other => panic!("endpoint failed to round-trip: {other:?}"),
            }
        }
    }

    #[test]
    fn a_greeting_without_the_trailing_rssi_byte_still_decodes() {
        let hello = Control::Hello {
            identity: identity(7),
            endpoint: mac(),
            capabilities: caps(Some(0x0081)),
            peer_rssi: Some(-63),
        };
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = hello.encode(&mut buf).unwrap();
        let trimmed = Control::decode(&buf[..len - 1]).unwrap();
        assert_eq!(
            trimmed,
            Control::Hello {
                identity: identity(7),
                endpoint: mac(),
                capabilities: caps(Some(0x0081)),
                peer_rssi: None,
            }
        );
    }

    #[test]
    fn a_gatt_only_welcome_round_trips_with_no_psm() {
        let welcome = Control::Welcome {
            identity: identity(9),
            endpoint: linux(),
            capabilities: LinkCapabilities {
                l2cap: None,
                link_mtu: 23,
            },
            peer_rssi: None,
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
            endpoint: mac(),
            capabilities: caps(Some(0x0090)),
            peer_rssi: None,
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
