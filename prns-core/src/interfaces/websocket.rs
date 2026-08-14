use crate::interfaces::{kiss_framing, rns_serial_framing};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, FrameSink, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability, IFAC_MAX_SIZE,
    TRAVERSED_NETWORK_BITRATE_ESTIMATE,
};
use crate::routing::links::MAX_LINK_MTU;

pub const WEBSOCKET_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;

pub const WEBSOCKET_HW_MTU_CAP: usize = MAX_LINK_MTU;
pub const FRAME_CAP: usize = MAX_LINK_MTU + IFAC_MAX_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketWireFraming {
    RawPacket,
    Hdlc,
    Kiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebSocketWireFramingParseError {
    UnknownFraming,
}

impl WebSocketWireFraming {
    pub fn from_name(name: &str) -> Result<Self, WebSocketWireFramingParseError> {
        match name {
            "raw" => Ok(Self::RawPacket),
            "hdlc" => Ok(Self::Hdlc),
            "kiss" => Ok(Self::Kiss),
            _ => Err(WebSocketWireFramingParseError::UnknownFraming),
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RawPacket => "raw",
            Self::Hdlc => "hdlc",
            Self::Kiss => "kiss",
        }
    }

    #[must_use]
    pub const fn channel_tag_suffix(self) -> &'static [u8] {
        match self {
            Self::RawPacket => b"\0raw",
            Self::Hdlc => b"\0hdlc",
            Self::Kiss => b"\0kiss",
        }
    }

    #[must_use]
    pub const fn message_cap(self) -> usize {
        match self {
            Self::RawPacket => FRAME_CAP,
            Self::Hdlc => rns_serial_framing::max_encoded_len(FRAME_CAP),
            Self::Kiss => kiss_framing::max_encoded_len(FRAME_CAP),
        }
    }

    pub fn encode(self, input: &[u8], output: &mut [u8]) -> Result<usize, EncodeError> {
        if input.is_empty() || input.len() > FRAME_CAP {
            return Err(EncodeError::InvalidPacketLength);
        }
        match self {
            Self::RawPacket => {
                if output.len() < input.len() {
                    return Err(EncodeError::OutputTooSmall);
                }
                output[..input.len()].copy_from_slice(input);
                Ok(input.len())
            }
            Self::Hdlc => {
                rns_serial_framing::encode(input, output).map_err(|_| EncodeError::OutputTooSmall)
            }
            Self::Kiss => {
                kiss_framing::encode(input, output).map_err(|_| EncodeError::OutputTooSmall)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    InvalidPacketLength,
    OutputTooSmall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    FrameTooBig,
}

pub struct WebSocketWireDecoder {
    state: WireDecoderState,
}

enum WireDecoderState {
    RawPacket,
    Hdlc(rns_serial_framing::RnsSerialScanner),
    Kiss(kiss_framing::KissScanner),
}

impl WebSocketWireDecoder {
    #[must_use]
    pub const fn new(framing: WebSocketWireFraming) -> Self {
        let state = match framing {
            WebSocketWireFraming::RawPacket => WireDecoderState::RawPacket,
            WebSocketWireFraming::Hdlc => {
                WireDecoderState::Hdlc(rns_serial_framing::RnsSerialScanner::new())
            }
            WebSocketWireFraming::Kiss => WireDecoderState::Kiss(kiss_framing::KissScanner::new()),
        };
        Self { state }
    }

    pub fn reset(&mut self) {
        match &mut self.state {
            WireDecoderState::RawPacket => {}
            WireDecoderState::Hdlc(scanner) => scanner.reset(),
            WireDecoderState::Kiss(scanner) => scanner.reset(),
        }
    }

    pub fn next_frame_into(
        &mut self,
        input: &[u8],
        offset: &mut usize,
        sink: &mut dyn FrameSink,
    ) -> Result<Option<usize>, DecodeError> {
        match &mut self.state {
            WireDecoderState::RawPacket => {
                if *offset != 0 || input.is_empty() {
                    *offset = input.len();
                    return Ok(None);
                }
                *offset = input.len();
                sink.clear();
                if input.len() > FRAME_CAP || sink.extend_from_slice(input).is_err() {
                    sink.clear();
                    return Err(DecodeError::FrameTooBig);
                }
                Ok(Some(input.len()))
            }
            WireDecoderState::Hdlc(scanner) => scanner
                .next_frame_into(input, offset, sink)
                .map_err(|_| DecodeError::FrameTooBig),
            WireDecoderState::Kiss(scanner) => scanner
                .next_frame_into(input, offset, sink)
                .map_err(|_| DecodeError::FrameTooBig),
        }
    }
}

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    gravity: crate::interfaces::InterfaceGravity::ZERO,
    bitrate: WEBSOCKET_BITRATE_ESTIMATE,
    mtu: MtuPolicy::optimized_from_bitrate(MAX_LINK_MTU),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

#[must_use]
pub fn configured_policy(configured: ConfiguredInterfacePolicy) -> EffectiveInterfacePolicy {
    DEFAULTS.configured(configured)
}

#[must_use]
pub fn policy_for_bitrate(bitrate: BitrateBps) -> EffectiveInterfacePolicy {
    configured_policy(ConfiguredInterfacePolicy {
        bitrate: Some(bitrate),
        ..ConfiguredInterfacePolicy::default()
    })
}

pub fn descriptor(id: InterfaceId, policy: EffectiveInterfacePolicy) -> InterfaceDescriptor {
    policy.descriptor(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[derive(Default)]
    struct TestSink(Vec<u8>);

    impl FrameSink for TestSink {
        fn clear(&mut self) {
            self.0.clear();
        }

        fn frame_len(&self) -> usize {
            self.0.len()
        }

        fn free_capacity(&self) -> usize {
            FRAME_CAP.saturating_sub(self.0.len())
        }

        fn push(&mut self, byte: u8) -> Result<(), crate::interfaces::FrameSinkError> {
            if self.0.len() >= FRAME_CAP {
                return Err(crate::interfaces::FrameSinkError::Full);
            }
            self.0.push(byte);
            Ok(())
        }

        fn extend_from_slice(
            &mut self,
            run: &[u8],
        ) -> Result<(), crate::interfaces::FrameSinkError> {
            if run.len() > FRAME_CAP.saturating_sub(self.0.len()) {
                return Err(crate::interfaces::FrameSinkError::Full);
            }
            self.0.extend_from_slice(run);
            Ok(())
        }
    }

    fn encoded(framing: WebSocketWireFraming, packet: &[u8]) -> Vec<u8> {
        let mut output = vec![0; framing.message_cap()];
        let len = framing.encode(packet, &mut output).expect("packet encodes");
        output.truncate(len);
        output
    }

    #[test]
    fn every_wire_framing_round_trips_delimiter_bytes() {
        let packet = [0x01, 0x7d, 0x7e, 0xc0, 0xdb, 0x02];
        for framing in [
            WebSocketWireFraming::RawPacket,
            WebSocketWireFraming::Hdlc,
            WebSocketWireFraming::Kiss,
        ] {
            let wire = encoded(framing, &packet);
            let mut decoder = WebSocketWireDecoder::new(framing);
            let mut sink = TestSink::default();
            let mut offset = 0;
            let decoded = decoder
                .next_frame_into(&wire, &mut offset, &mut sink)
                .expect("wire frame decodes");
            assert_eq!(decoded, Some(packet.len()));
            assert_eq!(sink.0, packet);
        }
    }

    #[test]
    fn stream_framing_survives_split_messages_and_coalesced_frames() {
        let first_packet = [1, 0x7e, 0xc0, 3];
        let second_packet = [4, 0x7d, 0xdb, 5];
        for framing in [WebSocketWireFraming::Hdlc, WebSocketWireFraming::Kiss] {
            let first = encoded(framing, &first_packet);
            let second = encoded(framing, &second_packet);
            let split = first.len() / 2;
            let mut decoder = WebSocketWireDecoder::new(framing);
            let mut sink = TestSink::default();
            let mut offset = 0;
            assert_eq!(
                decoder
                    .next_frame_into(&first[..split], &mut offset, &mut sink)
                    .expect("partial frame is accepted"),
                None
            );

            let mut joined = first[split..].to_vec();
            joined.extend_from_slice(&second);
            let mut offset = 0;
            assert_eq!(
                decoder
                    .next_frame_into(&joined, &mut offset, &mut sink)
                    .expect("first frame completes"),
                Some(first_packet.len())
            );
            assert_eq!(sink.0, first_packet);
            assert_eq!(
                decoder
                    .next_frame_into(&joined, &mut offset, &mut sink)
                    .expect("second frame completes"),
                Some(second_packet.len())
            );
            assert_eq!(sink.0, second_packet);
        }
    }

    #[test]
    fn oversize_stream_frame_is_dropped_and_the_decoder_realigns() {
        let oversized = vec![0x44; FRAME_CAP + 1];
        let valid = [0x11, 0x22, 0x33];
        for framing in [WebSocketWireFraming::Hdlc, WebSocketWireFraming::Kiss] {
            let mut wire = vec![0; framing.message_cap() + 2];
            let oversized_len = match framing {
                WebSocketWireFraming::Hdlc => {
                    rns_serial_framing::encode(&oversized, &mut wire).expect("oversize encodes")
                }
                WebSocketWireFraming::Kiss => {
                    kiss_framing::encode(&oversized, &mut wire).expect("oversize encodes")
                }
                WebSocketWireFraming::RawPacket => continue,
            };
            wire.truncate(oversized_len);
            wire.extend_from_slice(&encoded(framing, &valid));
            let mut decoder = WebSocketWireDecoder::new(framing);
            let mut sink = TestSink::default();
            let mut offset = 0;
            assert_eq!(
                decoder.next_frame_into(&wire, &mut offset, &mut sink),
                Err(DecodeError::FrameTooBig)
            );
            let mut recovered = None;
            while offset < wire.len() {
                match decoder.next_frame_into(&wire, &mut offset, &mut sink) {
                    Ok(Some(len)) if len != 0 => recovered = Some(sink.0.clone()),
                    Ok(_) | Err(DecodeError::FrameTooBig) => {}
                }
            }
            assert_eq!(recovered.as_deref(), Some(valid.as_slice()));
        }
    }

    #[test]
    fn mode_specific_caps_and_channel_tags_are_distinct() {
        assert_eq!(WebSocketWireFraming::RawPacket.message_cap(), FRAME_CAP);
        assert_eq!(
            WebSocketWireFraming::Hdlc.message_cap(),
            rns_serial_framing::max_encoded_len(FRAME_CAP)
        );
        assert_eq!(
            WebSocketWireFraming::Kiss.message_cap(),
            kiss_framing::max_encoded_len(FRAME_CAP)
        );
        assert_ne!(
            WebSocketWireFraming::RawPacket.channel_tag_suffix(),
            WebSocketWireFraming::Hdlc.channel_tag_suffix()
        );
        assert_ne!(
            WebSocketWireFraming::RawPacket.channel_tag_suffix(),
            WebSocketWireFraming::Kiss.channel_tag_suffix()
        );
        for framing in [
            WebSocketWireFraming::RawPacket,
            WebSocketWireFraming::Hdlc,
            WebSocketWireFraming::Kiss,
        ] {
            assert_eq!(WebSocketWireFraming::from_name(framing.name()), Ok(framing));
        }
        assert_eq!(
            WebSocketWireFraming::from_name("Raw"),
            Err(WebSocketWireFramingParseError::UnknownFraming)
        );
    }
}
