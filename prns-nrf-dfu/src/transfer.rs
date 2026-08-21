use alloc::vec::Vec;
use core::time::Duration;

use thiserror::Error;

use crate::{Acknowledgement, DfuImage, EncodedFrame, FrameEncodeError, ReliableFrameEncoder};

const DFU_INIT_PACKET: u32 = 1;
const DFU_START_PACKET: u32 = 3;
const DFU_DATA_PACKET: u32 = 4;
const DFU_STOP_DATA_PACKET: u32 = 5;
const DFU_UPDATE_MODE_APPLICATION: u32 = 4;
const FIRMWARE_CHUNK_BYTES: usize = 512;
const FLASH_PAGE_BYTES: usize = 4096;
const FLASH_PAGE_ERASE_MICROSECONDS: u64 = 89_700;
const FLASH_PAGE_WRITE_MICROSECONDS: u64 = 102_400;
const MINIMUM_ERASE_WAIT_MICROSECONDS: u64 = 500_000;

pub const RELIABLE_FRAME_ATTEMPT_LIMIT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReliableFrameAttempt {
    sequence_number: u8,
    number: u8,
}

impl ReliableFrameAttempt {
    pub const fn sequence_number(self) -> u8 {
        self.sequence_number
    }

    pub const fn number(self) -> u8 {
        self.number
    }

