use crate::crypto::{
    ed25519_public_key, ed25519_sign, ed25519_verify, sha256, Ed25519PublicKey, Ed25519SecretKey,
    Ed25519Signature,
};
use crate::interfaces::rns_serial_framing;

use super::core::{MULTIPATH_DEDUPLICATION_CAPACITY, MULTIPATH_DEDUPLICATION_MILLIS};

pub const TYPE_DISCOVER: u8 = 0x00;
pub const TYPE_CONNECT: u8 = 0x01;
pub const TYPE_COMMAND: u8 = 0x02;
pub const TYPE_LOG: u8 = 0x03;
pub const TYPE_DISPLAY: u8 = 0x04;
pub const TYPE_ENDPOINT_PACKET: u8 = 0x05;
pub const TYPE_ENCAPSULATED_PROTOCOL: u8 = 0x06;

pub const COMMAND_ENDPOINT_PACKET: u16 = 0x0001;

pub const EVENT_WDCL_CONNECTION: u16 = 0x3002;
pub const EVENT_WDCL_HOST_ENDPOINT: u16 = 0x3003;
pub const EVENT_WEAVE_ENDPOINT_ALIVE: u16 = 0x3102;
pub const EVENT_WEAVE_ENDPOINT_TIMEOUT: u16 = 0x3103;
pub const EVENT_WEAVE_ENDPOINT_VIA: u16 = 0x3104;

