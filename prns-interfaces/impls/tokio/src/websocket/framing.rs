use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::WebSocketStream;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::websocket::{
    WebSocketFramingSelection, WebSocketOutboundRelease,
    WebSocketSessionFrameDecodeOutcome as SessionFrameDecodeOutcome,
    WebSocketSessionFraming as SessionFraming,
    WebSocketSessionOutboundAction as SessionOutboundAction, WebSocketWireFraming,
    AUTO_DETECTION_GRACE_PERIOD_MILLIS,
};
use prns_core::interfaces::BitrateBps;
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{
    InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use prns_runtime::manifold::throughput::ThroughputLedger;

const AUTO_DETECTION_GRACE_PERIOD: std::time::Duration =
    std::time::Duration::from_millis(AUTO_DETECTION_GRACE_PERIOD_MILLIS);
const SOCKET_BUFFER_LEN: usize = 16 * 1024;

pub(crate) struct SessionConfig {
    bitrate: BitrateBps,
    started: tokio::time::Instant,
    framing_selection: WebSocketFramingSelection,
}

impl SessionConfig {
    pub(crate) fn new(
        bitrate: BitrateBps,
        started: tokio::time::Instant,
        framing_selection: WebSocketFramingSelection,
    ) -> Self {
        Self {
            bitrate,
            started,
            framing_selection,
        }
    }
}

pub(crate) fn config(selection: WebSocketFramingSelection) -> WebSocketConfig {
    let message_cap = selection.message_cap();
    WebSocketConfig::default()
        .read_buffer_size(SOCKET_BUFFER_LEN)
        .write_buffer_size(SOCKET_BUFFER_LEN)
        .max_write_buffer_size(
            message_cap
                .saturating_add(SOCKET_BUFFER_LEN)
                .saturating_add(1),
        )
        .max_message_size(Some(message_cap))
        .max_frame_size(Some(message_cap))
}

pub async fn serve<S, Seam>(
    mut socket: WebSocketStream<S>,
    seam: &mut Seam,
    status: &TokioInterfaceStatus,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    config: SessionConfig,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let mut framing = SessionFraming::new(config.framing_selection);
    let detection_grace = tokio::time::sleep(AUTO_DETECTION_GRACE_PERIOD);
    tokio::pin!(detection_grace);
    'session: loop {
        let raw_fallback_is_armed = framing.raw_fallback_is_armed();
        let can_read_outbound = framing.can_read_outbound();
        tokio::select! {
            inbound = socket.next() => {
                let Some(inbound) = inbound else {
                    break;
                };
                let message = match inbound {
                    Ok(message) => message,
                    Err(_) => break,
                };
                match message {
                    Message::Binary(frame) => {
                        if frame.is_empty() || frame.len() > config.framing_selection.message_cap() {
                            continue;
                        }
                        status.add_rx(frame.len() as u64);
                        let elapsed = u64::try_from(config.started.elapsed().as_millis()).unwrap_or(u64::MAX);
                        let now = InstantMillis(elapsed);
                        throughput.record_rx(now, frame.len() as u64);
                        status.set_transfer_rates(throughput.rates());
                        let mut offset = 0;
                        while offset < frame.len() {
                            let sink = seam.inbound_sink().await;
                            match framing.next_frame_into(&frame, &mut offset, sink) {
                                Ok(SessionFrameDecodeOutcome::Frame) => {
                                    seam.commit_inbound().await;
                                }
                                Ok(SessionFrameDecodeOutcome::ResolvedFrame(resolved)) => {
                                    seam.commit_inbound().await;
                                    if matches!(
                                        send_released_outbound(
                                        resolved,
                                        &mut socket,
                                        seam,
                                        status,
                                        airtime,
                                        throughput,
                                        &config,
                                    ).await,
                                        AcceptedOutboundSendOutcome::TransportClosed
                                    ) {
                                        break 'session;
                                    }
                                }
                                Ok(SessionFrameDecodeOutcome::Incomplete)
                                | Ok(SessionFrameDecodeOutcome::AmbiguousFraming) => break,
                                Err(_) => break,
                            }
                        }
                    }
                    Message::Text(_)
                    | Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Frame(_) => {}
                    Message::Close(_) => break,
                }
            }
            outbound = seam.next_outbound(), if can_read_outbound => {
                match framing.stage_outbound(outbound) {
                    SessionOutboundAction::Queued => {
                        seam.accept_outbound_custody();
                        detection_grace.as_mut().reset(
                            tokio::time::Instant::now() + AUTO_DETECTION_GRACE_PERIOD
                        );
                    }
                    SessionOutboundAction::Send(wire_framing) => {
                        if matches!(
                            send_wire_message(
                                &mut socket,
                                wire_framing,
                                outbound,
                                status,
                                airtime,
                                throughput,
                                &config,
                            ).await,
                            WireSendOutcome::TransportClosed
                        ) {
                            break;
                        }
                    }
                    SessionOutboundAction::Rejected
                    | SessionOutboundAction::Backpressured => {}
                }
            }
            () = &mut detection_grace, if raw_fallback_is_armed => {
                let Some(released) = framing.release_raw_fallback() else {
                    continue;
                };
                if matches!(
                    send_released_outbound(
                        released,
                        &mut socket,
                        seam,
                        status,
                        airtime,
                        throughput,
                        &config,
                    ).await,
                    AcceptedOutboundSendOutcome::TransportClosed
                ) {
                    break;
                }
            }
        }
    }
    if framing.has_pending_outbound() {
        seam.complete_outbound(OutboundDisposition::Dropped(
            OutboundDropReason::TransportFailure,
        ));
    }
}

