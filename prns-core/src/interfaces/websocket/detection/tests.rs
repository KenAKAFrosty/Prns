use super::allocated::{
    DecodedWebSocketFrame, WebSocketFrameDecodeOutcome, WebSocketFramingDecoder,
    WebSocketFramingState, WebSocketWireDetection, WebSocketWireDetector,
};
use super::*;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader,
};
use alloc::vec;
use alloc::vec::Vec;

fn packet(payload: &[u8]) -> Vec<u8> {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: DestinationHash::new([0x11; 16]).to_address(),
        context: WireContext::None,
    };
    let mut bytes = vec![0u8; crate::wire::HEADER_MIN_LEN + payload.len()];
    let header_len = header.write(&mut bytes).expect("header fits");
    bytes[header_len..].copy_from_slice(payload);
    bytes
}

fn encoded(framing: WebSocketWireFraming, packet: &[u8]) -> Vec<u8> {
    let mut output = vec![0; framing.message_cap()];
    let len = framing.encode(packet, &mut output).expect("packet encodes");
    output.truncate(len);
    output
}

fn detected_frame(outcome: WebSocketWireDetection) -> DecodedWebSocketFrame {
    let WebSocketWireDetection::Detected(frame) = outcome else {
        panic!("wire framing was not detected")
    };
    frame
}

#[test]
fn framing_selection_names_all_four_closed_variants() {
    assert_eq!(
        WebSocketFramingSelection::from_name(WebSocketFramingSelection::Auto.name()),
        Ok(WebSocketFramingSelection::Auto)
    );
    for framing in WebSocketWireFraming::ALL {
        let selection = WebSocketFramingSelection::Fixed(framing);
        assert_eq!(
            WebSocketFramingSelection::from_name(selection.name()),
            Ok(selection)
        );
    }
    assert_eq!(
        WebSocketFramingSelection::from_name("raw-packet"),
        Err(WebSocketFramingSelectionParseError::UnknownSelection)
    );
    assert_eq!(
        WebSocketFramingSelection::Auto.channel_tag_suffix(),
        b"\0auto"
    );
    assert_eq!(
        WebSocketFramingSelection::Auto.message_cap(),
        kiss_framing::max_encoded_len(FRAME_CAP)
    );
}

#[test]
fn raw_packet_is_unique_detection_evidence() {
    let packet = packet(&[0xC0, 0xDB, 0x7E, 0x7D]);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = Vec::new();
    let frame = detected_frame(
        detector
            .inspect_message(&packet, &mut sink)
            .expect("detection succeeds"),
    );
    assert_eq!(frame.framing(), WebSocketWireFraming::RawPacket);
    assert_eq!(frame.frame_len(), packet.len());
    assert_eq!(frame.consumed_message_bytes(), packet.len());
    assert_eq!(sink, packet);
}

#[test]
fn hdlc_detection_accumulates_across_websocket_messages() {
    let packet = packet(&[0x7E, 0x7D, 0x44]);
    let wire = encoded(WebSocketWireFraming::Hdlc, &packet);
    let split = wire.len() / 2;
    let mut detector = WebSocketWireDetector::new();
    let mut sink = Vec::new();
    assert_eq!(
        detector.inspect_message(&wire[..split], &mut sink),
        Ok(WebSocketWireDetection::AwaitingEvidence)
    );
    let frame = detected_frame(
        detector
            .inspect_message(&wire[split..], &mut sink)
            .expect("detection succeeds"),
    );
    assert_eq!(frame.framing(), WebSocketWireFraming::Hdlc);
    assert_eq!(frame.frame_len(), packet.len());
    assert_eq!(frame.consumed_message_bytes(), wire.len() - split);
    assert_eq!(sink, packet);
}

#[test]
fn kiss_detection_reports_the_first_coalesced_frame_boundary() {
    let packet = packet(&[0xC0, 0xDB, 0x44]);
    let first = encoded(WebSocketWireFraming::Kiss, &packet);
    let mut wire = first.clone();
    wire.extend_from_slice(&first);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = Vec::new();
    let frame = detected_frame(
        detector
            .inspect_message(&wire, &mut sink)
            .expect("detection succeeds"),
    );
    assert_eq!(frame.framing(), WebSocketWireFraming::Kiss);
    assert_eq!(frame.frame_len(), packet.len());
    assert_eq!(frame.consumed_message_bytes(), first.len());
    assert_eq!(sink, packet);
}