    pub const fn limit(self) -> u8 {
        RELIABLE_FRAME_ATTEMPT_LIMIT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReliableFrameAttemptState {
    Ready,
    Pending { sequence_number: u8, attempts: u8 },
}

#[derive(Debug)]
pub struct ReliableFrameAttempts {
    state: ReliableFrameAttemptState,
}

impl ReliableFrameAttempts {
    pub const fn new() -> Self {
        Self {
            state: ReliableFrameAttemptState::Ready,
        }
    }

    pub fn begin(
        &mut self,
        sequence_number: u8,
    ) -> Result<ReliableFrameAttempt, ReliableFrameAttemptError> {
        let number = match self.state {
            ReliableFrameAttemptState::Ready => 1,
            ReliableFrameAttemptState::Pending {
                sequence_number: pending,
                attempts,
            } if pending == sequence_number && attempts < RELIABLE_FRAME_ATTEMPT_LIMIT => {
                attempts + 1
            }
            ReliableFrameAttemptState::Pending {
                sequence_number: pending,
                attempts,
            } if pending == sequence_number => {
                return Err(ReliableFrameAttemptError::Exhausted {
                    sequence_number,
                    attempts,
                });
            }
            ReliableFrameAttemptState::Pending {
                sequence_number: pending,
                ..
            } => {
                return Err(ReliableFrameAttemptError::FrameChanged {
                    pending_sequence_number: pending,
                    actual_sequence_number: sequence_number,
                });
            }
        };
        self.state = ReliableFrameAttemptState::Pending {
            sequence_number,
            attempts: number,
        };
        Ok(ReliableFrameAttempt {
            sequence_number,
            number,
        })
    }

    pub fn accepted(&mut self, sequence_number: u8) -> Result<(), ReliableFrameAttemptError> {
        match self.state {
            ReliableFrameAttemptState::Ready => Err(ReliableFrameAttemptError::NoPendingAttempt),
            ReliableFrameAttemptState::Pending {
                sequence_number: pending,
                ..
            } if pending != sequence_number => Err(ReliableFrameAttemptError::FrameChanged {
                pending_sequence_number: pending,
                actual_sequence_number: sequence_number,
            }),
            ReliableFrameAttemptState::Pending { .. } => {
                self.state = ReliableFrameAttemptState::Ready;
                Ok(())
            }
        }
    }
}

impl Default for ReliableFrameAttempts {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ReliableFrameAttemptError {
    #[error("DFU frame {sequence_number} exhausted its {attempts} reliable transmission attempts")]
    Exhausted { sequence_number: u8, attempts: u8 },
    #[error(
        "DFU frame changed from sequence {pending_sequence_number} to {actual_sequence_number} before acknowledgement"
    )]
    FrameChanged {
        pending_sequence_number: u8,
        actual_sequence_number: u8,
    },
    #[error("no reliable DFU frame attempt is pending")]
    NoPendingAttempt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DfuBankLayout {
    Single,
    Dual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequiredWait(Duration);

impl RequiredWait {
    pub const fn duration(self) -> Duration {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferProgress {
    written_bytes: usize,
    total_bytes: usize,
}

impl TransferProgress {
    pub const fn written_bytes(self) -> usize {
        self.written_bytes
    }

    pub const fn total_bytes(self) -> usize {
        self.total_bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferState {
    AwaitingAcknowledgement,
    Ready,
    Complete,
}

#[derive(Debug)]
pub struct PendingTransferFrame<'a> {
    frame: &'a EncodedFrame,
    wait_after_acknowledgement: RequiredWait,
    progress_after_acknowledgement: TransferProgress,
}

impl PendingTransferFrame<'_> {
    pub fn frame(&self) -> &EncodedFrame {
        self.frame
    }

    pub const fn wait_after_acknowledgement(&self) -> RequiredWait {
        self.wait_after_acknowledgement
    }

    pub const fn progress_after_acknowledgement(&self) -> TransferProgress {
        self.progress_after_acknowledgement
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransferPhase {
    Start,
    Init,
    Data { offset: usize },
    Stop,
    Complete,
}

#[derive(Debug)]
struct PendingFrame {
    encoded: EncodedFrame,
    next_phase: TransferPhase,
    wait: RequiredWait,
    progress: TransferProgress,
}

#[derive(Debug)]
pub struct DfuTransfer<'a> {
    image: DfuImage<'a>,
    bank_layout: DfuBankLayout,
    encoder: ReliableFrameEncoder,
    phase: TransferPhase,
    pending: Option<PendingFrame>,
}

impl<'a> DfuTransfer<'a> {
    pub fn new(image: DfuImage<'a>, bank_layout: DfuBankLayout) -> Self {
        Self {
            image,
            bank_layout,
            encoder: ReliableFrameEncoder::new(),
            phase: TransferPhase::Start,
            pending: None,
        }
    }

    pub fn next_frame(&mut self) -> Result<Option<PendingTransferFrame<'_>>, TransferError> {
        if self.phase == TransferPhase::Complete {
            return Ok(None);
        }
        if self.pending.is_none() {
            self.pending = Some(self.build_pending_frame()?);
        }
        Ok(self.pending.as_ref().map(|pending| PendingTransferFrame {
            frame: &pending.encoded,
            wait_after_acknowledgement: pending.wait,
            progress_after_acknowledgement: pending.progress,
        }))
    }

    pub fn acknowledge(
        &mut self,
        acknowledgement: Acknowledgement,
    ) -> Result<TransferState, TransferError> {
        let pending = self
            .pending
            .as_ref()
            .ok_or(TransferError::NoFrameAwaitingAcknowledgement)?;
        let expected = pending.encoded.expected_acknowledgement();
        if acknowledgement != expected {
            return Err(TransferError::UnexpectedAcknowledgement {
                expected: expected.number(),
                actual: acknowledgement.number(),
            });
        }
        self.phase = pending.next_phase;
        self.pending = None;
        if self.phase == TransferPhase::Complete {
            Ok(TransferState::Complete)
        } else {
            Ok(TransferState::Ready)
        }
    }

    pub fn state(&self) -> TransferState {
        if self.phase == TransferPhase::Complete {
            TransferState::Complete
        } else if self.pending.is_some() {
            TransferState::AwaitingAcknowledgement
        } else {
            TransferState::Ready
        }
    }

    fn build_pending_frame(&mut self) -> Result<PendingFrame, TransferError> {
        let total_bytes = self.image.firmware().len();
        let (payload, next_phase, wait, written_bytes) = match self.phase {
            TransferPhase::Start => {
                let mut payload = Vec::with_capacity(20);
                payload.extend_from_slice(&DFU_START_PACKET.to_le_bytes());
                payload.extend_from_slice(&DFU_UPDATE_MODE_APPLICATION.to_le_bytes());
                payload.extend_from_slice(&0_u32.to_le_bytes());
                payload.extend_from_slice(&0_u32.to_le_bytes());
                payload.extend_from_slice(&(total_bytes as u32).to_le_bytes());
                (payload, TransferPhase::Init, erase_wait(total_bytes), 0)
            }
            TransferPhase::Init => {
                let mut payload = Vec::with_capacity(6 + self.image.init_packet().bytes().len());
                payload.extend_from_slice(&DFU_INIT_PACKET.to_le_bytes());
                payload.extend_from_slice(self.image.init_packet().bytes());
                payload.extend_from_slice(&0_u16.to_le_bytes());
                (
                    payload,
                    TransferPhase::Data { offset: 0 },
                    RequiredWait(Duration::ZERO),
                    0,
                )
            }
            TransferPhase::Data { offset } => {
                let remaining = &self.image.firmware()[offset..];
                let chunk_bytes = remaining.len().min(FIRMWARE_CHUNK_BYTES);
                let next_offset = offset + chunk_bytes;
                let mut payload = Vec::with_capacity(4 + chunk_bytes);
                payload.extend_from_slice(&DFU_DATA_PACKET.to_le_bytes());
                payload.extend_from_slice(&remaining[..chunk_bytes]);
                let next_phase = if next_offset == total_bytes {
                    TransferPhase::Stop
                } else {
                    TransferPhase::Data {
                        offset: next_offset,
                    }
                };
                let page_boundary = next_offset.is_multiple_of(FLASH_PAGE_BYTES);
                let final_chunk = next_offset == total_bytes;
                let wait = if page_boundary || final_chunk {
                    RequiredWait(Duration::from_micros(FLASH_PAGE_WRITE_MICROSECONDS))
                } else {
                    RequiredWait(Duration::ZERO)
                };
                (payload, next_phase, wait, next_offset)
            }
            TransferPhase::Stop => (
                DFU_STOP_DATA_PACKET.to_le_bytes().to_vec(),
                TransferPhase::Complete,
                activation_wait(total_bytes, self.bank_layout),
                total_bytes,
            ),
            TransferPhase::Complete => return Err(TransferError::AlreadyComplete),
        };
        Ok(PendingFrame {
            encoded: self.encoder.encode(&payload)?,
            next_phase,
            wait,
            progress: TransferProgress {
                written_bytes,
                total_bytes,
            },
        })
    }
}

fn flash_pages(image_bytes: usize) -> u64 {
    image_bytes.div_ceil(FLASH_PAGE_BYTES) as u64
}

fn erase_wait(image_bytes: usize) -> RequiredWait {
    let calculated = flash_pages(image_bytes) * FLASH_PAGE_ERASE_MICROSECONDS;
    RequiredWait(Duration::from_micros(
        calculated.max(MINIMUM_ERASE_WAIT_MICROSECONDS),
    ))
}

fn activation_wait(image_bytes: usize, bank_layout: DfuBankLayout) -> RequiredWait {
    let microseconds = match bank_layout {
        DfuBankLayout::Single => FLASH_PAGE_ERASE_MICROSECONDS + FLASH_PAGE_WRITE_MICROSECONDS,
        DfuBankLayout::Dual => {
            erase_wait(image_bytes).duration().as_micros() as u64
                + flash_pages(image_bytes) * FLASH_PAGE_WRITE_MICROSECONDS
        }
    };
    RequiredWait(Duration::from_micros(microseconds))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransferError {
    #[error(transparent)]
    Encode(#[from] FrameEncodeError),
    #[error("DFU transfer has no frame awaiting acknowledgement")]
    NoFrameAwaitingAcknowledgement,
    #[error("DFU transfer expected acknowledgement {expected}, received {actual}")]
    UnexpectedAcknowledgement { expected: u8, actual: u8 },
    #[error("DFU transfer is already complete")]
    AlreadyComplete,
}

#[cfg(test)]
mod tests {
    use std::{boxed::Box, error::Error};

    use crate::{
        ApplicationInitPacket, ApplicationInitPacketSpec, ApplicationVersion, DfuDeviceRevision,
        DfuDeviceType, DfuImage, DfuImageError, SoftdeviceFirmwareId, SoftdeviceRequirements,
    };

    use super::{
        DfuBankLayout, DfuTransfer, ReliableFrameAttemptError, ReliableFrameAttempts, TransferState,
    };

    fn image(firmware: &[u8]) -> Result<DfuImage<'_>, DfuImageError> {
        let fwid = SoftdeviceFirmwareId::new(0x0123)?;
        let requirements = SoftdeviceRequirements::new(fwid, std::iter::empty())?;
        let spec = ApplicationInitPacketSpec {
            device_type: DfuDeviceType::new(0x0052),
            device_revision: DfuDeviceRevision::new(52840),
            application_version: ApplicationVersion::NotEnforced,
            softdevices: requirements,
        };
        let init_packet = ApplicationInitPacket::build(firmware, &spec)?;
        DfuImage::new(firmware, init_packet)
    }

    #[test]
    fn transfer_does_not_advance_without_expected_acknowledgement() -> Result<(), Box<dyn Error>> {
        let firmware = [0x5a; 513];
        let image = image(&firmware)?;
        let mut transfer = DfuTransfer::new(image, DfuBankLayout::Dual);
        let first = transfer
            .next_frame()?
            .ok_or(super::TransferError::AlreadyComplete)?;
        let first_bytes = first.frame().bytes().to_vec();
        let expected = first.frame().expected_acknowledgement();
        assert_eq!(transfer.state(), TransferState::AwaitingAcknowledgement);

        let retry = transfer
            .next_frame()?
            .ok_or(super::TransferError::AlreadyComplete)?;
        assert_eq!(retry.frame().bytes(), first_bytes);
        assert_eq!(transfer.acknowledge(expected), Ok(TransferState::Ready));
        Ok(())
    }

    #[test]
    fn complete_transfer_has_start_init_data_and_stop_frames() -> Result<(), Box<dyn Error>> {
        let firmware = [0x5a; 513];
        let image = image(&firmware)?;
        let mut transfer = DfuTransfer::new(image, DfuBankLayout::Dual);
        let mut frames = 0;
        while transfer.state() != TransferState::Complete {
            let next = transfer
                .next_frame()?
                .ok_or(super::TransferError::AlreadyComplete)?;
            let acknowledgement = next.frame().expected_acknowledgement();
            frames += 1;
            assert!(transfer.acknowledge(acknowledgement).is_ok());
        }
        assert_eq!(frames, 5);
        Ok(())
    }

    #[test]
    fn reliable_attempts_are_bounded_per_pending_frame() -> Result<(), Box<dyn Error>> {
        let mut attempts = ReliableFrameAttempts::new();

        assert_eq!(attempts.begin(1)?.number(), 1);
        assert_eq!(attempts.begin(1)?.number(), 2);
        assert_eq!(attempts.begin(1)?.number(), 3);
        assert_eq!(
            attempts.begin(1),
            Err(ReliableFrameAttemptError::Exhausted {
                sequence_number: 1,
                attempts: 3,
            })
        );
        Ok(())
    }

    #[test]
    fn accepted_frame_resets_attempts_for_the_next_sequence() -> Result<(), Box<dyn Error>> {
        let mut attempts = ReliableFrameAttempts::new();

        assert_eq!(attempts.begin(1)?.number(), 1);
        assert_eq!(attempts.begin(1)?.number(), 2);
        attempts.accepted(1)?;
        assert_eq!(attempts.begin(2)?.number(), 1);
        Ok(())
    }

    #[test]
    fn pending_attempts_reject_a_different_sequence() -> Result<(), Box<dyn Error>> {
        let mut attempts = ReliableFrameAttempts::new();
        let _ = attempts.begin(1)?;

        assert_eq!(
            attempts.begin(2),
            Err(ReliableFrameAttemptError::FrameChanged {
                pending_sequence_number: 1,
                actual_sequence_number: 2,
            })
        );
        assert_eq!(
            attempts.accepted(2),
            Err(ReliableFrameAttemptError::FrameChanged {
                pending_sequence_number: 1,
                actual_sequence_number: 2,
            })
        );
        Ok(())
    }
}
