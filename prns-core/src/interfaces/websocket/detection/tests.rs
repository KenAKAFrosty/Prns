use super::allocated::{
    DecodedWebSocketFrame, WebSocketFrameDecodeOutcome, WebSocketFramingDecoder,
    WebSocketFramingState, WebSocketWireDetection, WebSocketWireDetector,
};
use super::*;
use crate::interfaces::framing::FrameBuffer;
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireContext, WirePacketHeader,
};
use alloc::vec;
use alloc::vec::Vec;

#[allow(clippy::expect_used)]
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

#[allow(clippy::expect_used)]
fn encoded(framing: WebSocketWireFraming, packet: &[u8]) -> Vec<u8> {
    let mut output = vec![0; framing.message_cap()];
    let len = framing.encode(packet, &mut output).expect("packet encodes");
    output.truncate(len);
    output
}

#[allow(clippy::panic)]
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

fn automatic_session_releases_provisional_raw_and_recovers_as(framing: WebSocketWireFraming) {
    let first = packet(b"first");
    let second = packet(b"second");
    let mut session = WebSocketSessionFraming::new(WebSocketFramingSelection::Auto);

    assert!(session.release_raw_fallback().is_none());
    assert!(session.can_read_outbound());
    assert!(!session.can_stage_multiple_outbound());
    assert_eq!(
        session.stage_outbound(&first),
        WebSocketSessionOutboundAction::Queued
    );
    assert!(!session.can_read_outbound());
    assert_eq!(
        session.stage_outbound(&second),
        WebSocketSessionOutboundAction::Backpressured
    );

    let release = session.release_raw_fallback();
    assert!(matches!(
        release.as_ref(),
        Some(resolved)
            if resolved.framing() == WebSocketWireFraming::RawPacket
                && resolved.pending_packet() == Some(first.as_slice())
    ));
    assert!(session.release_raw_fallback().is_none());
    assert!(session.is_detecting());
    assert!(session.can_read_outbound());
    assert!(session.can_stage_multiple_outbound());
    assert_eq!(
        session.stage_outbound(&second),
        WebSocketSessionOutboundAction::Send(WebSocketWireFraming::RawPacket)
    );

    let inbound = packet(b"late-framing-evidence");
    let message = encoded(framing, &inbound);
    let mut offset = 0;
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
    let outcome = session.next_frame_into(&message, &mut offset, &mut sink);
    assert!(matches!(
        outcome,
        Ok(WebSocketSessionFrameDecodeOutcome::ResolvedFrame(resolved))
            if resolved.framing() == framing && resolved.pending_packet().is_none()
    ));
    assert_eq!(sink.as_slice(), inbound.as_slice());
    assert!(!session.is_detecting());
    assert!(session.can_stage_multiple_outbound());
    assert_eq!(
        session.stage_outbound(&second),
        WebSocketSessionOutboundAction::Send(framing)
    );
}

#[test]
fn automatic_session_recovers_from_provisional_raw_to_kiss() {
    automatic_session_releases_provisional_raw_and_recovers_as(WebSocketWireFraming::Kiss);
}

#[test]
fn automatic_session_recovers_from_provisional_raw_to_hdlc() {
    automatic_session_releases_provisional_raw_and_recovers_as(WebSocketWireFraming::Hdlc);
}

#[test]
fn passive_session_detection_selects_pending_outbound_framing() {
    let inbound = packet(b"inbound");
    let outbound = packet(b"outbound");
    let message = encoded(WebSocketWireFraming::Kiss, &inbound);
    let mut session = WebSocketSessionFraming::new(WebSocketFramingSelection::Auto);
    assert_eq!(
        session.stage_outbound(&outbound),
        WebSocketSessionOutboundAction::Queued
    );

    let mut offset = 0;
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
    let outcome = session
        .next_frame_into(&message, &mut offset, &mut sink)
        .expect("framing detection succeeds");
    let WebSocketSessionFrameDecodeOutcome::ResolvedFrame(resolved) = outcome else {
        panic!("KISS is unique framing evidence")
    };
    assert_eq!(resolved.framing(), WebSocketWireFraming::Kiss);
    assert_eq!(resolved.pending_packet(), Some(outbound.as_slice()));
    assert_eq!(sink.as_slice(), inbound.as_slice());
    assert_eq!(
        session.stage_outbound(&outbound),
        WebSocketSessionOutboundAction::Send(WebSocketWireFraming::Kiss)
    );
}

