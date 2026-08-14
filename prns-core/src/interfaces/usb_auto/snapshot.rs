//! Headless config snapshot body codec. See `T1000E_HEADLESS_CONFIG.md`.
//!
//! The webUI (`prns.dev/configure`) renders a headless Hopspot's state from a
//! `Message::Snapshot` whose body this module encodes and decodes. The body is
//! a sectioned, versioned binary: a section count followed by length-prefixed
//! typed sections. Each section is a [`SnapshotSection`]; unknown section tags
//! are skipped on decode so future schema additions stay forward-compatible
//! without a version bump, and a host that speaks an older schema still renders
//! every section it recognizes.
//!
//! The schema version lives in the enclosing `Message::Snapshot` envelope
//! ([`super::config::SNAPSHOT_SCHEMA_VERSION`]); this module owns the body
//! layout for schema v1. The authoritative home for every wire code below is
//! this module — the device config task assembles a [`SnapshotBody`] from
//! runtime state and encodes it; the browser (via prns-wasm) decodes it.
//!
//! Layering: the codec depends only on prns-core types ([`RadioProfile`],
//! [`ConnectionState`], [`ConfigInterface`]) and plain scalars, so it is
//! shared verbatim by the no_std embassy device and the wasm host. Cross-layer
//! state that lives higher (e.g. the persistence state machine in
//! `personal-hopspot-core`) is adapted into a snapshot-local representation
//! here; the wire code is a config-lane concept, owned here, not a duplication
//! of the higher-layer state machine.

use heapless::String as HeaplessString;
use heapless::Vec as HeaplessVec;

use crate::interfaces::lora::RadioProfile;
use crate::interfaces::status::{AirtimeUtilization, ConnectionState, TransferRates};

use super::config::ConfigInterface;
use super::protocol::MAX_SNAPSHOT_BODY_BYTES;

/// Maximum number of sections carried in one snapshot body. v1 defines nine;
/// the headroom lets later schemas add sections before raising
/// [`super::config::SNAPSHOT_SCHEMA_VERSION`].
pub const MAX_SNAPSHOT_SECTIONS: usize = 16;

/// Largest firmware-version string the codec accepts. Bounds the decoded
/// [`SnapshotSection::DeviceInfo`] without a heap allocation.
pub const MAX_FIRMWARE_VERSION_BYTES: usize = 32;

/// Largest interface failure-reason string the codec accepts.
pub const MAX_FAILURE_REASON_BYTES: usize = 64;

/// Maximum interfaces whose counts one [`SnapshotSection::InterfaceCounts`]
/// section can carry. v1 has three (LoRa, USB, BLE).
pub const MAX_INTERFACE_COUNTS: usize = 4;

// Section tags. Stable across releases; never reuse a retired tag.
const SECTION_DEVICE_INFO: u8 = 0x01;
const SECTION_PERSISTENCE: u8 = 0x02;
const SECTION_LORA_STATUS: u8 = 0x03;
const SECTION_USB_STATUS: u8 = 0x04;
const SECTION_BLE_STATUS: u8 = 0x05;
const SECTION_BLE_RECOVERY: u8 = 0x06;
const SECTION_LORA_SPECTRUM: u8 = 0x07;
const SECTION_RADIO_PROFILE: u8 = 0x08;
const SECTION_INTERFACE_COUNTS: u8 = 0x09;

/// Failure to encode a snapshot body into a fixed buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotEncodeError {
    /// The encoded body exceeds [`MAX_SNAPSHOT_BODY_BYTES`].
    BodyTooLarge,
    /// The output buffer is smaller than the encoded body.
    BufferTooSmall,
}

/// Failure to decode a snapshot body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotDecodeError {
    /// The body ended before a declared length was satisfied.
    Truncated,
    /// A section declared a length larger than the remaining body.
    OversizeSection,
    /// More known sections arrived than [`MAX_SNAPSHOT_SECTIONS`] can hold.
    TooManySections,
    /// A section body was not a valid instance of its declared tag.
    InvalidSection,
    /// A `RadioProfile` section failed [`RadioProfile::decode`].
    InvalidProfile,
}

