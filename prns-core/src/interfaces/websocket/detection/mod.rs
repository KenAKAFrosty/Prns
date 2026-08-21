#[cfg(feature = "alloc")]
use super::WebSocketWireDecoder;
use super::{WebSocketWireFraming, WebSocketWireFramingParseError, FRAME_CAP};
use crate::interfaces::kiss_framing;
#[cfg(feature = "alloc")]
use crate::interfaces::rns_serial_framing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketFramingSelection {
    Auto,
    Fixed(WebSocketWireFraming),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketFramingSelectionParseError {
    UnknownSelection,
}

impl WebSocketFramingSelection {
    pub fn from_name(name: &str) -> Result<Self, WebSocketFramingSelectionParseError> {
        if name == Self::Auto.name() {
            return Ok(Self::Auto);
        }
        WebSocketWireFraming::from_name(name)
            .map(Self::Fixed)
            .map_err(|error| match error {
                WebSocketWireFramingParseError::UnknownFraming => {
                    WebSocketFramingSelectionParseError::UnknownSelection
                }
            })
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed(framing) => framing.name(),
        }
    }

    #[must_use]
    pub const fn channel_tag_suffix(self) -> &'static [u8] {
        match self {
            Self::Auto => b"\0auto",
            Self::Fixed(framing) => framing.channel_tag_suffix(),
        }
    }

    #[must_use]
    pub const fn message_cap(self) -> usize {
        match self {
            Self::Auto => kiss_framing::max_encoded_len(FRAME_CAP),
            Self::Fixed(framing) => framing.message_cap(),
        }
    }
}