#[test]
fn raw_packet_is_unique_detection_evidence() {
    let packet = packet(&[0xC0, 0xDB, 0x7E, 0x7D]);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
    let frame = detected_frame(
        detector
            .inspect_message(&packet, &mut sink)
            .expect("detection succeeds"),
    );
    assert_eq!(frame.framing(), WebSocketWireFraming::RawPacket);
    assert_eq!(frame.frame_len(), packet.len());
    assert_eq!(frame.consumed_message_bytes(), packet.len());
    assert_eq!(sink.as_slice(), packet.as_slice());
}

#[test]
fn hdlc_detection_accumulates_across_websocket_messages() {
    let packet = packet(&[0x7E, 0x7D, 0x44]);
    let wire = encoded(WebSocketWireFraming::Hdlc, &packet);
    let split = wire.len() / 2;
    let mut detector = WebSocketWireDetector::new();
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
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
    assert_eq!(sink.as_slice(), packet.as_slice());
}

#[test]
fn kiss_detection_reports_the_first_coalesced_frame_boundary() {
    let packet = packet(&[0xC0, 0xDB, 0x44]);
    let first = encoded(WebSocketWireFraming::Kiss, &packet);
    let mut wire = first.clone();
    wire.extend_from_slice(&first);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
    let frame = detected_frame(
        detector
            .inspect_message(&wire, &mut sink)
            .expect("detection succeeds"),
    );
    assert_eq!(frame.framing(), WebSocketWireFraming::Kiss);
    assert_eq!(frame.frame_len(), packet.len());
    assert_eq!(frame.consumed_message_bytes(), first.len());
    assert_eq!(sink.as_slice(), packet.as_slice());
}

#[test]
fn multiple_valid_interpretations_remain_ambiguous() {
    let inner = packet(&[0x22]);
    let framed_inner = encoded(WebSocketWireFraming::Kiss, &inner);
    let outer = packet(&framed_inner);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
    assert_eq!(
        detector.inspect_message(&outer, &mut sink),
        Ok(WebSocketWireDetection::AmbiguousEvidence)
    );
    assert!(sink.as_slice().is_empty());
}

#[test]
fn opaque_ifac_and_malformed_frames_do_not_select_a_codec() {
    let mut authenticated = packet(&[0x22]);
    authenticated[0] |= 0x80;
    let malformed = encoded(WebSocketWireFraming::Hdlc, &[0x01, 0x02]);
    let mut detector = WebSocketWireDetector::new();
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
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
fn packet_evidence_uses_the_reference_hop_boundary() {
    let mut last_reachable = packet(&[0x22]);
    last_reachable[1] = crate::wire::MAX_HOP_COUNT - 1;
    let mut detector = WebSocketWireDetector::new();
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
    assert_eq!(
        detected_frame(
            detector
                .inspect_message(&last_reachable, &mut sink)
                .expect("detection succeeds"),
        )
        .framing(),
        WebSocketWireFraming::RawPacket,
    );

    let mut out_of_reach = last_reachable;
    out_of_reach[1] = crate::wire::MAX_HOP_COUNT;
    let mut detector = WebSocketWireDetector::new();
    assert_eq!(
        detector.inspect_message(&out_of_reach, &mut sink),
        Ok(WebSocketWireDetection::AwaitingEvidence),
    );
}

#[test]
fn reset_discards_partial_stream_evidence() {
    let packet = packet(&[0x7E, 0x7D, 0x44]);
    let wire = encoded(WebSocketWireFraming::Hdlc, &packet);
    let split = wire.len() / 2;
    let mut detector = WebSocketWireDetector::new();
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
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
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
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
    let mut sink = FrameBuffer::<FRAME_CAP>::new();
    let mut offset = 0;

    let first_frame = decoder
        .next_frame_into(&wire, &mut offset, &mut sink)
        .expect("first packet decodes");
    assert!(matches!(first_frame, WebSocketFrameDecodeOutcome::Frame(_)));
    assert_eq!(offset, first.len());
    assert_eq!(sink.as_slice(), packet.as_slice());

    let second_frame = decoder
        .next_frame_into(&wire, &mut offset, &mut sink)
        .expect("second packet decodes");
    assert!(matches!(
        second_frame,
        WebSocketFrameDecodeOutcome::Frame(_)
    ));
    assert_eq!(offset, wire.len());
    assert_eq!(sink.as_slice(), packet.as_slice());
}