enum WireSendOutcome {
    Sent,
    Rejected,
    TransportClosed,
}

enum AcceptedOutboundSendOutcome {
    Continue,
    TransportClosed,
}

async fn send_released_outbound<S, Seam>(
    released: WebSocketOutboundRelease,
    socket: &mut WebSocketStream<S>,
    seam: &mut Seam,
    status: &TokioInterfaceStatus,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    config: &SessionConfig,
) -> AcceptedOutboundSendOutcome
where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let Some(packet) = released.pending_packet() else {
        return AcceptedOutboundSendOutcome::Continue;
    };
    match send_wire_message(
        socket,
        released.framing(),
        packet,
        status,
        airtime,
        throughput,
        config,
    )
    .await
    {
        WireSendOutcome::Sent => {
            seam.complete_outbound(OutboundDisposition::Sent);
            AcceptedOutboundSendOutcome::Continue
        }
        WireSendOutcome::Rejected => {
            seam.complete_outbound(OutboundDisposition::Dropped(OutboundDropReason::Rejected));
            AcceptedOutboundSendOutcome::Continue
        }
        WireSendOutcome::TransportClosed => {
            seam.complete_outbound(OutboundDisposition::Dropped(
                OutboundDropReason::TransportFailure,
            ));
            AcceptedOutboundSendOutcome::TransportClosed
        }
    }
}

async fn send_wire_message<S>(
    socket: &mut WebSocketStream<S>,
    framing: WebSocketWireFraming,
    packet: &[u8],
    status: &TokioInterfaceStatus,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    config: &SessionConfig,
) -> WireSendOutcome
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some((message, encoded_len)) = wire_message(framing, packet) else {
        return WireSendOutcome::Rejected;
    };
    if socket.send(message).await.is_err() {
        return WireSendOutcome::TransportClosed;
    }
    status.add_tx(encoded_len as u64);
    let elapsed = u64::try_from(config.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let now = InstantMillis(elapsed);
    throughput.record_tx(now, encoded_len as u64);
    status.set_transfer_rates(throughput.rates());
    let frame_airtime = frame_airtime_us(encoded_len, config.bitrate);
    status.set_airtime(airtime.record_tx(now, frame_airtime));
    WireSendOutcome::Sent
}

