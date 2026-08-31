use super::identity::{default_group_tag, BleIdentity, GROUP_TAG_LEN};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerProtocol {
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

pub(super) const CONTROL_HELLO: u8 = 0x01;
const CONTROL_WELCOME: u8 = 0x02;
pub(super) const CONTROL_CLOSE: u8 = 0x03;
const CONTROL_IDENTITY_LEN: usize = 16;
const ENDPOINT_LEN: usize = 2;
const CONTROL_CAP_LEN: usize = 3;
const CONTROL_RSSI_LEN: usize = 1;
const GREETING_ID_AT: usize = 1;
const GREETING_ENDPOINT_AT: usize = GREETING_ID_AT + CONTROL_IDENTITY_LEN;
const GREETING_CAP_AT: usize = GREETING_ENDPOINT_AT + ENDPOINT_LEN;
const GREETING_RSSI_AT: usize = GREETING_CAP_AT + CONTROL_CAP_LEN;
const GREETING_GROUP_AT: usize = GREETING_RSSI_AT + CONTROL_RSSI_LEN;
/// Legacy Hello/Welcome length (no discovery group tag).
pub const CONTROL_LEGACY_GREETING_LEN: usize = GREETING_GROUP_AT;
/// Current Hello/Welcome length (includes optional discovery group tag).
pub const CONTROL_MAX_LEN: usize = GREETING_GROUP_AT + GROUP_TAG_LEN;
/// Body length after the control tag for identity+endpoint+caps (RSSI optional).
const GREETING_BODY_MIN_LEN: usize = CONTROL_IDENTITY_LEN + ENDPOINT_LEN + CONTROL_CAP_LEN;
/// Body length after the control tag for a legacy greeting (id+endpoint+caps+rssi).
const GREETING_BODY_LEGACY_LEN: usize = GREETING_BODY_MIN_LEN + CONTROL_RSSI_LEN;

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
pub enum L2capArrangement {
    GattOnly,
    EitherOpens,
    Opens(Endpoint),
}

pub fn l2cap_arrangement(local: Endpoint, peer: Endpoint) -> L2capArrangement {
    known_arrangement(local, peer)
        .or_else(|| known_arrangement(peer, local))
        .unwrap_or(L2capArrangement::GattOnly)
}

fn known_arrangement(a: Endpoint, b: Endpoint) -> Option<L2capArrangement> {
    use AppleHost::{Ios, IpadOs, MacOs};
    use Endpoint::{Android, BlueZ, CoreBluetooth, Esp32, Nrf52};
    match (a, b) {
        (CoreBluetooth(MacOs), BlueZ(host)) => Some(L2capArrangement::Opens(BlueZ(host))),
        (CoreBluetooth(MacOs), Android(host)) => Some(L2capArrangement::Opens(Android(host))),
        (CoreBluetooth(Ios | IpadOs), Android(_)) => Some(L2capArrangement::Opens(a)),
        (BlueZ(_), Android(_)) => Some(L2capArrangement::EitherOpens),
        (BlueZ(_), Nrf52(_)) => Some(L2capArrangement::EitherOpens),
        (Android(_), Nrf52(_)) => Some(L2capArrangement::EitherOpens),
        (Esp32(_), Esp32(_)) => Some(L2capArrangement::EitherOpens),
        (Esp32(_), Nrf52(_)) => Some(L2capArrangement::EitherOpens),
        (BlueZ(_), Esp32(_)) => Some(L2capArrangement::EitherOpens),
        (Android(_), Esp32(_)) => Some(L2capArrangement::EitherOpens),
        _ => None,
    }
}

pub fn we_should_be_central(
    l2cap_arrangement: L2capArrangement,
    ours: BleIdentity,
    our_endpoint: Endpoint,
    theirs: BleIdentity,
) -> bool {
    match l2cap_arrangement {
        L2capArrangement::Opens(opener) => opener == our_endpoint,
        L2capArrangement::GattOnly | L2capArrangement::EitherOpens => ours < theirs,
    }
}

pub fn is_keeper(
    l2cap_arrangement: L2capArrangement,
    our_role: HandshakeRole,
    ours: BleIdentity,
    our_endpoint: Endpoint,
    theirs: BleIdentity,
) -> bool {
    matches!(our_role, HandshakeRole::Dialer)
        == we_should_be_central(l2cap_arrangement, ours, our_endpoint, theirs)
}

/// True when this handshake landed in the wrong GATT role for an `Opens(E)` arrangement.
///
/// The designated opener must be GATT central to open CoC. Either side may dial for discovery,
/// but a settle in the wrong role must drop so the opener can dial (or re-dial) as central:
/// - opener stuck as listener/peripheral → drop and dial
/// - non-opener who dialed (wrong central) → drop and wait for the opener's dial
pub fn needs_redial(
    l2cap_arrangement: L2capArrangement,
    our_role: HandshakeRole,
    our_endpoint: Endpoint,
) -> bool {
    match l2cap_arrangement {
        L2capArrangement::Opens(opener) => {
            let we_open = opener == our_endpoint;
            (we_open && matches!(our_role, HandshakeRole::Listener))
                || (!we_open && matches!(our_role, HandshakeRole::Dialer))
        }
        L2capArrangement::GattOnly | L2capArrangement::EitherOpens => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2capPlan {
    Open { psm: Psm },
    Accept,
    None,
}

pub fn l2cap_plan(
    l2cap_arrangement: L2capArrangement,
    our_role: HandshakeRole,
    our_endpoint: Endpoint,
    our_capabilities: &LinkCapabilities,
    peer_capabilities: &LinkCapabilities,
) -> L2capPlan {
    let we_are_central = matches!(our_role, HandshakeRole::Dialer);
    let we_open = match l2cap_arrangement {
        L2capArrangement::GattOnly => return L2capPlan::None,
        L2capArrangement::EitherOpens => we_are_central,
        L2capArrangement::Opens(opener) => opener == our_endpoint,
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
        /// Absent on legacy wire; treated as the default reticulum group when matching.
        group_tag: Option<[u8; GROUP_TAG_LEN]>,
    },
    Welcome {
        identity: BleIdentity,
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
        peer_rssi: Option<i8>,
        /// Absent on legacy wire; treated as the default reticulum group when matching.
        group_tag: Option<[u8; GROUP_TAG_LEN]>,
    },
    Close {
        reason: CloseReason,
    },
}

/// Resolve a handshake/advertisement discovery group: missing → default reticulum tag.
#[must_use]
pub fn resolved_discovery_group(tag: Option<[u8; GROUP_TAG_LEN]>) -> [u8; GROUP_TAG_LEN] {
    tag.unwrap_or_else(default_group_tag)
}

impl Control {
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        match self {
            Control::Hello {
                identity,
                endpoint,
                capabilities,
                peer_rssi,
                group_tag,
            } => encode_greeting(
                CONTROL_HELLO,
                identity,
                *endpoint,
                capabilities,
                *peer_rssi,
                *group_tag,
                out,
            ),
            Control::Welcome {
                identity,
                endpoint,
                capabilities,
                peer_rssi,
                group_tag,
            } => encode_greeting(
                CONTROL_WELCOME,
                identity,
                *endpoint,
                capabilities,
                *peer_rssi,
                *group_tag,
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
                let (identity, endpoint, capabilities, peer_rssi, group_tag) =
                    decode_greeting(body)?;
                Some(Control::Hello {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi,
                    group_tag,
                })
            }
            CONTROL_WELCOME => {
                let (identity, endpoint, capabilities, peer_rssi, group_tag) =
                    decode_greeting(body)?;
                Some(Control::Welcome {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi,
                    group_tag,
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
    group_tag: Option<[u8; GROUP_TAG_LEN]>,
    out: &mut [u8],
) -> Option<usize> {
    let len = if group_tag.is_some() {
        CONTROL_MAX_LEN
    } else {
        CONTROL_LEGACY_GREETING_LEN
    };
    let slot = out.get_mut(..len)?;
    slot[0] = tag;
    slot[GREETING_ID_AT..GREETING_ENDPOINT_AT].copy_from_slice(identity.as_bytes());
    slot[GREETING_ENDPOINT_AT..GREETING_CAP_AT].copy_from_slice(&endpoint_bytes(endpoint));
    let mut caps = [0u8; CONTROL_CAP_LEN];
    capabilities.encode(&mut caps);
    slot[GREETING_CAP_AT..GREETING_RSSI_AT].copy_from_slice(&caps);
    slot[GREETING_RSSI_AT] = encode_rssi(peer_rssi);
    if let Some(group) = group_tag {
        slot[GREETING_GROUP_AT..CONTROL_MAX_LEN].copy_from_slice(&group);
    }
    Some(len)
}

type DecodedGreeting = (
    BleIdentity,
    Endpoint,
    LinkCapabilities,
    Option<i8>,
    Option<[u8; GROUP_TAG_LEN]>,
);

fn decode_greeting(body: &[u8]) -> Option<DecodedGreeting> {
    if body.len() < GREETING_BODY_MIN_LEN {
        return None;
    }
    let id_end = CONTROL_IDENTITY_LEN;
    let endpoint_end = id_end + ENDPOINT_LEN;
    let cap_end = endpoint_end + CONTROL_CAP_LEN;
    let identity_bytes: [u8; CONTROL_IDENTITY_LEN] = body.get(..id_end)?.try_into().ok()?;
    let endpoint = decode_endpoint(body.get(id_end..endpoint_end)?)?;
    let capabilities = LinkCapabilities::decode(body.get(endpoint_end..cap_end)?)?;
    let peer_rssi = body.get(cap_end).copied().and_then(decode_rssi);
    let group_tag = body
        .get(GREETING_BODY_LEGACY_LEN..GREETING_BODY_LEGACY_LEN + GROUP_TAG_LEN)
        .and_then(|bytes| <[u8; GROUP_TAG_LEN]>::try_from(bytes).ok());
    Some((
        BleIdentity::new(identity_bytes),
        endpoint,
        capabilities,
        peer_rssi,
        group_tag,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalPeer {
    pub identity: BleIdentity,
    pub endpoint: Endpoint,
    pub capabilities: LinkCapabilities,
    pub group_tag: [u8; GROUP_TAG_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstablishedPeer {
    pub identity: BleIdentity,
    pub transport: EstablishedTransport,
    pub peer_rssi: Option<i8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishedTransport {
    Native {
        endpoint: Endpoint,
        capabilities: LinkCapabilities,
    },
    ColumbaGatt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeOutcome {
    Pending,
    Settled(EstablishedPeer),
    Aborted(CloseReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeReaction {
    pub reply: Option<Control>,
    pub outcome: HandshakeOutcome,
}

pub struct Handshake {
    role: HandshakeRole,
    local: LocalPeer,
    measured_rssi: Option<i8>,
}

impl Handshake {
    pub fn begin(
        role: HandshakeRole,
        local: LocalPeer,
        measured_rssi: Option<i8>,
    ) -> (Self, Option<Control>) {
        let opening = match role {
            HandshakeRole::Dialer => Some(Control::Hello {
                identity: local.identity,
                endpoint: local.endpoint,
                capabilities: local.capabilities,
                peer_rssi: measured_rssi,
                group_tag: Some(local.group_tag),
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

    pub fn absorb(&mut self, msg: Control) -> HandshakeReaction {
        match (self.role, msg) {
            (
                HandshakeRole::Listener,
                Control::Hello {
                    identity,
                    endpoint,
                    capabilities,
                    peer_rssi,
                    group_tag,
                },
            ) => {
                if identity == self.local.identity {
                    return self.we_close(CloseReason::SelfConnection);
                }
                if !self.discovery_group_matches(group_tag) {
                    return self.we_close(CloseReason::Incompatible);
                }
                HandshakeReaction {
                    reply: Some(Control::Welcome {
                        identity: self.local.identity,
                        endpoint: self.local.endpoint,
                        capabilities: self.local.capabilities,
                        peer_rssi: self.measured_rssi,
                        group_tag: Some(self.local.group_tag),
                    }),
                    outcome: HandshakeOutcome::Settled(EstablishedPeer {
                        identity,
                        transport: EstablishedTransport::Native {
                            endpoint,
                            capabilities,
                        },
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
                    group_tag,
                },
            ) => {
                if identity == self.local.identity {
                    return self.we_close(CloseReason::SelfConnection);
                }
                if !self.discovery_group_matches(group_tag) {
                    return self.we_close(CloseReason::Incompatible);
                }
                HandshakeReaction {
                    reply: None,
                    outcome: HandshakeOutcome::Settled(EstablishedPeer {
                        identity,
                        transport: EstablishedTransport::Native {
                            endpoint,
                            capabilities,
                        },
                        peer_rssi,
                    }),
                }
            }
            (_, Control::Close { reason }) => HandshakeReaction {
                reply: None,
                outcome: HandshakeOutcome::Aborted(reason),
            },
            _ => self.we_close(CloseReason::Incompatible),
        }
    }

    fn discovery_group_matches(&self, peer_tag: Option<[u8; GROUP_TAG_LEN]>) -> bool {
        resolved_discovery_group(peer_tag) == self.local.group_tag
    }

    fn we_close(&self, reason: CloseReason) -> HandshakeReaction {
        HandshakeReaction {
            reply: Some(Control::Close { reason }),
            outcome: HandshakeOutcome::Aborted(reason),
        }
    }
}