/// The persistence state of the profile store, mirrored into the snapshot.
///
/// The authoritative state machine lives in `personal-hopspot-core`; this is
/// its wire representation on the config lane. The mapping is one-to-one and
/// performed by the device config task at assembly time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPersistence {
    Durable,
    Deferred,
    Failed,
}

impl SnapshotPersistence {
    const DURABLE: u8 = 0;
    const DEFERRED: u8 = 1;
    const FAILED: u8 = 2;

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Durable => Self::DURABLE,
            Self::Deferred => Self::DEFERRED,
            Self::Failed => Self::FAILED,
        }
    }

    pub const fn from_wire(byte: u8) -> Self {
        match byte {
            Self::DEFERRED => Self::Deferred,
            Self::FAILED => Self::Failed,
            _ => Self::Durable,
        }
    }
}

/// Per-interface runtime status shared by the LoRa, USB, and BLE sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceStatusBody {
    pub enabled: bool,
    pub connection: ConnectionState,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub airtime: Option<AirtimeUtilization>,
    pub transfer_rates: Option<TransferRates>,
}

impl InterfaceStatusBody {
    const FLAG_ENABLED: u8 = 1 << 0;
    const FLAG_AIRTIME: u8 = 1 << 1;
    const FLAG_RATES: u8 = 1 << 2;

    fn encode(&self, w: &mut Writer) -> Result<(), SnapshotEncodeError> {
        let mut flags = 0u8;
        if self.enabled {
            flags |= Self::FLAG_ENABLED;
        }
        if self.airtime.is_some() {
            flags |= Self::FLAG_AIRTIME;
        }
        if self.transfer_rates.is_some() {
            flags |= Self::FLAG_RATES;
        }
        w.push_u8(flags)?;
        w.push_u8(self.connection.as_u8())?;
        w.push_u64(self.rx_bytes)?;
        w.push_u64(self.tx_bytes)?;
        let airtime = self.airtime.unwrap_or(AirtimeUtilization {
            short_per_mille: 0,
            long_per_mille: 0,
        });
        w.push_u16(airtime.short_per_mille)?;
        w.push_u16(airtime.long_per_mille)?;
        let rates = self.transfer_rates.unwrap_or(TransferRates {
            rx_bps: 0,
            tx_bps: 0,
        });
        w.push_u32(rates.rx_bps)?;
        w.push_u32(rates.tx_bps)?;
        Ok(())
    }

    fn decode(r: &mut Reader) -> Result<Self, SnapshotDecodeError> {
        let flags = r.read_u8()?;
        let connection = ConnectionState::from_u8(r.read_u8()?);
        let rx_bytes = r.read_u64()?;
        let tx_bytes = r.read_u64()?;
        let airtime_short = r.read_u16()?;
        let airtime_long = r.read_u16()?;
        let rx_bps = r.read_u32()?;
        let tx_bps = r.read_u32()?;
        let airtime = (flags & Self::FLAG_AIRTIME != 0).then_some(AirtimeUtilization {
            short_per_mille: airtime_short,
            long_per_mille: airtime_long,
        });
        let transfer_rates =
            (flags & Self::FLAG_RATES != 0).then_some(TransferRates { rx_bps, tx_bps });
        Ok(Self {
            enabled: flags & Self::FLAG_ENABLED != 0,
            connection,
            rx_bytes,
            tx_bytes,
            airtime,
            transfer_rates,
        })
    }
}

/// BLE recovery counters and supervisor-peer breadth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BluetoothRecoveryBody {
    pub ingress_pressure: u32,
    pub setup_failures: u32,
    pub transport_closures: u32,
    pub egress_pressure_events: u32,
    pub member_count: u8,
}

impl BluetoothRecoveryBody {
    fn encode(&self, w: &mut Writer) -> Result<(), SnapshotEncodeError> {
        w.push_u32(self.ingress_pressure)?;
        w.push_u32(self.setup_failures)?;
        w.push_u32(self.transport_closures)?;
        w.push_u32(self.egress_pressure_events)?;
        w.push_u8(self.member_count)?;
        Ok(())
    }

    fn decode(r: &mut Reader) -> Result<Self, SnapshotDecodeError> {
        Ok(Self {
            ingress_pressure: r.read_u32()?,
            setup_failures: r.read_u32()?,
            transport_closures: r.read_u32()?,
            egress_pressure_events: r.read_u32()?,
            member_count: r.read_u8()?,
        })
    }
}