#[test]
fn multiple_valid_interpretations_remain_ambiguous() {
    let inner = packet(&[0x22]);
    let framed_inner = encoded(WebSocketWireFraming::Kiss, &inner);
    let outer = packet(&framed_inner);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = Vec::new();
    assert_eq!(
        detector.inspect_message(&outer, &mut sink),
        Ok(WebSocketWireDetection::AmbiguousEvidence)
    );
    assert!(sink.is_empty());
}

#[test]
fn opaque_ifac_and_malformed_frames_do_not_select_a_codec() {
    let mut authenticated = packet(&[0x22]);
    authenticated[0] |= 0x80;
    let malformed = encoded(WebSocketWireFraming::Hdlc, &[0x01, 0x02]);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = Vec::new();
    assert_eq!(
        detector.inspect_message(&authenticated, &mut sink),
        Ok(WebSocketWireDetection::AwaitingEvidence)
    );
    assert_eq!(
        detector.inspect_message(&malformed, &mut sink),
        Ok(WebSocketWireDetection::AwaitingEvidence)
    );
}

#[test]
fn reset_discards_partial_stream_evidence() {
    let packet = packet(&[0x7E, 0x7D, 0x44]);
    let wire = encoded(WebSocketWireFraming::Hdlc, &packet);
    let split = wire.len() / 2;
    let mut detector = WebSocketWireDetector::new();
    let mut sink = Vec::new();
    assert_eq!(
        detector.inspect_message(&wire[..split], &mut sink),
        Ok(WebSocketWireDetection::AwaitingEvidence)
    );
    detector.reset();
    assert_eq!(
        detector.inspect_message(&wire[split..], &mut sink),
        Ok(WebSocketWireDetection::AwaitingEvidence)
    );
}

#[test]
fn auto_and_fixed_connection_states_reset_by_their_own_policy() {
    let packet = packet(&[0x22]);
    let mut automatic = WebSocketFramingDecoder::new(WebSocketFramingSelection::Auto);
    let mut sink = Vec::new();
    let mut offset = 0;
    assert!(matches!(
        automatic
            .next_frame_into(&packet, &mut offset, &mut sink)
            .expect("packet decodes"),
        WebSocketFrameDecodeOutcome::Frame(_)
    ));
    assert_eq!(
        automatic.state(),
        WebSocketFramingState::Resolved(WebSocketWireFraming::RawPacket)
    );
    automatic.reset_connection();
    assert_eq!(automatic.state(), WebSocketFramingState::Detecting);

    let mut fixed =
        WebSocketFramingDecoder::new(WebSocketFramingSelection::Fixed(WebSocketWireFraming::Kiss));
    fixed.reset_connection();
    assert_eq!(
        fixed.state(),
        WebSocketFramingState::Resolved(WebSocketWireFraming::Kiss)
    );
}

#[test]
fn auto_resolution_continues_through_coalesced_frames() {
    let packet = packet(&[0xC0, 0xDB, 0x44]);
    let first = encoded(WebSocketWireFraming::Kiss, &packet);
    let mut wire = first.clone();
    wire.extend_from_slice(&first);
    let mut decoder = WebSocketFramingDecoder::new(WebSocketFramingSelection::Auto);
    let mut sink = Vec::new();
    let mut offset = 0;

    let first_frame = decoder
        .next_frame_into(&wire, &mut offset, &mut sink)
        .expect("first packet decodes");
    assert!(matches!(first_frame, WebSocketFrameDecodeOutcome::Frame(_)));
    assert_eq!(offset, first.len());
    assert_eq!(sink, packet);

    let second_frame = decoder
        .next_frame_into(&wire, &mut offset, &mut sink)
        .expect("second packet decodes");
    assert!(matches!(
        second_frame,
        WebSocketFrameDecodeOutcome::Frame(_)
    ));
    assert_eq!(offset, wire.len());
    assert_eq!(sink, packet);
}
