use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::WebSocketStream;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::websocket::{WebSocketWireDecoder, WebSocketWireFraming};
use prns_core::interfaces::BitrateBps;
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::InterfaceSeam;
use prns_runtime::manifold::throughput::ThroughputLedger;

const SOCKET_BUFFER_LEN: usize = 16 * 1024;

pub(crate) struct SessionConfig {
    bitrate: BitrateBps,
    started: tokio::time::Instant,
    wire_framing: WebSocketWireFraming,
}

impl SessionConfig {
    pub(crate) fn new(
        bitrate: BitrateBps,
        started: tokio::time::Instant,
        wire_framing: WebSocketWireFraming,
    ) -> Self {
        Self {
            bitrate,
            started,
            wire_framing,
        }
    }
}

pub(crate) fn config(framing: WebSocketWireFraming) -> WebSocketConfig {
    let message_cap = framing.message_cap();
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
    let mut decoder = WebSocketWireDecoder::new(config.wire_framing);
    loop {
        tokio::select! {
            inbound = socket.next() => {
                let Some(inbound) = inbound else {
                    return;
                };
                let message = match inbound {
                    Ok(message) => message,
                    Err(_) => return,
                };
                match message {
                    Message::Binary(frame) => {
                        if frame.is_empty() || frame.len() > config.wire_framing.message_cap() {
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
                            match decoder.next_frame_into(&frame, &mut offset, sink) {
                                Ok(Some(_)) => seam.commit_inbound().await,
                                Ok(None) => break,
                                Err(_) => {}
                            }
                        }
                    }
                    Message::Text(_)
                    | Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Frame(_) => {}
                    Message::Close(_) => return,
                }
            }
            outbound = seam.next_outbound() => {
                let Some((message, encoded_len)) = wire_message(config.wire_framing, outbound) else {
                    continue;
                };
                if socket.send(message).await.is_err() {
                    return;
                }
                status.add_tx(encoded_len as u64);
                let elapsed = u64::try_from(config.started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let now = InstantMillis(elapsed);
                throughput.record_tx(now, encoded_len as u64);
                status.set_transfer_rates(throughput.rates());
                let frame_airtime = frame_airtime_us(encoded_len, config.bitrate);
                status.set_airtime(airtime.record_tx(now, frame_airtime));
            }
        }
    }
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
    use prns_core::interfaces::websocket;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[test]
    fn websocket_buffers_and_inbound_messages_are_bounded() {
        for framing in [
            WebSocketWireFraming::RawPacket,
            WebSocketWireFraming::Hdlc,
            WebSocketWireFraming::Kiss,
        ] {
            let config = config(framing);
            assert_eq!(config.read_buffer_size, SOCKET_BUFFER_LEN);
            assert_eq!(config.write_buffer_size, SOCKET_BUFFER_LEN);
            assert_eq!(config.max_message_size, Some(framing.message_cap()));
            assert_eq!(config.max_frame_size, Some(framing.message_cap()));
            assert!(
                config.max_write_buffer_size > config.write_buffer_size + framing.message_cap()
            );
        }
    }

    #[tokio::test]
    async fn an_oversized_message_is_rejected_by_the_protocol_layer() {
        let (client_io, server_io) = tokio::io::duplex(SOCKET_BUFFER_LEN);
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(
            server_io,
            Role::Server,
            Some(config(WebSocketWireFraming::RawPacket)),
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