/// LoRa spectrum menu details, read from `LoRaSpectrumStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoRaSpectrumBody {
    pub channel_busy_per_mille: u16,
    pub noise_floor_dbm: Option<i16>,
    pub cca_threshold_dbm: Option<i16>,
    pub deferrals: u32,
    pub false_preambles: u32,
    pub contention_timeouts: u32,
    pub duty_holds: u32,
    pub duty_timeouts: u32,
    pub radio_recoveries: u32,
}

impl LoRaSpectrumBody {
    const FLAG_NOISE: u8 = 1 << 0;
    const FLAG_CCA: u8 = 1 << 1;

    fn encode(&self, w: &mut Writer) -> Result<(), SnapshotEncodeError> {
        let mut flags = 0u8;
        if self.noise_floor_dbm.is_some() {
            flags |= Self::FLAG_NOISE;
        }
        if self.cca_threshold_dbm.is_some() {
            flags |= Self::FLAG_CCA;
        }
        w.push_u8(flags)?;
        w.push_u16(self.channel_busy_per_mille)?;
        w.push_i16(self.noise_floor_dbm.unwrap_or(0))?;
        w.push_i16(self.cca_threshold_dbm.unwrap_or(0))?;
        w.push_u32(self.deferrals)?;
        w.push_u32(self.false_preambles)?;
        w.push_u32(self.contention_timeouts)?;
        w.push_u32(self.duty_holds)?;
        w.push_u32(self.duty_timeouts)?;
        w.push_u32(self.radio_recoveries)?;
        Ok(())
    }

    fn decode(r: &mut Reader) -> Result<Self, SnapshotDecodeError> {
        let flags = r.read_u8()?;
        let channel_busy_per_mille = r.read_u16()?;
        let noise = r.read_i16()?;
        let cca = r.read_i16()?;
        let deferrals = r.read_u32()?;
        let false_preambles = r.read_u32()?;
        let contention_timeouts = r.read_u32()?;
        let duty_holds = r.read_u32()?;
        let duty_timeouts = r.read_u32()?;
        let radio_recoveries = r.read_u32()?;
        Ok(Self {
            channel_busy_per_mille,
            noise_floor_dbm: (flags & Self::FLAG_NOISE != 0).then_some(noise),
            cca_threshold_dbm: (flags & Self::FLAG_CCA != 0).then_some(cca),
            deferrals,
            false_preambles,
            contention_timeouts,
            duty_holds,
            duty_timeouts,
            radio_recoveries,
        })
    }
}

/// One interface's destination/link counts from the interface store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceCount {
    pub kind: ConfigInterface,
    pub destinations: u32,
    pub links: u32,
    pub transported_links: u32,
}

impl InterfaceCount {
    fn encode(&self, w: &mut Writer) -> Result<(), SnapshotEncodeError> {
        w.push_u8(self.kind.to_wire_code())?;
        w.push_u32(self.destinations)?;
        w.push_u32(self.links)?;
        w.push_u32(self.transported_links)?;
        Ok(())
    }

    fn decode(r: &mut Reader) -> Result<Self, SnapshotDecodeError> {
        let kind = ConfigInterface::from_wire_code(r.read_u8()?)
            .ok_or(SnapshotDecodeError::InvalidSection)?;
        Ok(Self {
            kind,
            destinations: r.read_u32()?,
            links: r.read_u32()?,
            transported_links: r.read_u32()?,
        })
    }
}

/// One typed section of a snapshot body. Variants correspond to the stable
/// section tags above; the decoder skips bytes it does not recognize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotSection {
    /// Firmware version string (USB descriptor identity comes from the WebUSB
    /// device descriptor, so the body carries only the version).
    DeviceInfo {
        version: HeaplessString<MAX_FIRMWARE_VERSION_BYTES>,
    },
    /// Profile-store persistence state.
    Persistence {
        state: SnapshotPersistence,
    },
    LoraStatus(InterfaceStatusBody),
    UsbStatus(InterfaceStatusBody),
    BleStatus {
        status: InterfaceStatusBody,
        failure_reason: HeaplessString<MAX_FAILURE_REASON_BYTES>,
    },
    BleRecovery(BluetoothRecoveryBody),
    LoraSpectrum(LoRaSpectrumBody),
    /// The persisted radio profile (the live applied profile is not reachable
    /// from the config task; the persisted one matches after a successful
    /// apply-and-persist).
    RadioProfile(RadioProfile),
    InterfaceCounts(HeaplessVec<InterfaceCount, MAX_INTERFACE_COUNTS>),
}

