#![forbid(unsafe_code)]

use prns_nrf_dfu::{
    AcknowledgementDecoder, ApplicationInitPacketSpec, ApplicationVersion, DfuBankLayout,
    DfuDeviceRevision, DfuDeviceType, DfuImage, DfuImageError, DfuTransfer,
    ReliableFrameAttemptError, ReliableFrameAttempts, SoftdeviceFirmwareId, SoftdeviceRequirements,
    TransferError, TransferProgress, TransferState,
};
use thiserror::Error;
use wasm_bindgen::prelude::*;

const MICROSECONDS_PER_MILLISECOND: u128 = 1_000;

#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrfDfuBankLayout {
    Single,
    Dual,
}

impl From<NrfDfuBankLayout> for DfuBankLayout {
    fn from(value: NrfDfuBankLayout) -> Self {
        match value {
            NrfDfuBankLayout::Single => Self::Single,
            NrfDfuBankLayout::Dual => Self::Dual,
        }
    }
}

#[wasm_bindgen]
pub struct NrfDfuCompatibility {
    init_packet: ApplicationInitPacketSpec,
    bank_layout: DfuBankLayout,
}

#[wasm_bindgen]
impl NrfDfuCompatibility {
    #[wasm_bindgen(js_name = notEnforcedApplication)]
    pub fn not_enforced_application(
        device_type: u16,
        device_revision: u16,
        softdevice_fwids: Vec<u16>,
        bank_layout: NrfDfuBankLayout,
    ) -> Result<NrfDfuCompatibility, JsError> {
        Self::not_enforced(device_type, device_revision, softdevice_fwids, bank_layout)
            .map_err(js_error)
    }
}

impl NrfDfuCompatibility {
    fn not_enforced(
        device_type: u16,
        device_revision: u16,
        softdevice_fwids: Vec<u16>,
        bank_layout: NrfDfuBankLayout,
    ) -> Result<Self, NrfDfuAdapterError> {
        let mut fwids = softdevice_fwids.into_iter();
        let required = fwids
            .next()
            .ok_or(NrfDfuAdapterError::EmptySoftdeviceRequirements)?;
        let required = SoftdeviceFirmwareId::new(required)?;
        let additional = fwids
            .map(SoftdeviceFirmwareId::new)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            init_packet: ApplicationInitPacketSpec {
                device_type: DfuDeviceType::new(device_type),
                device_revision: DfuDeviceRevision::new(device_revision),
                application_version: ApplicationVersion::NotEnforced,
                softdevices: SoftdeviceRequirements::new(required, additional)?,
            },
            bank_layout: bank_layout.into(),
        })
    }
}

#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrfDfuSessionState {
    Ready,
    AwaitingAcknowledgement,
    RetryRequired,
    Complete,
    Failed,
}

#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrfDfuAcknowledgementTransitionKind {
    AwaitingMore,
    RetryRequired,
    FrameAccepted,
    TransferComplete,
}

#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrfDfuRetryReason {
    MalformedAcknowledgement,
    UnexpectedAcknowledgement,
}

#[wasm_bindgen]
#[derive(Debug, PartialEq, Eq)]
pub struct NrfDfuFrame {
    bytes: Vec<u8>,
    attempt: u8,
    attempt_limit: u8,
}

#[wasm_bindgen]
impl NrfDfuFrame {
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn attempt(&self) -> u8 {
        self.attempt
    }