pub const BROADCAST_SWITCH: SwitchId = SwitchId::new([0xff; SwitchId::LEN]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SwitchId([u8; Self::LEN]);

impl SwitchId {
    pub const LEN: usize = 4;

    pub const fn new(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EndpointId([u8; Self::LEN]);

impl EndpointId {
    pub const LEN: usize = 8;

    pub const fn new(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

pub struct WeaveHostIdentity {
    secret: Ed25519SecretKey,
    public: Ed25519PublicKey,
    switch_id: SwitchId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SeenPacket {
    hash: [u8; 32],
    received_at_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipathDeduplicator {
    entries: [Option<SeenPacket>; MULTIPATH_DEDUPLICATION_CAPACITY],
    next: usize,
}

impl Default for MultipathDeduplicator {
    fn default() -> Self {
        Self::new()
    }
}

impl MultipathDeduplicator {
    pub const fn new() -> Self {
        Self {
            entries: [None; MULTIPATH_DEDUPLICATION_CAPACITY],
            next: 0,
        }
    }

    pub fn accepts(&mut self, packet: &[u8], received_at_millis: u64) -> bool {
        let hash = sha256(packet);
        if self.entries.iter().flatten().any(|entry| {
            entry.hash == hash
                && received_at_millis
                    < entry
                        .received_at_millis
                        .saturating_add(MULTIPATH_DEDUPLICATION_MILLIS)
        }) {
            return false;
        }
        self.entries[self.next] = Some(SeenPacket {
            hash,
            received_at_millis,
        });
        self.next = (self.next + 1) % MULTIPATH_DEDUPLICATION_CAPACITY;
        true
    }
}

impl WeaveHostIdentity {
    pub fn from_signing_secret(secret: [u8; Ed25519SecretKey::LEN]) -> Self {
        let secret = Ed25519SecretKey::new(secret);
        let public = ed25519_public_key(&secret);
        let switch_id = switch_id_for_public_key(public);
        Self {
            secret,
            public,
            switch_id,
        }
    }

    pub const fn switch_id(&self) -> SwitchId {
        self.switch_id
    }

    pub const fn signing_public_key(&self) -> Ed25519PublicKey {
        self.public
    }

    fn sign(&self, message: &[u8]) -> Ed25519Signature {
        ed25519_sign(&self.secret, message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEvent<'a> {
    Discovered {
        switch_id: SwitchId,
        signing_public_key: Ed25519PublicKey,
    },
    Connected,
    HostEndpoint(EndpointId),
    EndpointAlive(EndpointId),
    EndpointTimedOut(EndpointId),
    EndpointVia {
        endpoint: EndpointId,
        switch_id: SwitchId,
    },
    EndpointPacket {
        source: EndpointId,
        payload: &'a [u8],
    },
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    InvalidDiscoverySignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    RawFrameTooSmall,
    FramedOutputTooSmall,
}

pub fn decode_device_frame(
    frame: &[u8],
    local_switch_id: SwitchId,
) -> Result<DeviceEvent<'_>, DecodeError> {
    if frame.len() <= SwitchId::LEN {
        return Ok(DeviceEvent::Ignored);
    }
    match frame[SwitchId::LEN] {
        TYPE_DISCOVER => decode_discovery(frame),
        TYPE_ENDPOINT_PACKET if frame[..SwitchId::LEN] == local_switch_id.0 => {
            Ok(decode_endpoint_packet(frame))
        }
        TYPE_ENDPOINT_PACKET => Ok(DeviceEvent::Ignored),
        TYPE_LOG => Ok(decode_log_event(frame)),
        TYPE_CONNECT | TYPE_COMMAND | TYPE_DISPLAY | TYPE_ENCAPSULATED_PROTOCOL => {
            Ok(DeviceEvent::Ignored)
        }
        _ => Ok(DeviceEvent::Ignored),
    }
}

pub fn encode_discovery(
    identity: &WeaveHostIdentity,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let mut raw = [0u8; SwitchId::LEN + 1 + SwitchId::LEN];
    encode_frame(
        BROADCAST_SWITCH,
        TYPE_DISCOVER,
        identity.switch_id().as_bytes(),
        &mut raw,
        output,
    )
}

pub fn encode_handshake(
    identity: &WeaveHostIdentity,
    remote: SwitchId,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let signature = identity.sign(remote.as_bytes());
    let mut payload = [0u8; Ed25519PublicKey::LEN + Ed25519Signature::LEN];
    payload[..Ed25519PublicKey::LEN].copy_from_slice(&identity.signing_public_key().0);
    payload[Ed25519PublicKey::LEN..].copy_from_slice(&signature.0);
    let mut raw = [0u8; SwitchId::LEN + 1 + Ed25519PublicKey::LEN + Ed25519Signature::LEN];
    encode_frame(remote, TYPE_CONNECT, &payload, &mut raw, output)
}

pub fn encode_discovery_response(
    device_identity: &WeaveHostIdentity,
    host_switch_id: SwitchId,
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let signature = device_identity.sign(host_switch_id.as_bytes());
    let mut payload = [0u8; Ed25519PublicKey::LEN + Ed25519Signature::LEN];
    payload[..Ed25519PublicKey::LEN].copy_from_slice(&device_identity.signing_public_key().0);
    payload[Ed25519PublicKey::LEN..].copy_from_slice(&signature.0);
    let mut raw = [0u8; SwitchId::LEN + 1 + Ed25519PublicKey::LEN + Ed25519Signature::LEN];
    encode_frame(host_switch_id, TYPE_DISCOVER, &payload, &mut raw, output)
}

pub fn encode_endpoint_packet(
    remote: SwitchId,
    endpoint: EndpointId,
    payload: &[u8],
    raw: &mut [u8],
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let required = SwitchId::LEN + 1 + 2 + EndpointId::LEN + payload.len();
    if raw.len() < required {
        return Err(EncodeError::RawFrameTooSmall);
    }
    raw[..SwitchId::LEN].copy_from_slice(remote.as_bytes());
    raw[SwitchId::LEN] = TYPE_COMMAND;
    raw[SwitchId::LEN + 1..SwitchId::LEN + 3]
        .copy_from_slice(&COMMAND_ENDPOINT_PACKET.to_be_bytes());
    raw[SwitchId::LEN + 3..SwitchId::LEN + 3 + EndpointId::LEN]
        .copy_from_slice(endpoint.as_bytes());
    raw[SwitchId::LEN + 3 + EndpointId::LEN..required].copy_from_slice(payload);
    rns_serial_framing::encode(&raw[..required], output)
        .map_err(|_| EncodeError::FramedOutputTooSmall)
}

fn encode_frame(
    remote: SwitchId,
    frame_type: u8,
    payload: &[u8],
    raw: &mut [u8],
    output: &mut [u8],
) -> Result<usize, EncodeError> {
    let required = SwitchId::LEN + 1 + payload.len();
    if raw.len() < required {
        return Err(EncodeError::RawFrameTooSmall);
    }
    raw[..SwitchId::LEN].copy_from_slice(remote.as_bytes());
    raw[SwitchId::LEN] = frame_type;
    raw[SwitchId::LEN + 1..required].copy_from_slice(payload);
    rns_serial_framing::encode(&raw[..required], output)
        .map_err(|_| EncodeError::FramedOutputTooSmall)
}

fn decode_discovery(frame: &[u8]) -> Result<DeviceEvent<'_>, DecodeError> {
    const RESPONSE_LEN: usize = SwitchId::LEN + 1 + Ed25519PublicKey::LEN + Ed25519Signature::LEN;
    if frame.len() != RESPONSE_LEN {
        return Ok(DeviceEvent::Ignored);
    }
    let mut public = [0u8; Ed25519PublicKey::LEN];
    public.copy_from_slice(&frame[SwitchId::LEN + 1..SwitchId::LEN + 1 + Ed25519PublicKey::LEN]);
    let public = Ed25519PublicKey(public);
    let mut signature = [0u8; Ed25519Signature::LEN];
    signature.copy_from_slice(&frame[SwitchId::LEN + 1 + Ed25519PublicKey::LEN..]);
    ed25519_verify(
        &public,
        &frame[..SwitchId::LEN],
        &Ed25519Signature(signature),
    )
    .map_err(|_| DecodeError::InvalidDiscoverySignature)?;
    Ok(DeviceEvent::Discovered {
        switch_id: switch_id_for_public_key(public),
        signing_public_key: public,
    })
}

fn decode_endpoint_packet(frame: &[u8]) -> DeviceEvent<'_> {
    let payload_start = SwitchId::LEN + 1;
    if frame.len() <= payload_start + EndpointId::LEN {
        return DeviceEvent::Ignored;
    }
    let source_start = frame.len() - EndpointId::LEN;
    let mut source = [0u8; EndpointId::LEN];
    source.copy_from_slice(&frame[source_start..]);
    DeviceEvent::EndpointPacket {
        source: EndpointId::new(source),
        payload: &frame[payload_start..source_start],
    }
}

fn decode_log_event(frame: &[u8]) -> DeviceEvent<'_> {
    const LOG_PREFIX_LEN: usize = SwitchId::LEN + 1 + 1;
    const EVENT_OFFSET: usize = 6;
    const EVENT_DATA_OFFSET: usize = 8;
    if frame.len() < LOG_PREFIX_LEN + EVENT_DATA_OFFSET {
        return DeviceEvent::Ignored;
    }
    let log = &frame[LOG_PREFIX_LEN..];
    let event = u16::from_be_bytes([log[EVENT_OFFSET], log[EVENT_OFFSET + 1]]);
    let data = &log[EVENT_DATA_OFFSET..];
    match event {
        EVENT_WDCL_CONNECTION => DeviceEvent::Connected,
        EVENT_WDCL_HOST_ENDPOINT => endpoint_event(data, DeviceEvent::HostEndpoint),
        EVENT_WEAVE_ENDPOINT_ALIVE => endpoint_event(data, DeviceEvent::EndpointAlive),
        EVENT_WEAVE_ENDPOINT_TIMEOUT => endpoint_event(data, DeviceEvent::EndpointTimedOut),
        EVENT_WEAVE_ENDPOINT_VIA if data.len() == EndpointId::LEN + SwitchId::LEN => {
            let mut endpoint = [0u8; EndpointId::LEN];
            endpoint.copy_from_slice(&data[..EndpointId::LEN]);
            let mut switch_id = [0u8; SwitchId::LEN];
            switch_id.copy_from_slice(&data[EndpointId::LEN..]);
            DeviceEvent::EndpointVia {
                endpoint: EndpointId::new(endpoint),
                switch_id: SwitchId::new(switch_id),
            }
        }
        _ => DeviceEvent::Ignored,
    }
}

fn endpoint_event<'a>(
    data: &[u8],
    event: impl FnOnce(EndpointId) -> DeviceEvent<'a>,
) -> DeviceEvent<'a> {
    if data.len() != EndpointId::LEN {
        return DeviceEvent::Ignored;
    }
    let mut endpoint = [0u8; EndpointId::LEN];
    endpoint.copy_from_slice(data);
    event(EndpointId::new(endpoint))
}

fn switch_id_for_public_key(public: Ed25519PublicKey) -> SwitchId {
    let mut switch_id = [0u8; SwitchId::LEN];
    switch_id.copy_from_slice(&public.0[Ed25519PublicKey::LEN - SwitchId::LEN..]);
    SwitchId::new(switch_id)
}