impl SnapshotSection {
    const fn tag(&self) -> u8 {
        match self {
            Self::DeviceInfo { .. } => SECTION_DEVICE_INFO,
            Self::Persistence { .. } => SECTION_PERSISTENCE,
            Self::LoraStatus(_) => SECTION_LORA_STATUS,
            Self::UsbStatus(_) => SECTION_USB_STATUS,
            Self::BleStatus { .. } => SECTION_BLE_STATUS,
            Self::BleRecovery(_) => SECTION_BLE_RECOVERY,
            Self::LoraSpectrum(_) => SECTION_LORA_SPECTRUM,
            Self::RadioProfile(_) => SECTION_RADIO_PROFILE,
            Self::InterfaceCounts(_) => SECTION_INTERFACE_COUNTS,
        }
    }

    fn encode_body(&self, w: &mut Writer) -> Result<(), SnapshotEncodeError> {
        match self {
            Self::DeviceInfo { version } => {
                let bytes = version.as_bytes();
                w.push_u8(bytes.len() as u8)?;
                w.push_bytes(bytes)?;
            }
            Self::Persistence { state } => w.push_u8(state.to_wire())?,
            Self::LoraStatus(status) => status.encode(w)?,
            Self::UsbStatus(status) => status.encode(w)?,
            Self::BleStatus {
                status,
                failure_reason,
            } => {
                status.encode(w)?;
                let bytes = failure_reason.as_bytes();
                w.push_u8(bytes.len() as u8)?;
                w.push_bytes(bytes)?;
            }
            Self::BleRecovery(body) => body.encode(w)?,
            Self::LoraSpectrum(body) => body.encode(w)?,
            Self::RadioProfile(profile) => {
                let mut buf = [0u8; crate::interfaces::lora::PROFILE_WIRE_LEN];
                profile.encode(&mut buf);
                w.push_bytes(&buf)?;
            }
            Self::InterfaceCounts(counts) => {
                w.push_u8(counts.len() as u8)?;
                for count in counts {
                    count.encode(w)?;
                }
            }
        }
        Ok(())
    }

    /// Decode a section with a known tag from its body bytes. Returns
    /// `Ok(None)` for unknown tags so the caller can skip them without error.
    fn decode_tagged(tag: u8, body: &[u8]) -> Result<Option<Self>, SnapshotDecodeError> {
        let mut r = Reader::new(body);
        let section = match tag {
            SECTION_DEVICE_INFO => {
                let len = r.read_u8()? as usize;
                let bytes = r.read_bytes(len)?;
                let mut version = HeaplessString::new();
                version
                    .push_str(
                        core::str::from_utf8(bytes)
                            .map_err(|_| SnapshotDecodeError::InvalidSection)?,
                    )
                    .map_err(|_| SnapshotDecodeError::InvalidSection)?;
                Self::DeviceInfo { version }
            }
            SECTION_PERSISTENCE => Self::Persistence {
                state: SnapshotPersistence::from_wire(r.read_u8()?),
            },
            SECTION_LORA_STATUS => Self::LoraStatus(InterfaceStatusBody::decode(&mut r)?),
            SECTION_USB_STATUS => Self::UsbStatus(InterfaceStatusBody::decode(&mut r)?),
            SECTION_BLE_STATUS => {
                let status = InterfaceStatusBody::decode(&mut r)?;
                let len = r.read_u8()? as usize;
                let bytes = r.read_bytes(len)?;
                let mut failure_reason = HeaplessString::new();
                failure_reason
                    .push_str(
                        core::str::from_utf8(bytes)
                            .map_err(|_| SnapshotDecodeError::InvalidSection)?,
                    )
                    .map_err(|_| SnapshotDecodeError::InvalidSection)?;
                Self::BleStatus {
                    status,
                    failure_reason,
                }
            }
            SECTION_BLE_RECOVERY => Self::BleRecovery(BluetoothRecoveryBody::decode(&mut r)?),
            SECTION_LORA_SPECTRUM => Self::LoraSpectrum(LoRaSpectrumBody::decode(&mut r)?),
            SECTION_RADIO_PROFILE => {
                let bytes = r.read_bytes(crate::interfaces::lora::PROFILE_WIRE_LEN)?;
                let profile =
                    RadioProfile::decode(bytes).ok_or(SnapshotDecodeError::InvalidProfile)?;
                Self::RadioProfile(profile)
            }
            SECTION_INTERFACE_COUNTS => {
                let count = r.read_u8()? as usize;
                let mut counts = HeaplessVec::new();
                for _ in 0..count {
                    let entry = InterfaceCount::decode(&mut r)?;
                    counts
                        .push(entry)
                        .map_err(|_| SnapshotDecodeError::TooManySections)?;
                }
                Self::InterfaceCounts(counts)
            }
            _ => return Ok(None),
        };
        Ok(Some(section))
    }
}

