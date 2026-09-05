use super::super::MonotonicMicros;
use super::profile::{TURBO_AIR_FRAME_MAX, TURBO_FRAME_DATA_MAX, TURBO_LOGICAL_PACKET_MAX};
use super::schedule::{SupercycleCycle, SupercycleCycleError};

const FRAME_VERSION: u8 = 1;
const FRAME_VERSION_SHIFT: u8 = 4;
const FRAME_VERSION_MASK: u8 = 0x30;
const FRAME_TYPE_SHIFT: u8 = 6;
const FRAME_RESERVED_MASK: u8 = 0x0f;
const COMMON_HEADER_BYTES: usize = 2;
const DATAGRAM_ID_BYTES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatagramId([u8; DATAGRAM_ID_BYTES]);

impl DatagramId {
    pub const fn new(bytes: [u8; DATAGRAM_ID_BYTES]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; DATAGRAM_ID_BYTES] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboFrameType {
    DataSingle,
    DataFirst,
    DataFinal,
    Acquisition,
}

impl TurboFrameType {
    const fn bits(self) -> u8 {
        match self {
            Self::DataSingle => 0,
            Self::DataFirst => 1,
            Self::DataFinal => 2,
            Self::Acquisition => 3,
        }
    }

    const fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::DataSingle,
            1 => Self::DataFirst,
            2 => Self::DataFinal,
            _ => Self::Acquisition,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedTurboFrame {
    bytes: [u8; TURBO_AIR_FRAME_MAX],
    len: u8,
}

impl EncodedTurboFrame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum EncodedDatagram {
    Single(EncodedTurboFrame),
    Fragmented {
        first: EncodedTurboFrame,
        final_frame: EncodedTurboFrame,
    },
}

impl EncodedDatagram {
    pub const fn frame_count(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Fragmented { .. } => 2,
        }
    }

    pub fn frame(&self, index: usize) -> Result<&EncodedTurboFrame, TurboFrameError> {
        match (self, index) {
            (Self::Single(frame), 0) => Ok(frame),
            (Self::Fragmented { first, .. }, 0) => Ok(first),
            (Self::Fragmented { final_frame, .. }, 1) => Ok(final_frame),
            _ => Err(TurboFrameError::FrameIndexOutsideDatagram {
                index,
                frame_count: self.frame_count(),
            }),
        }
    }