fn wire_message(framing: WebSocketWireFraming, packet: &[u8]) -> Option<(Message, usize)> {
    match framing {
        WebSocketWireFraming::RawPacket => {
            if packet.is_empty() || packet.len() > framing.message_cap() {
                return None;
            }
            Some((Message::binary(packet.to_vec()), packet.len()))
        }
        WebSocketWireFraming::Hdlc | WebSocketWireFraming::Kiss => {
            let mut encoded = std::vec![0; framing.message_cap()];
            let encoded_len = framing.encode(packet, &mut encoded).ok()?;
            encoded.truncate(encoded_len);
            Some((Message::binary(encoded), encoded_len))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use prns_core::interfaces::websocket;
    use prns_core::interfaces::{ConnectionState, FrameSink, InterfaceId, InterfaceKind};
    use prns_core::wire::{
        ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
        WireContext, WirePacketHeader,
    };
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use tokio_tungstenite::tungstenite::protocol::Role;

    struct OutboundOnlySeam {
        sink: Vec<u8>,
        outbound: TokioGrantConsumer,
        inbound: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    }

    impl InterfaceSeam for OutboundOnlySeam {
        fn fill_entropy(&mut self, bytes: &mut [u8]) {
            bytes.fill(0);
        }

        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.sink
        }

        async fn commit_inbound(&mut self) {
            if let Some(inbound) = &self.inbound {
                let _ = inbound.send(self.sink.clone());
            }
            self.sink.clear();
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }

        fn accept_outbound_custody(&mut self) {
            self.outbound.release();
        }
    }

    fn packet(payload: &[u8]) -> Vec<u8> {
        let header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport_id: None,
            address: DestinationHash::new([0x31; 16]).to_address(),
            context: WireContext::None,
        };
        let mut bytes = vec![0u8; prns_core::wire::HEADER_MIN_LEN + payload.len()];
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

    #[test]
    fn websocket_buffers_and_inbound_messages_are_bounded() {
        for framing in WebSocketWireFraming::ALL {
            let selection = WebSocketFramingSelection::Fixed(framing);
            let config = config(selection);
            assert_eq!(config.read_buffer_size, SOCKET_BUFFER_LEN);
            assert_eq!(config.write_buffer_size, SOCKET_BUFFER_LEN);
            assert_eq!(config.max_message_size, Some(framing.message_cap()));
            assert_eq!(config.max_frame_size, Some(framing.message_cap()));
            assert!(
                config.max_write_buffer_size > config.write_buffer_size + framing.message_cap()
            );
        }
        let automatic = config(WebSocketFramingSelection::Auto);
        assert_eq!(
            automatic.max_message_size,
            Some(WebSocketFramingSelection::Auto.message_cap())
        );
        assert_eq!(automatic.max_frame_size, automatic.max_message_size);
    }

    async fn late_framing_evidence_replaces_provisional_raw_egress(framing: WebSocketWireFraming) {
        let (client_io, server_io) = tokio::io::duplex(SOCKET_BUFFER_LEN);
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let server = WebSocketStream::from_raw_socket(
            server_io,
            Role::Server,
            Some(config(WebSocketFramingSelection::Auto)),
        )
        .await;
        let (mut outbound, outbound_consumer) = tokio_grant_lane(websocket::FRAME_CAP, 1);
        let (inbound_sender, mut inbound) = tokio::sync::mpsc::unbounded_channel();
        let seam = OutboundOnlySeam {
            sink: Vec::new(),
            outbound: outbound_consumer,
            inbound: Some(inbound_sender),
        };
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketServerPeer, b"auto-test");
        let status = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected);
        let started = tokio::time::Instant::now();
        let task = tokio::spawn(async move {
            let mut seam = seam;
            let mut airtime = AirtimeLedger::new();
            let mut throughput = ThroughputLedger::new();
            serve(
                server,
                &mut seam,
                &status,
                &mut airtime,
                &mut throughput,
                SessionConfig::new(
                    websocket::WEBSOCKET_BITRATE_ESTIMATE,
                    started,
                    WebSocketFramingSelection::Auto,
                ),
            )
            .await;
        });

        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(client.next().now_or_never().is_none());

        let outbound_packet = packet(b"silent-peer");
        outbound
            .try_grant()
            .expect("the outbound lane has capacity")
            .fill(&outbound_packet);
        outbound.commit();
        tokio::task::yield_now().await;
        assert!(client.next().now_or_never().is_none());

        tokio::time::advance(AUTO_DETECTION_GRACE_PERIOD).await;
        tokio::task::yield_now().await;
        let received = client
            .next()
            .now_or_never()
            .expect("the fallback deadline releases the packet")
            .expect("the websocket remains open")
            .expect("the websocket message is valid")
            .into_data();
        assert_eq!(received, outbound_packet);

        let inbound_packet = packet(b"late-framing-evidence");
        client
            .send(Message::binary(encoded(framing, &inbound_packet)))
            .await
            .expect("late framing evidence reaches the session");
        assert_eq!(
            inbound
                .recv()
                .await
                .expect("the framed packet is committed"),
            inbound_packet
        );

        let subsequent_packet = packet(b"after-evidence");
        outbound
            .try_grant()
            .expect("the outbound lane regains capacity")
            .fill(&subsequent_packet);
        outbound.commit();
        let received = client
            .next()
            .await
            .expect("the websocket remains open")
            .expect("the websocket message is valid")
            .into_data();
        assert_eq!(received, encoded(framing, &subsequent_packet));
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn late_kiss_evidence_replaces_provisional_raw_egress() {
        late_framing_evidence_replaces_provisional_raw_egress(WebSocketWireFraming::Kiss).await;
    }

    #[tokio::test(start_paused = true)]
    async fn late_hdlc_evidence_replaces_provisional_raw_egress() {
        late_framing_evidence_replaces_provisional_raw_egress(WebSocketWireFraming::Hdlc).await;
    }

    #[tokio::test]
    async fn passive_kiss_evidence_releases_the_pending_packet_as_kiss() {
        let (client_io, server_io) = tokio::io::duplex(SOCKET_BUFFER_LEN);
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let server = WebSocketStream::from_raw_socket(
            server_io,
            Role::Server,
            Some(config(WebSocketFramingSelection::Auto)),
        )
        .await;
        let (mut outbound, outbound_consumer) = tokio_grant_lane(websocket::FRAME_CAP, 1);
        let seam = OutboundOnlySeam {
            sink: Vec::new(),
            outbound: outbound_consumer,
            inbound: None,
        };
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketServerPeer, b"kiss-test");
        let status = TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected);
        let task = tokio::spawn(async move {
            let mut seam = seam;
            let mut airtime = AirtimeLedger::new();
            let mut throughput = ThroughputLedger::new();
            serve(
                server,
                &mut seam,
                &status,
                &mut airtime,
                &mut throughput,
                SessionConfig::new(
                    websocket::WEBSOCKET_BITRATE_ESTIMATE,
                    tokio::time::Instant::now(),
                    WebSocketFramingSelection::Auto,
                ),
            )
            .await;
        });

        let outbound_packet = packet(b"pending");
        outbound
            .try_grant()
            .expect("the outbound lane has capacity")
            .fill(&outbound_packet);
        outbound.commit();
        tokio::task::yield_now().await;

        let inbound_packet = packet(b"evidence");
        client
            .send(Message::binary(encoded(
                WebSocketWireFraming::Kiss,
                &inbound_packet,
            )))
            .await
            .expect("KISS evidence reaches the session");
        let received = tokio::time::timeout(std::time::Duration::from_secs(1), client.next())
            .await
            .expect("evidence resolves framing before the fallback")
            .expect("the websocket remains open")
            .expect("the websocket message is valid")
            .into_data();
        assert_eq!(
            received,
            encoded(WebSocketWireFraming::Kiss, &outbound_packet)
        );
        task.abort();
    }

    #[tokio::test]
    async fn an_oversized_message_is_rejected_by_the_protocol_layer() {
        let (client_io, server_io) = tokio::io::duplex(SOCKET_BUFFER_LEN);
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(
            server_io,
            Role::Server,
            Some(config(WebSocketFramingSelection::Fixed(
                WebSocketWireFraming::RawPacket,
            ))),
        )
        .await;
        let oversized = std::vec![0u8; websocket::FRAME_CAP + 1];
        let sending = tokio::spawn(async move { client.send(Message::binary(oversized)).await });
        let received = server.next().await;
        sending.abort();
        let _ = sending.await;
        assert!(matches!(
            received,
            Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(_)))
        ));
    }
}