/// A decoded snapshot body: the sections the decoder recognized, in wire order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotBody {
    pub sections: HeaplessVec<SnapshotSection, MAX_SNAPSHOT_SECTIONS>,
}

impl SnapshotBody {
    /// Encode the body into `out`, returning the number of bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, SnapshotEncodeError> {
        let mut w = Writer::new(out);
        w.push_u8(self.sections.len() as u8)?;
        for section in &self.sections {
            let tag = section.tag();
            let len_index = w.written() + 1;
            w.push_u8(tag)?;
            w.push_u16(0)?;
            let body_start = w.written();
            section.encode_body(&mut w)?;
            let body_len = w.written() - body_start;
            w.set_u16_le(len_index, body_len as u16);
        }
        if w.written() > MAX_SNAPSHOT_BODY_BYTES {
            return Err(SnapshotEncodeError::BodyTooLarge);
        }
        Ok(w.written())
    }

    /// Decode a body. Unknown section tags are skipped; known sections land in
    /// `sections` in wire order.
    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotDecodeError> {
        let mut r = Reader::new(bytes);
        let count = r.read_u8()?;
        let mut sections = HeaplessVec::new();
        for _ in 0..count {
            let tag = r.read_u8()?;
            let len = r.read_u16()? as usize;
            let body = r.read_bytes(len)?;
            if let Some(section) = SnapshotSection::decode_tagged(tag, body)? {
                sections
                    .push(section)
                    .map_err(|_| SnapshotDecodeError::TooManySections)?;
            }
        }
        Ok(Self { sections })
    }
}

struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn written(&self) -> usize {
        self.pos
    }

    fn push_u8(&mut self, value: u8) -> Result<(), SnapshotEncodeError> {
        self.reserve(1)?[0] = value;
        self.pos += 1;
        Ok(())
    }

    fn push_u16(&mut self, value: u16) -> Result<(), SnapshotEncodeError> {
        self.push_bytes(&value.to_le_bytes())
    }

    fn push_u32(&mut self, value: u32) -> Result<(), SnapshotEncodeError> {
        self.push_bytes(&value.to_le_bytes())
    }

    fn push_u64(&mut self, value: u64) -> Result<(), SnapshotEncodeError> {
        self.push_bytes(&value.to_le_bytes())
    }

    fn push_i16(&mut self, value: i16) -> Result<(), SnapshotEncodeError> {
        self.push_bytes(&value.to_le_bytes())
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), SnapshotEncodeError> {
        let slot = self.reserve(bytes.len())?;
        slot.copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }

    fn set_u16_le(&mut self, at: usize, value: u16) {
        let bytes = value.to_le_bytes();
        self.buf[at] = bytes[0];
        self.buf[at + 1] = bytes[1];
    }

    fn reserve(&mut self, n: usize) -> Result<&mut [u8], SnapshotEncodeError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(SnapshotEncodeError::BufferTooSmall)?;
        if end > self.buf.len() {
            return Err(SnapshotEncodeError::BufferTooSmall);
        }
        Ok(&mut self.buf[self.pos..end])
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, SnapshotDecodeError> {
        Ok(*self
            .read_bytes(1)?
            .first()
            .ok_or(SnapshotDecodeError::Truncated)?)
    }

    fn read_u16(&mut self) -> Result<u16, SnapshotDecodeError> {
        Ok(u16::from_le_bytes(
            self.read_bytes(2)?
                .try_into()
                .map_err(|_| SnapshotDecodeError::Truncated)?,
        ))
    }

    fn read_u32(&mut self) -> Result<u32, SnapshotDecodeError> {
        Ok(u32::from_le_bytes(
            self.read_bytes(4)?
                .try_into()
                .map_err(|_| SnapshotDecodeError::Truncated)?,
        ))
    }

    fn read_u64(&mut self) -> Result<u64, SnapshotDecodeError> {
        Ok(u64::from_le_bytes(
            self.read_bytes(8)?
                .try_into()
                .map_err(|_| SnapshotDecodeError::Truncated)?,
        ))
    }

    fn read_i16(&mut self) -> Result<i16, SnapshotDecodeError> {
        Ok(i16::from_le_bytes(
            self.read_bytes(2)?
                .try_into()
                .map_err(|_| SnapshotDecodeError::Truncated)?,
        ))
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], SnapshotDecodeError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(SnapshotDecodeError::OversizeSection)?;
        if end > self.bytes.len() {
            return Err(SnapshotDecodeError::Truncated);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::lora::{ModemPreset, PreambleSymbols, Region, TxPower};

    fn sample_profile() -> RadioProfile {
        RadioProfile {
            frequency: Region::Eu868.default_frequency(),
            modulation: ModemPreset::LongSlow.modulation(),
            tx_power: TxPower::new(14),
            preamble: PreambleSymbols::new(32),
            region: Region::Eu868,
        }
    }

    fn lora_status() -> InterfaceStatusBody {
        InterfaceStatusBody {
            enabled: true,
            connection: ConnectionState::Connected,
            rx_bytes: 0x0011_2233_4455,
            tx_bytes: 0x6677_8899_aabb,
            airtime: Some(AirtimeUtilization {
                short_per_mille: 12,
                long_per_mille: 34,
            }),
            transfer_rates: Some(TransferRates {
                rx_bps: 1200,
                tx_bps: 9600,
            }),
        }
    }

    #[test]
    fn every_section_round_trips() {
        let mut counts = HeaplessVec::new();
        counts
            .push(InterfaceCount {
                kind: ConfigInterface::Lora,
                destinations: 5,
                links: 2,
                transported_links: 1,
            })
            .unwrap();
        counts
            .push(InterfaceCount {
                kind: ConfigInterface::Ble,
                destinations: 3,
                links: 0,
                transported_links: 0,
            })
            .unwrap();

        let mut sections = HeaplessVec::new();
        sections
            .push(SnapshotSection::DeviceInfo {
                version: "0.1.0-t1000e".try_into().unwrap(),
            })
            .unwrap();
        sections
            .push(SnapshotSection::Persistence {
                state: SnapshotPersistence::Deferred,
            })
            .unwrap();
        sections
            .push(SnapshotSection::LoraStatus(lora_status()))
            .unwrap();
        sections
            .push(SnapshotSection::UsbStatus(InterfaceStatusBody {
                enabled: false,
                connection: ConnectionState::Disabled,
                rx_bytes: 0,
                tx_bytes: 0,
                airtime: None,
                transfer_rates: None,
            }))
            .unwrap();
        sections
            .push(SnapshotSection::BleStatus {
                status: InterfaceStatusBody {
                    enabled: true,
                    connection: ConnectionState::Degraded,
                    rx_bytes: 100,
                    tx_bytes: 200,
                    airtime: None,
                    transfer_rates: None,
                },
                failure_reason: "supervisor link lost".try_into().unwrap(),
            })
            .unwrap();
        sections
            .push(SnapshotSection::BleRecovery(BluetoothRecoveryBody {
                ingress_pressure: 7,
                setup_failures: 1,
                transport_closures: 2,
                egress_pressure_events: 9,
                member_count: 3,
            }))
            .unwrap();
        sections
            .push(SnapshotSection::LoraSpectrum(LoRaSpectrumBody {
                channel_busy_per_mille: 42,
                noise_floor_dbm: Some(-105),
                cca_threshold_dbm: Some(-80),
                deferrals: 10,
                false_preambles: 1,
                contention_timeouts: 0,
                duty_holds: 2,
                duty_timeouts: 0,
                radio_recoveries: 1,
            }))
            .unwrap();
        sections
            .push(SnapshotSection::RadioProfile(sample_profile()))
            .unwrap();
        sections
            .push(SnapshotSection::InterfaceCounts(counts))
            .unwrap();

        let body = SnapshotBody { sections };
        let mut buf = [0u8; MAX_SNAPSHOT_BODY_BYTES];
        let n = body.encode(&mut buf).unwrap();
        let decoded = SnapshotBody::decode(&buf[..n]).unwrap();
        assert_eq!(decoded, body);
    }

    #[test]
    fn unknown_section_tag_is_skipped_not_rejected() {
        let mut sections = HeaplessVec::new();
        sections
            .push(SnapshotSection::Persistence {
                state: SnapshotPersistence::Durable,
            })
            .unwrap();
        let body = SnapshotBody { sections };
        let mut buf = [0u8; 64];
        let n = body.encode(&mut buf).unwrap();

        // Inject a bogus section (tag 0x7F, empty body) between the count and
        // the real section by rebuilding the body bytes by hand.
        let mut doctored = [0u8; 64];
        doctored[0] = 2; // two sections
        doctored[1] = 0x7F; // unknown tag
        doctored[2..4].copy_from_slice(&0u16.to_le_bytes()); // zero-length body
        doctored[4..4 + n - 1].copy_from_slice(&buf[1..n]);
        let decoded = SnapshotBody::decode(&doctored[..4 + n - 1]).unwrap();
        assert_eq!(decoded.sections.len(), 1);
        assert_eq!(
            decoded.sections[0],
            SnapshotSection::Persistence {
                state: SnapshotPersistence::Durable,
            }
        );
    }

    #[test]
    fn truncated_body_is_rejected() {
        assert_eq!(
            SnapshotBody::decode(&[]).err(),
            Some(SnapshotDecodeError::Truncated)
        );
    }

    #[test]
    fn oversize_section_is_rejected() {
        let mut buf = [0u8; 8];
        buf[0] = 1; // one section
        buf[1] = SECTION_PERSISTENCE;
        buf[2..4].copy_from_slice(&100u16.to_le_bytes()); // claims 100 bytes
        assert_eq!(
            SnapshotBody::decode(&buf).err(),
            Some(SnapshotDecodeError::Truncated)
        );
    }

    #[test]
    fn invalid_profile_section_is_rejected() {
        // Encode a RadioProfile section but corrupt the reserved byte so
        // RadioProfile::decode refuses it.
        let mut sections = HeaplessVec::new();
        sections
            .push(SnapshotSection::RadioProfile(sample_profile()))
            .unwrap();
        let body = SnapshotBody { sections };
        let mut buf = [0u8; 64];
        let n = body.encode(&mut buf).unwrap();
        // The profile body starts after [count][tag][len][12 bytes]; corrupt the
        // reserved (last) profile byte.
        let reserved_index = n - 1;
        buf[reserved_index] = 0xFF;
        assert_eq!(
            SnapshotBody::decode(&buf[..n]).err(),
            Some(SnapshotDecodeError::InvalidProfile)
        );
    }

    #[test]
    fn empty_body_decodes_to_no_sections() {
        let mut buf = [0u8; 1];
        buf[0] = 0;
        let decoded = SnapshotBody::decode(&buf).unwrap();
        assert!(decoded.sections.is_empty());
    }

    #[test]
    fn encode_rejects_a_buffer_that_is_too_small() {
        let mut sections = HeaplessVec::new();
        sections
            .push(SnapshotSection::RadioProfile(sample_profile()))
            .unwrap();
        let body = SnapshotBody { sections };
        let mut buf = [0u8; 4]; // far too small for the section header + profile
        assert_eq!(
            body.encode(&mut buf).err(),
            Some(SnapshotEncodeError::BufferTooSmall)
        );
    }
}