    pub fn keyed_airtime_us(&self, profile: super::TurboPhyProfile) -> u64 {
        match self {
            Self::Single(frame) => profile.time_on_air_us(frame.len()),
            Self::Fragmented { first, final_frame } => profile
                .time_on_air_us(first.len())
                .saturating_add(profile.time_on_air_us(final_frame.len())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedTurboFrame<'a> {
    DataSingle {
        cycle: SupercycleCycle,
        datagram_id: DatagramId,
        payload: &'a [u8],
    },
    DataFirst {
        cycle: SupercycleCycle,
        datagram_id: DatagramId,
        payload: &'a [u8],
    },
    DataFinal {
        cycle: SupercycleCycle,
        datagram_id: DatagramId,
        payload: &'a [u8],
    },
    Acquisition {
        cycle: SupercycleCycle,
        contention_slot: u8,
    },
}

impl DecodedTurboFrame<'_> {
    pub const fn cycle(&self) -> SupercycleCycle {
        match self {
            Self::DataSingle { cycle, .. }
            | Self::DataFirst { cycle, .. }
            | Self::DataFinal { cycle, .. }
            | Self::Acquisition { cycle, .. } => *cycle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboFrameError {
    EmptyDatagram,
    DatagramTooLarge { bytes: usize, maximum: usize },
    FrameTooShort { bytes: usize, minimum: usize },
    FrameTooLarge { bytes: usize, maximum: usize },
    UnsupportedVersion { version: u8 },
    ReservedBitsSet { bits: u8 },
    InvalidCycle(SupercycleCycleError),
    EmptyPayload,
    NonCanonicalFirstFragment { payload_bytes: usize },
    InvalidAcquisitionLength { bytes: usize },
    AcquisitionContentionSlotOutsideRange { contention_slot: u8 },
    AcquisitionReservedBitsSet { bits: u8 },
    FrameIndexOutsideDatagram { index: usize, frame_count: usize },
}

pub fn encode_datagram(
    cycle: SupercycleCycle,
    datagram_id: DatagramId,
    payload: &[u8],
) -> Result<EncodedDatagram, TurboFrameError> {
    if payload.is_empty() {
        return Err(TurboFrameError::EmptyDatagram);
    }
    if payload.len() > TURBO_LOGICAL_PACKET_MAX {
        return Err(TurboFrameError::DatagramTooLarge {
            bytes: payload.len(),
            maximum: TURBO_LOGICAL_PACKET_MAX,
        });
    }
    if payload.len() <= TURBO_FRAME_DATA_MAX {
        return Ok(EncodedDatagram::Single(encode_data_frame(
            TurboFrameType::DataSingle,
            cycle,
            datagram_id,
            payload,
        )));
    }
    Ok(EncodedDatagram::Fragmented {
        first: encode_data_frame(
            TurboFrameType::DataFirst,
            cycle,
            datagram_id,
            &payload[..TURBO_FRAME_DATA_MAX],
        ),
        final_frame: encode_data_frame(
            TurboFrameType::DataFinal,
            cycle,
            datagram_id,
            &payload[TURBO_FRAME_DATA_MAX..],
        ),
    })
}

pub fn encode_acquisition(
    cycle: SupercycleCycle,
    contention_slot: u8,
) -> Result<EncodedTurboFrame, TurboFrameError> {
    if contention_slot >= super::ACQUISITION_BEACON_CONTENTION_SLOTS {
        return Err(TurboFrameError::AcquisitionContentionSlotOutsideRange { contention_slot });
    }
    let mut frame = empty_frame();
    frame.bytes[0] = header(TurboFrameType::Acquisition);
    frame.bytes[1] = cycle.index();
    frame.bytes[2] = contention_slot;
    frame.len = 3;
    Ok(frame)
}

pub fn decode_frame(bytes: &[u8]) -> Result<DecodedTurboFrame<'_>, TurboFrameError> {
    if bytes.len() < COMMON_HEADER_BYTES {
        return Err(TurboFrameError::FrameTooShort {
            bytes: bytes.len(),
            minimum: COMMON_HEADER_BYTES,
        });
    }
    if bytes.len() > TURBO_AIR_FRAME_MAX {
        return Err(TurboFrameError::FrameTooLarge {
            bytes: bytes.len(),
            maximum: TURBO_AIR_FRAME_MAX,
        });
    }
    let version = (bytes[0] & FRAME_VERSION_MASK) >> FRAME_VERSION_SHIFT;
    if version != FRAME_VERSION {
        return Err(TurboFrameError::UnsupportedVersion { version });
    }
    let reserved = bytes[0] & FRAME_RESERVED_MASK;
    if reserved != 0 {
        return Err(TurboFrameError::ReservedBitsSet { bits: reserved });
    }
    let frame_type = TurboFrameType::from_bits(bytes[0] >> FRAME_TYPE_SHIFT);
    let cycle = SupercycleCycle::new(bytes[1]).map_err(TurboFrameError::InvalidCycle)?;
    match frame_type {
        TurboFrameType::Acquisition => {
            if bytes.len() != 3 {
                return Err(TurboFrameError::InvalidAcquisitionLength { bytes: bytes.len() });
            }
            if bytes[2] & 0xf0 != 0 {
                return Err(TurboFrameError::AcquisitionReservedBitsSet {
                    bits: bytes[2] & 0xf0,
                });
            }
            if bytes[2] >= super::ACQUISITION_BEACON_CONTENTION_SLOTS {
                return Err(TurboFrameError::AcquisitionContentionSlotOutsideRange {
                    contention_slot: bytes[2],
                });
            }
            Ok(DecodedTurboFrame::Acquisition {
                cycle,
                contention_slot: bytes[2],
            })
        }
        frame_type => {
            if bytes.len() <= COMMON_HEADER_BYTES + DATAGRAM_ID_BYTES {
                return Err(TurboFrameError::EmptyPayload);
            }
            let datagram_id = DatagramId::new([bytes[2], bytes[3], bytes[4]]);
            let payload = &bytes[5..];
            match frame_type {
                TurboFrameType::DataSingle => Ok(DecodedTurboFrame::DataSingle {
                    cycle,
                    datagram_id,
                    payload,
                }),
                TurboFrameType::DataFirst if payload.len() == TURBO_FRAME_DATA_MAX => {
                    Ok(DecodedTurboFrame::DataFirst {
                        cycle,
                        datagram_id,
                        payload,
                    })
                }
                TurboFrameType::DataFirst => Err(TurboFrameError::NonCanonicalFirstFragment {
                    payload_bytes: payload.len(),
                }),
                TurboFrameType::DataFinal => Ok(DecodedTurboFrame::DataFinal {
                    cycle,
                    datagram_id,
                    payload,
                }),
                TurboFrameType::Acquisition => {
                    Err(TurboFrameError::InvalidAcquisitionLength { bytes: bytes.len() })
                }
            }
        }
    }
}

fn encode_data_frame(
    frame_type: TurboFrameType,
    cycle: SupercycleCycle,
    datagram_id: DatagramId,
    payload: &[u8],
) -> EncodedTurboFrame {
    let mut frame = empty_frame();
    frame.bytes[0] = header(frame_type);
    frame.bytes[1] = cycle.index();
    frame.bytes[2..5].copy_from_slice(&datagram_id.bytes());
    frame.bytes[5..5 + payload.len()].copy_from_slice(payload);
    frame.len = (5 + payload.len()) as u8;
    frame
}

const fn header(frame_type: TurboFrameType) -> u8 {
    (frame_type.bits() << FRAME_TYPE_SHIFT) | (FRAME_VERSION << FRAME_VERSION_SHIFT)
}

const fn empty_frame() -> EncodedTurboFrame {
    EncodedTurboFrame {
        bytes: [0; TURBO_AIR_FRAME_MAX],
        len: 0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurboDatagram {
    bytes: [u8; TURBO_LOGICAL_PACKET_MAX],
    len: u16,
}

impl TurboDatagram {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
enum ReassemblyState {
    Empty,
    First {
        cycle: SupercycleCycle,
        datagram_id: DatagramId,
        payload: [u8; TURBO_FRAME_DATA_MAX],
        received_at: MonotonicMicros,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReassemblyLifetime(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReassemblyLifetimeError {
    Empty,
}

impl ReassemblyLifetime {
    pub const fn new(micros: u64) -> Result<Self, ReassemblyLifetimeError> {
        if micros == 0 {
            return Err(ReassemblyLifetimeError::Empty);
        }
        Ok(Self(micros))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReassemblyError {
    FinalWithoutFirst,
    FragmentIdentityMismatch,
    UnexpectedFrame,
    NonMonotonicFrame,
    FirstFragmentExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ReassemblyOutcome {
    WaitingForFinal,
    Complete(TurboDatagram),
}

pub struct TurboReassembler {
    state: ReassemblyState,
    lifetime: ReassemblyLifetime,
}

impl TurboReassembler {
    pub const fn new(lifetime: ReassemblyLifetime) -> Self {
        Self {
            state: ReassemblyState::Empty,
            lifetime,
        }
    }

    pub fn ingest(
        &mut self,
        received_at: MonotonicMicros,
        frame: DecodedTurboFrame<'_>,
    ) -> Result<ReassemblyOutcome, ReassemblyError> {
        match (self.state, frame) {
            (ReassemblyState::Empty, DecodedTurboFrame::DataSingle { payload, .. }) => Ok(
                ReassemblyOutcome::Complete(datagram_from_parts(payload, &[])),
            ),
            (
                ReassemblyState::Empty,
                DecodedTurboFrame::DataFirst {
                    cycle,
                    datagram_id,
                    payload,
                },
            ) => {
                let mut first = [0; TURBO_FRAME_DATA_MAX];
                first.copy_from_slice(payload);
                self.state = ReassemblyState::First {
                    cycle,
                    datagram_id,
                    payload: first,
                    received_at,
                };
                Ok(ReassemblyOutcome::WaitingForFinal)
            }
            (ReassemblyState::Empty, DecodedTurboFrame::DataFinal { .. }) => {
                Err(ReassemblyError::FinalWithoutFirst)
            }
            (
                ReassemblyState::First {
                    cycle,
                    datagram_id,
                    payload: first,
                    received_at: first_received_at,
                },
                DecodedTurboFrame::DataFinal {
                    cycle: final_cycle,
                    datagram_id: final_id,
                    payload,
                },
            ) => {
                self.state = ReassemblyState::Empty;
                let Some(age_us) = received_at.micros().checked_sub(first_received_at.micros())
                else {
                    return Err(ReassemblyError::NonMonotonicFrame);
                };
                if age_us > self.lifetime.0 {
                    return Err(ReassemblyError::FirstFragmentExpired);
                }
                if cycle != final_cycle || datagram_id != final_id {
                    return Err(ReassemblyError::FragmentIdentityMismatch);
                }
                Ok(ReassemblyOutcome::Complete(datagram_from_parts(
                    &first, payload,
                )))
            }
            (ReassemblyState::First { .. }, _) => {
                self.state = ReassemblyState::Empty;
                Err(ReassemblyError::UnexpectedFrame)
            }
            (_, DecodedTurboFrame::Acquisition { .. }) => Err(ReassemblyError::UnexpectedFrame),
        }
    }
}

fn datagram_from_parts(first: &[u8], second: &[u8]) -> TurboDatagram {
    let mut bytes = [0; TURBO_LOGICAL_PACKET_MAX];
    bytes[..first.len()].copy_from_slice(first);
    bytes[first.len()..first.len() + second.len()].copy_from_slice(second);
    TurboDatagram {
        bytes,
        len: (first.len() + second.len()) as u16,
    }
}