#[cfg(feature = "alloc")]
mod allocated {
    use super::*;
    use crate::interfaces::{FrameSink, FrameSinkError};
    use crate::wire::{
        wire_hop_count_is_valid, DestinationType, IfacFlag, PacketType, PropagationType,
        WirePacketHeader,
    };
    use alloc::vec::Vec;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum WebSocketWireDetection {
        AwaitingEvidence,
        AmbiguousEvidence,
        Detected(DecodedWebSocketFrame),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct DecodedWebSocketFrame {
        framing: WebSocketWireFraming,
        frame_len: usize,
        consumed_message_bytes: usize,
    }

    impl DecodedWebSocketFrame {
        #[must_use]
        pub const fn framing(self) -> WebSocketWireFraming {
            self.framing
        }

        #[must_use]
        pub const fn frame_len(self) -> usize {
            self.frame_len
        }

        #[must_use]
        pub const fn consumed_message_bytes(self) -> usize {
            self.consumed_message_bytes
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum WebSocketFrameDecodeOutcome {
        Incomplete,
        AmbiguousFraming,
        Frame(DecodedWebSocketFrame),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum WebSocketFramingState {
        Detecting,
        Resolved(WebSocketWireFraming),
    }

    pub struct WebSocketFramingDecoder {
        state: FramingDecoderState,
    }

    enum FramingDecoderState {
        AutoDetecting(WebSocketWireDetector),
        AutoResolved(WebSocketWireDecoder),
        Fixed(WebSocketWireDecoder),
    }

    impl WebSocketFramingDecoder {
        #[must_use]
        pub const fn new(selection: WebSocketFramingSelection) -> Self {
            let state = match selection {
                WebSocketFramingSelection::Auto => {
                    FramingDecoderState::AutoDetecting(WebSocketWireDetector::new())
                }
                WebSocketFramingSelection::Fixed(framing) => {
                    FramingDecoderState::Fixed(WebSocketWireDecoder::new(framing))
                }
            };
            Self { state }
        }

        #[must_use]
        pub const fn state(&self) -> WebSocketFramingState {
            match &self.state {
                FramingDecoderState::AutoDetecting(_) => WebSocketFramingState::Detecting,
                FramingDecoderState::AutoResolved(decoder)
                | FramingDecoderState::Fixed(decoder) => {
                    WebSocketFramingState::Resolved(decoder.framing())
                }
            }
        }

        pub fn reset_connection(&mut self) {
            match &mut self.state {
                FramingDecoderState::AutoDetecting(detector) => detector.reset(),
                FramingDecoderState::AutoResolved(_) => {
                    self.state = FramingDecoderState::AutoDetecting(WebSocketWireDetector::new());
                }
                FramingDecoderState::Fixed(decoder) => decoder.reset(),
            }
        }

        pub fn next_frame_into(
            &mut self,
            input: &[u8],
            offset: &mut usize,
            sink: &mut dyn FrameSink,
        ) -> Result<WebSocketFrameDecodeOutcome, super::super::DecodeError> {
            if *offset >= input.len() {
                *offset = input.len();
                return Ok(WebSocketFrameDecodeOutcome::Incomplete);
            }
            match &mut self.state {
                FramingDecoderState::AutoDetecting(detector) => {
                    let message_start = *offset;
                    let detection = detector.inspect_message(&input[message_start..], sink)?;
                    match detection {
                        WebSocketWireDetection::AwaitingEvidence => {
                            *offset = input.len();
                            Ok(WebSocketFrameDecodeOutcome::Incomplete)
                        }
                        WebSocketWireDetection::AmbiguousEvidence => {
                            *offset = input.len();
                            Ok(WebSocketFrameDecodeOutcome::AmbiguousFraming)
                        }
                        WebSocketWireDetection::Detected(frame) => {
                            *offset += frame.consumed_message_bytes();
                            let framing = frame.framing();
                            self.state = FramingDecoderState::AutoResolved(
                                WebSocketWireDecoder::new(framing),
                            );
                            Ok(WebSocketFrameDecodeOutcome::Frame(DecodedWebSocketFrame {
                                framing,
                                frame_len: frame.frame_len(),
                                consumed_message_bytes: frame.consumed_message_bytes(),
                            }))
                        }
                    }
                }
                FramingDecoderState::AutoResolved(decoder)
                | FramingDecoderState::Fixed(decoder) => {
                    let framing = decoder.framing();
                    let message_start = *offset;
                    match decoder.next_frame_into(input, offset, sink)? {
                        Some(frame_len) => {
                            Ok(WebSocketFrameDecodeOutcome::Frame(DecodedWebSocketFrame {
                                framing,
                                frame_len,
                                consumed_message_bytes: *offset - message_start,
                            }))
                        }
                        None => Ok(WebSocketFrameDecodeOutcome::Incomplete),
                    }
                }
            }
        }
    }

    enum PendingOutbound {
        Empty,
        Frame(Vec<u8>),
    }

    enum DetectingOutbound {
        AwaitingEvidence(PendingOutbound),
        ProvisionalRaw,
    }

    enum SessionFramingState {
        Detecting {
            decoder: WebSocketFramingDecoder,
            outbound: DetectingOutbound,
        },
        Ready(WebSocketWireDecoder),
    }

    pub struct WebSocketOutboundRelease {
        framing: WebSocketWireFraming,
        pending: PendingOutbound,
    }

    impl WebSocketOutboundRelease {
        #[must_use]
        pub const fn framing(&self) -> WebSocketWireFraming {
            self.framing
        }

        #[must_use]
        pub fn pending_packet(&self) -> Option<&[u8]> {
            match &self.pending {
                PendingOutbound::Empty => None,
                PendingOutbound::Frame(packet) => Some(packet),
            }
        }
    }

    pub enum WebSocketSessionFrameDecodeOutcome {
        Incomplete,
        AmbiguousFraming,
        Frame,
        ResolvedFrame(WebSocketOutboundRelease),
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum WebSocketSessionOutboundAction {
        Queued,
        Send(WebSocketWireFraming),
        Rejected,
        Backpressured,
    }

    pub struct WebSocketSessionFraming {
        state: SessionFramingState,
    }

    impl WebSocketSessionFraming {
        #[must_use]
        pub const fn new(selection: WebSocketFramingSelection) -> Self {
            let state = match selection {
                WebSocketFramingSelection::Auto => SessionFramingState::Detecting {
                    decoder: WebSocketFramingDecoder::new(WebSocketFramingSelection::Auto),
                    outbound: DetectingOutbound::AwaitingEvidence(PendingOutbound::Empty),
                },
                WebSocketFramingSelection::Fixed(framing) => {
                    SessionFramingState::Ready(WebSocketWireDecoder::new(framing))
                }
            };
            Self { state }
        }

        #[must_use]
        pub const fn raw_fallback_is_armed(&self) -> bool {
            matches!(
                &self.state,
                SessionFramingState::Detecting {
                    outbound: DetectingOutbound::AwaitingEvidence(PendingOutbound::Frame(_)),
                    ..
                }
            )
        }

        #[must_use]
        pub const fn is_detecting(&self) -> bool {
            matches!(&self.state, SessionFramingState::Detecting { .. })
        }

        #[must_use]
        pub const fn can_read_outbound(&self) -> bool {
            matches!(
                &self.state,
                SessionFramingState::Detecting {
                    outbound: DetectingOutbound::AwaitingEvidence(PendingOutbound::Empty),
                    ..
                } | SessionFramingState::Detecting {
                    outbound: DetectingOutbound::ProvisionalRaw,
                    ..
                } | SessionFramingState::Ready(_)
            )
        }

        #[must_use]
        pub const fn can_stage_multiple_outbound(&self) -> bool {
            matches!(
                &self.state,
                SessionFramingState::Detecting {
                    outbound: DetectingOutbound::ProvisionalRaw,
                    ..
                } | SessionFramingState::Ready(_)
            )
        }

        #[must_use]
        pub const fn has_pending_outbound(&self) -> bool {
            self.raw_fallback_is_armed()
        }

        pub fn stage_outbound(&mut self, packet: &[u8]) -> WebSocketSessionOutboundAction {
            match &mut self.state {
                SessionFramingState::Detecting { outbound, .. } => match outbound {
                    DetectingOutbound::AwaitingEvidence(pending_outbound) => match pending_outbound
                    {
                        PendingOutbound::Empty
                            if !packet.is_empty() && packet.len() <= FRAME_CAP =>
                        {
                            *pending_outbound = PendingOutbound::Frame(packet.to_vec());
                            WebSocketSessionOutboundAction::Queued
                        }
                        PendingOutbound::Empty => WebSocketSessionOutboundAction::Rejected,
                        PendingOutbound::Frame(_) => WebSocketSessionOutboundAction::Backpressured,
                    },
                    DetectingOutbound::ProvisionalRaw => {
                        WebSocketSessionOutboundAction::Send(WebSocketWireFraming::RawPacket)
                    }
                },
                SessionFramingState::Ready(decoder) => {
                    WebSocketSessionOutboundAction::Send(decoder.framing())
                }
            }
        }

        pub fn release_raw_fallback(&mut self) -> Option<WebSocketOutboundRelease> {
            let SessionFramingState::Detecting { outbound, .. } = &mut self.state else {
                return None;
            };
            let pending = match outbound {
                DetectingOutbound::AwaitingEvidence(pending) => match pending {
                    PendingOutbound::Empty => return None,
                    PendingOutbound::Frame(_) => {
                        core::mem::replace(pending, PendingOutbound::Empty)
                    }
                },
                DetectingOutbound::ProvisionalRaw => return None,
            };
            *outbound = DetectingOutbound::ProvisionalRaw;
            Some(WebSocketOutboundRelease {
                framing: WebSocketWireFraming::RawPacket,
                pending,
            })
        }

        pub fn next_frame_into(
            &mut self,
            input: &[u8],
            offset: &mut usize,
            sink: &mut dyn FrameSink,
        ) -> Result<WebSocketSessionFrameDecodeOutcome, super::super::DecodeError> {
            match &mut self.state {
                SessionFramingState::Ready(decoder) => decoder
                    .next_frame_into(input, offset, sink)
                    .map(|frame| match frame {
                        Some(_) => WebSocketSessionFrameDecodeOutcome::Frame,
                        None => WebSocketSessionFrameDecodeOutcome::Incomplete,
                    }),
                SessionFramingState::Detecting { decoder, .. } => {
                    match decoder.next_frame_into(input, offset, sink)? {
                        WebSocketFrameDecodeOutcome::Incomplete => {
                            Ok(WebSocketSessionFrameDecodeOutcome::Incomplete)
                        }
                        WebSocketFrameDecodeOutcome::AmbiguousFraming => {
                            Ok(WebSocketSessionFrameDecodeOutcome::AmbiguousFraming)
                        }
                        WebSocketFrameDecodeOutcome::Frame(frame) => {
                            let resolution = self.resolve(frame.framing());
                            Ok(WebSocketSessionFrameDecodeOutcome::ResolvedFrame(
                                resolution,
                            ))
                        }
                    }
                }
            }
        }

        fn resolve(&mut self, framing: WebSocketWireFraming) -> WebSocketOutboundRelease {
            let pending = match &mut self.state {
                SessionFramingState::Detecting { outbound, .. } => match outbound {
                    DetectingOutbound::AwaitingEvidence(pending) => {
                        core::mem::replace(pending, PendingOutbound::Empty)
                    }
                    DetectingOutbound::ProvisionalRaw => PendingOutbound::Empty,
                },
                SessionFramingState::Ready(_) => PendingOutbound::Empty,
            };
            self.state = SessionFramingState::Ready(WebSocketWireDecoder::new(framing));
            WebSocketOutboundRelease { framing, pending }
        }
    }

    pub(super) struct WebSocketWireDetector {
        hdlc_scanner: rns_serial_framing::RnsSerialScanner,
        hdlc_frame: DetectionFrame,
        hdlc_needs_opening_flag: bool,
        kiss_scanner: kiss_framing::KissScanner,
        kiss_frame: DetectionFrame,
    }

    impl Default for WebSocketWireDetector {
        fn default() -> Self {
            Self::new()
        }
    }

    impl WebSocketWireDetector {
        #[must_use]
        pub const fn new() -> Self {
            Self {
                hdlc_scanner: rns_serial_framing::RnsSerialScanner::new(),
                hdlc_frame: DetectionFrame::new(),
                hdlc_needs_opening_flag: true,
                kiss_scanner: kiss_framing::KissScanner::new(),
                kiss_frame: DetectionFrame::new(),
            }
        }

        pub fn reset(&mut self) {
            self.hdlc_scanner.reset();
            self.hdlc_frame.clear();
            self.hdlc_needs_opening_flag = true;
            self.kiss_scanner.reset();
            self.kiss_frame.clear();
        }

        pub fn inspect_message(
            &mut self,
            message: &[u8],
            sink: &mut dyn FrameSink,
        ) -> Result<WebSocketWireDetection, super::super::DecodeError> {
            sink.clear();
            let raw = is_packet_evidence(message);
            let hdlc = next_hdlc_evidence(
                &mut self.hdlc_scanner,
                &mut self.hdlc_frame,
                &mut self.hdlc_needs_opening_flag,
                message,
            );
            let kiss = next_kiss_evidence(&mut self.kiss_scanner, &mut self.kiss_frame, message);
            match (raw, hdlc, kiss) {
                (false, None, None) => Ok(WebSocketWireDetection::AwaitingEvidence),
                (true, None, None) => detected(
                    WebSocketWireFraming::RawPacket,
                    message.len(),
                    message,
                    sink,
                ),
                (false, Some(consumed), None) => detected(
                    WebSocketWireFraming::Hdlc,
                    consumed,
                    self.hdlc_frame.as_slice(),
                    sink,
                ),
                (false, None, Some(consumed)) => detected(
                    WebSocketWireFraming::Kiss,
                    consumed,
                    self.kiss_frame.as_slice(),
                    sink,
                ),
                (true, Some(_), None)
                | (true, None, Some(_))
                | (false, Some(_), Some(_))
                | (true, Some(_), Some(_)) => Ok(WebSocketWireDetection::AmbiguousEvidence),
            }
        }
    }

    struct DetectionFrame {
        bytes: Vec<u8>,
    }

    impl DetectionFrame {
        const fn new() -> Self {
            Self { bytes: Vec::new() }
        }

        fn as_slice(&self) -> &[u8] {
            &self.bytes
        }
    }

    impl FrameSink for DetectionFrame {
        fn clear(&mut self) {
            self.bytes.clear();
        }

        fn frame_len(&self) -> usize {
            self.bytes.len()
        }

        fn free_capacity(&self) -> usize {
            FRAME_CAP.saturating_sub(self.bytes.len())
        }

        fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
            if self.bytes.len() >= FRAME_CAP {
                return Err(FrameSinkError::Full);
            }
            self.bytes.push(byte);
            Ok(())
        }

        fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
            if run.len() > FRAME_CAP.saturating_sub(self.bytes.len()) {
                return Err(FrameSinkError::Full);
            }
            self.bytes.extend_from_slice(run);
            Ok(())
        }
    }

    fn next_hdlc_evidence(
        scanner: &mut rns_serial_framing::RnsSerialScanner,
        frame: &mut DetectionFrame,
        needs_opening_flag: &mut bool,
        message: &[u8],
    ) -> Option<usize> {
        let mut offset = 0;
        while offset < message.len() {
            if *needs_opening_flag {
                let flag_offset = message[offset..]
                    .iter()
                    .position(|byte| *byte == rns_serial_framing::FLAG)?;
                offset += flag_offset;
            }
            match scanner.next_frame_into(message, &mut offset, frame) {
                Ok(Some(_)) => {
                    *needs_opening_flag = true;
                    if is_packet_evidence(frame.as_slice()) {
                        return Some(offset);
                    }
                }
                Err(rns_serial_framing::DecodeError::FrameTooBig) => {
                    *needs_opening_flag = true;
                }
                Ok(None) => {
                    *needs_opening_flag = false;
                    return None;
                }
            }
        }
        None
    }

    fn next_kiss_evidence(
        scanner: &mut kiss_framing::KissScanner,
        frame: &mut DetectionFrame,
        message: &[u8],
    ) -> Option<usize> {
        let mut offset = 0;
        while offset < message.len() {
            match scanner.next_frame_into(message, &mut offset, frame) {
                Ok(Some(_)) if is_packet_evidence(frame.as_slice()) => return Some(offset),
                Ok(Some(_)) | Err(kiss_framing::DecodeError::FrameTooBig) => {}
                Ok(None) => return None,
            }
        }
        None
    }

    fn is_packet_evidence(bytes: &[u8]) -> bool {
        if bytes.is_empty() || bytes.len() > FRAME_CAP {
            return false;
        }
        let Ok((header, _)) = WirePacketHeader::parse(bytes) else {
            return false;
        };
        if header.ifac_flag == IfacFlag::Authenticated || !wire_hop_count_is_valid(header.hops) {
            return false;
        }
        let is_transported = header.propagation == PropagationType::Transport;
        if header.transport_id.is_some() != is_transported {
            return false;
        }
        header.packet_type != PacketType::LinkRequest
            || header.destination_type == DestinationType::Single
    }

    fn detected(
        framing: WebSocketWireFraming,
        consumed_message_bytes: usize,
        frame: &[u8],
        sink: &mut dyn FrameSink,
    ) -> Result<WebSocketWireDetection, super::super::DecodeError> {
        sink.clear();
        if sink.extend_from_slice(frame).is_err() {
            sink.clear();
            return Err(super::super::DecodeError::FrameTooBig);
        }
        Ok(WebSocketWireDetection::Detected(DecodedWebSocketFrame {
            framing,
            frame_len: frame.len(),
            consumed_message_bytes,
        }))
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests;

#[cfg(feature = "alloc")]
pub use allocated::{
    DecodedWebSocketFrame, WebSocketFrameDecodeOutcome, WebSocketFramingDecoder,
    WebSocketFramingState, WebSocketOutboundRelease, WebSocketSessionFrameDecodeOutcome,
    WebSocketSessionFraming, WebSocketSessionOutboundAction,
};