    #[wasm_bindgen(getter, js_name = attemptLimit)]
    pub fn attempt_limit(&self) -> u8 {
        self.attempt_limit
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AcknowledgedFrame {
    wait_milliseconds: u32,
    progress: TransferProgress,
}

#[wasm_bindgen]
pub struct NrfDfuAcknowledgementTransition {
    kind: NrfDfuAcknowledgementTransitionKind,
    retry_reason: Option<NrfDfuRetryReason>,
    acknowledged: Option<AcknowledgedFrame>,
}

#[wasm_bindgen]
impl NrfDfuAcknowledgementTransition {
    #[wasm_bindgen(getter)]
    pub fn kind(&self) -> NrfDfuAcknowledgementTransitionKind {
        self.kind
    }

    #[wasm_bindgen(js_name = retryReason)]
    pub fn retry_reason(&self) -> Result<NrfDfuRetryReason, JsError> {
        self.retry_reason
            .ok_or(NrfDfuAdapterError::TransitionHasNoRetryReason)
            .map_err(js_error)
    }

    #[wasm_bindgen(getter, js_name = waitMilliseconds)]
    pub fn wait_milliseconds(&self) -> Result<u32, JsError> {
        self.acknowledged
            .map(|acknowledged| acknowledged.wait_milliseconds)
            .ok_or(NrfDfuAdapterError::TransitionHasNoAcknowledgedFrame)
            .map_err(js_error)
    }

    #[wasm_bindgen(getter, js_name = writtenBytes)]
    pub fn written_bytes(&self) -> Result<u32, JsError> {
        self.acknowledged
            .map(|acknowledged| acknowledged.progress.written_bytes())
            .ok_or(NrfDfuAdapterError::TransitionHasNoAcknowledgedFrame)
            .and_then(|bytes| {
                u32::try_from(bytes).map_err(|_| NrfDfuAdapterError::ProgressOutOfRange(bytes))
            })
            .map_err(js_error)
    }

    #[wasm_bindgen(getter, js_name = totalBytes)]
    pub fn total_bytes(&self) -> Result<u32, JsError> {
        self.acknowledged
            .map(|acknowledged| acknowledged.progress.total_bytes())
            .ok_or(NrfDfuAdapterError::TransitionHasNoAcknowledgedFrame)
            .and_then(|bytes| {
                u32::try_from(bytes).map_err(|_| NrfDfuAdapterError::ProgressOutOfRange(bytes))
            })
            .map_err(js_error)
    }
}

impl NrfDfuAcknowledgementTransition {
    const fn awaiting_more() -> Self {
        Self {
            kind: NrfDfuAcknowledgementTransitionKind::AwaitingMore,
            retry_reason: None,
            acknowledged: None,
        }
    }

    const fn retry_required(reason: NrfDfuRetryReason) -> Self {
        Self {
            kind: NrfDfuAcknowledgementTransitionKind::RetryRequired,
            retry_reason: Some(reason),
            acknowledged: None,
        }
    }

    const fn acknowledged(
        kind: NrfDfuAcknowledgementTransitionKind,
        acknowledged: AcknowledgedFrame,
    ) -> Self {
        Self {
            kind,
            retry_reason: None,
            acknowledged: Some(acknowledged),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionCondition {
    Active,
    RetryRequired,
    Failed,
}

#[wasm_bindgen]
pub struct NrfDfuSession {
    transfer: DfuTransfer<'static>,
    decoder: AcknowledgementDecoder,
    attempts: ReliableFrameAttempts,
    condition: SessionCondition,
}

#[wasm_bindgen]
impl NrfDfuSession {
    #[wasm_bindgen(constructor)]
    pub fn new(
        firmware: Vec<u8>,
        init_packet: Vec<u8>,
        compatibility: &NrfDfuCompatibility,
    ) -> Result<NrfDfuSession, JsError> {
        Self::from_artifacts(firmware, init_packet, compatibility).map_err(js_error)
    }

    #[wasm_bindgen(getter)]
    pub fn state(&self) -> NrfDfuSessionState {
        self.session_state()
    }

    #[wasm_bindgen(js_name = nextFrame)]
    pub fn next_frame(&mut self) -> Result<NrfDfuFrame, JsError> {
        self.begin_next_frame().map_err(js_error)
    }

    #[wasm_bindgen(js_name = retryFrame)]
    pub fn retry_frame(&mut self) -> Result<NrfDfuFrame, JsError> {
        self.begin_retry().map_err(js_error)
    }

    #[wasm_bindgen(js_name = pushAcknowledgement)]
    pub fn push_acknowledgement(
        &mut self,
        bytes: Vec<u8>,
    ) -> Result<NrfDfuAcknowledgementTransition, JsError> {
        self.decode_acknowledgement(&bytes).map_err(js_error)
    }
}

impl NrfDfuSession {
    fn from_artifacts(
        firmware: Vec<u8>,
        init_packet: Vec<u8>,
        compatibility: &NrfDfuCompatibility,
    ) -> Result<Self, NrfDfuAdapterError> {
        let image =
            DfuImage::from_owned_artifacts(firmware, init_packet, &compatibility.init_packet)?;
        Ok(Self {
            transfer: DfuTransfer::new(image, compatibility.bank_layout),
            decoder: AcknowledgementDecoder::new(),
            attempts: ReliableFrameAttempts::new(),
            condition: SessionCondition::Active,
        })
    }

    fn session_state(&self) -> NrfDfuSessionState {
        match self.condition {
            SessionCondition::RetryRequired => return NrfDfuSessionState::RetryRequired,
            SessionCondition::Failed => return NrfDfuSessionState::Failed,
            SessionCondition::Active => {}
        }
        match self.transfer.state() {
            TransferState::Ready => NrfDfuSessionState::Ready,
            TransferState::AwaitingAcknowledgement => NrfDfuSessionState::AwaitingAcknowledgement,
            TransferState::Complete => NrfDfuSessionState::Complete,
        }
    }

    fn begin_next_frame(&mut self) -> Result<NrfDfuFrame, NrfDfuAdapterError> {
        match self.session_state() {
            NrfDfuSessionState::Ready => self.begin_pending_attempt(),
            NrfDfuSessionState::AwaitingAcknowledgement => {
                Err(NrfDfuAdapterError::FrameAlreadyAwaitingAcknowledgement)
            }
            NrfDfuSessionState::RetryRequired => Err(NrfDfuAdapterError::FrameRetryRequired),
            NrfDfuSessionState::Complete => Err(NrfDfuAdapterError::SessionComplete),
            NrfDfuSessionState::Failed => Err(NrfDfuAdapterError::SessionFailed),
        }
    }

    fn begin_retry(&mut self) -> Result<NrfDfuFrame, NrfDfuAdapterError> {
        match self.session_state() {
            NrfDfuSessionState::AwaitingAcknowledgement | NrfDfuSessionState::RetryRequired => {
                self.decoder = AcknowledgementDecoder::new();
                self.condition = SessionCondition::Active;
                self.begin_pending_attempt()
            }
            NrfDfuSessionState::Ready => Err(NrfDfuAdapterError::NoFrameToRetry),
            NrfDfuSessionState::Complete => Err(NrfDfuAdapterError::SessionComplete),
            NrfDfuSessionState::Failed => Err(NrfDfuAdapterError::SessionFailed),
        }
    }

    fn begin_pending_attempt(&mut self) -> Result<NrfDfuFrame, NrfDfuAdapterError> {
        let (sequence_number, bytes) = {
            let pending = self
                .transfer
                .next_frame()?
                .ok_or(NrfDfuAdapterError::SessionComplete)?;
            (
                pending.frame().sequence_number(),
                pending.frame().bytes().to_vec(),
            )
        };
        let attempt = match self.attempts.begin(sequence_number) {
            Ok(attempt) => attempt,
            Err(error) => {
                self.condition = SessionCondition::Failed;
                return Err(error.into());
            }
        };
        Ok(NrfDfuFrame {
            bytes,
            attempt: attempt.number(),
            attempt_limit: attempt.limit(),
        })
    }

    fn decode_acknowledgement(
        &mut self,
        bytes: &[u8],
    ) -> Result<NrfDfuAcknowledgementTransition, NrfDfuAdapterError> {
        match self.session_state() {
            NrfDfuSessionState::AwaitingAcknowledgement => {}
            NrfDfuSessionState::RetryRequired => {
                return Err(NrfDfuAdapterError::FrameRetryRequired);
            }
            NrfDfuSessionState::Ready => {
                return Err(NrfDfuAdapterError::NoFrameAwaitingAcknowledgement);
            }
            NrfDfuSessionState::Complete => return Err(NrfDfuAdapterError::SessionComplete),
            NrfDfuSessionState::Failed => return Err(NrfDfuAdapterError::SessionFailed),
        }

        for (offset, byte) in bytes.iter().enumerate() {
            let acknowledgement = match self.decoder.push(*byte) {
                Ok(acknowledgement) => acknowledgement,
                Err(_) => {
                    self.decoder = AcknowledgementDecoder::new();
                    self.condition = SessionCondition::RetryRequired;
                    return Ok(NrfDfuAcknowledgementTransition::retry_required(
                        NrfDfuRetryReason::MalformedAcknowledgement,
                    ));
                }
            };
            let Some(acknowledgement) = acknowledgement else {
                continue;
            };
            if offset + 1 != bytes.len() {
                self.condition = SessionCondition::Failed;
                return Err(NrfDfuAdapterError::TrailingAcknowledgementBytes);
            }
            return self.accept_acknowledgement(acknowledgement);
        }
        Ok(NrfDfuAcknowledgementTransition::awaiting_more())
    }

    fn accept_acknowledgement(
        &mut self,
        acknowledgement: prns_nrf_dfu::Acknowledgement,
    ) -> Result<NrfDfuAcknowledgementTransition, NrfDfuAdapterError> {
        let (sequence_number, wait, progress) = {
            let pending = self
                .transfer
                .next_frame()?
                .ok_or(NrfDfuAdapterError::SessionComplete)?;
            (
                pending.frame().sequence_number(),
                pending.wait_after_acknowledgement(),
                pending.progress_after_acknowledgement(),
            )
        };
        let state = match self.transfer.acknowledge(acknowledgement) {
            Ok(state) => state,
            Err(TransferError::UnexpectedAcknowledgement { .. }) => {
                self.decoder = AcknowledgementDecoder::new();
                self.condition = SessionCondition::RetryRequired;
                return Ok(NrfDfuAcknowledgementTransition::retry_required(
                    NrfDfuRetryReason::UnexpectedAcknowledgement,
                ));
            }
            Err(error) => {
                self.condition = SessionCondition::Failed;
                return Err(error.into());
            }
        };
        if let Err(error) = self.attempts.accepted(sequence_number) {
            self.condition = SessionCondition::Failed;
            return Err(error.into());
        }
        self.decoder = AcknowledgementDecoder::new();
        let kind = match state {
            TransferState::Ready => NrfDfuAcknowledgementTransitionKind::FrameAccepted,
            TransferState::Complete => NrfDfuAcknowledgementTransitionKind::TransferComplete,
            TransferState::AwaitingAcknowledgement => {
                self.condition = SessionCondition::Failed;
                return Err(NrfDfuAdapterError::AcknowledgementDidNotAdvance);
            }
        };
        Ok(NrfDfuAcknowledgementTransition::acknowledged(
            kind,
            AcknowledgedFrame {
                wait_milliseconds: wait_milliseconds(wait)?,
                progress,
            },
        ))
    }
}

fn wait_milliseconds(wait: prns_nrf_dfu::RequiredWait) -> Result<u32, NrfDfuAdapterError> {
    let milliseconds = wait
        .duration()
        .as_micros()
        .div_ceil(MICROSECONDS_PER_MILLISECOND);
    u32::try_from(milliseconds).map_err(|_| NrfDfuAdapterError::WaitOutOfRange(milliseconds))
}

fn js_error(error: NrfDfuAdapterError) -> JsError {
    JsError::new(&error.to_string())
}

#[derive(Debug, Error, PartialEq, Eq)]
enum NrfDfuAdapterError {
    #[error(transparent)]
    Image(#[from] DfuImageError),
    #[error(transparent)]
    Transfer(#[from] TransferError),
    #[error(transparent)]
    Attempt(#[from] ReliableFrameAttemptError),
    #[error("DFU compatibility requires at least one exact SoftDevice FWID")]
    EmptySoftdeviceRequirements,
    #[error("a DFU frame is already awaiting acknowledgement")]
    FrameAlreadyAwaitingAcknowledgement,
    #[error("the pending DFU frame must be retransmitted before accepting acknowledgement bytes")]
    FrameRetryRequired,
    #[error("no DFU frame is available to retry")]
    NoFrameToRetry,
    #[error("no DFU frame is awaiting acknowledgement bytes")]
    NoFrameAwaitingAcknowledgement,
    #[error("DFU acknowledgement bytes continue after a complete frame")]
    TrailingAcknowledgementBytes,
    #[error("DFU acknowledgement did not advance the transfer")]
    AcknowledgementDidNotAdvance,
    #[error("DFU transfer is complete")]
    SessionComplete,
    #[error("DFU transfer has failed closed")]
    SessionFailed,
    #[error("DFU transition has no retry reason")]
    TransitionHasNoRetryReason,
    #[error("DFU transition has no acknowledged frame")]
    TransitionHasNoAcknowledgedFrame,
    #[error("DFU wait of {0} milliseconds exceeds the browser adapter range")]
    WaitOutOfRange(u128),
    #[error("DFU progress value {0} exceeds the browser adapter range")]
    ProgressOutOfRange(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compatibility() -> Result<NrfDfuCompatibility, NrfDfuAdapterError> {
        NrfDfuCompatibility::not_enforced(0x0052, 52840, vec![0x0123], NrfDfuBankLayout::Single)
    }

    fn session() -> Result<NrfDfuSession, NrfDfuAdapterError> {
        NrfDfuSession::from_artifacts(
            vec![1, 2, 3],
            vec![
                0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad, 0xad,
            ],
            &compatibility()?,
        )
    }

    #[test]
    fn validated_session_exposes_frames_and_rust_owned_transitions(
    ) -> Result<(), NrfDfuAdapterError> {
        let mut session = session()?;
        let frame = session.begin_next_frame()?;
        assert_eq!(frame.attempt, 1);
        assert_eq!(frame.attempt_limit, 3);
        assert_eq!(
            session.session_state(),
            NrfDfuSessionState::AwaitingAcknowledgement
        );

        let partial = session.decode_acknowledgement(&[0xc0, 0x10])?;
        assert_eq!(
            partial.kind,
            NrfDfuAcknowledgementTransitionKind::AwaitingMore
        );
        let accepted = session.decode_acknowledgement(&[0x00, 0x00, 0xf0, 0xc0])?;
        assert_eq!(
            accepted.kind,
            NrfDfuAcknowledgementTransitionKind::FrameAccepted
        );
        assert_eq!(
            accepted.acknowledged.map(|value| value.wait_milliseconds),
            Some(500)
        );
        assert_eq!(
            accepted
                .acknowledged
                .map(|value| (value.progress.written_bytes(), value.progress.total_bytes())),
            Some((0, 3))
        );
        assert_eq!(session.session_state(), NrfDfuSessionState::Ready);
        Ok(())
    }

    #[test]
    fn malformed_acknowledgement_requires_a_bounded_retry() -> Result<(), NrfDfuAdapterError> {
        let mut session = session()?;
        let first = session.begin_next_frame()?;
        let retry = session.decode_acknowledgement(&[0xc0, 0x10, 0x00, 0x00, 0x00, 0xc0])?;
        assert_eq!(
            retry.kind,
            NrfDfuAcknowledgementTransitionKind::RetryRequired
        );
        assert_eq!(
            retry.retry_reason,
            Some(NrfDfuRetryReason::MalformedAcknowledgement)
        );
        assert_eq!(session.session_state(), NrfDfuSessionState::RetryRequired);
        assert!(matches!(
            session.decode_acknowledgement(&[0xc0]),
            Err(NrfDfuAdapterError::FrameRetryRequired)
        ));

        let second = session.begin_retry()?;
        let third = session.begin_retry()?;
        assert_eq!(second.bytes, first.bytes);
        assert_eq!(third.bytes, first.bytes);
        assert_eq!(second.attempt, 2);
        assert_eq!(third.attempt, 3);
        assert_eq!(
            session.begin_retry(),
            Err(NrfDfuAdapterError::Attempt(
                ReliableFrameAttemptError::Exhausted {
                    sequence_number: 1,
                    attempts: 3,
                }
            ))
        );
        assert_eq!(session.session_state(), NrfDfuSessionState::Failed);
        Ok(())
    }

    #[test]
    fn mismatched_owned_artifacts_never_create_a_session() -> Result<(), NrfDfuAdapterError> {
        assert_eq!(
            NrfDfuSession::from_artifacts(
                vec![1, 2, 4],
                vec![
                    0x52, 0x00, 0x68, 0xce, 0xff, 0xff, 0xff, 0xff, 0x01, 0x00, 0x23, 0x01, 0xad,
                    0xad,
                ],
                &compatibility()?,
            )
            .map(|_| ()),
            Err(NrfDfuAdapterError::Image(
                DfuImageError::InitPacketFirmwareMismatch {
                    init_packet_crc: 0xadad,
                    firmware_crc: 0xdd4a,
                }
            ))
        );
        Ok(())
    }
}
