use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::WebSocketStream;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::websocket;
use prns_core::interfaces::BitrateBps;
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::InterfaceSeam;
use prns_runtime::manifold::throughput::ThroughputLedger;

const SOCKET_BUFFER_LEN: usize = 16 * 1024;

pub(crate) fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(SOCKET_BUFFER_LEN)
        .write_buffer_size(SOCKET_BUFFER_LEN)
        .max_write_buffer_size(
            websocket::FRAME_CAP
                .saturating_add(SOCKET_BUFFER_LEN)
                .saturating_add(1),
        )
        .max_message_size(Some(websocket::FRAME_CAP))
        .max_frame_size(Some(websocket::FRAME_CAP))
}

pub async fn serve<S, Seam>(
    mut socket: WebSocketStream<S>,
    seam: &mut Seam,
    status: &TokioInterfaceStatus,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    bitrate: BitrateBps,
    started: tokio::time::Instant,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
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
                        if frame.is_empty() || frame.len() > websocket::FRAME_CAP {
                            continue;
                        }
                        status.add_rx(frame.len() as u64);
                        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                        let now = InstantMillis(elapsed);
                        throughput.record_rx(now, frame.len() as u64);
                        status.set_transfer_rates(throughput.rates());
                        seam.next_inbound(&frame).await;
                    }
                    Message::Text(_)
                    | Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Frame(_) => {}
                    Message::Close(_) => return,
                }
            }
            outbound = seam.next_outbound() => {
                if outbound.len() > websocket::FRAME_CAP {
                    continue;
                }
                let frame = outbound.to_vec();
                let len = frame.len();
                if socket.send(Message::binary(frame)).await.is_err() {
                    return;
                }
                status.add_tx(len as u64);
                let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                let now = InstantMillis(elapsed);
                throughput.record_tx(now, len as u64);
                status.set_transfer_rates(throughput.rates());
                let frame_airtime = frame_airtime_us(len, bitrate);
                status.set_airtime(airtime.record_tx(now, frame_airtime));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[test]
    fn websocket_buffers_and_inbound_messages_are_bounded() {
        let config = config();
        assert_eq!(config.read_buffer_size, SOCKET_BUFFER_LEN);
        assert_eq!(config.write_buffer_size, SOCKET_BUFFER_LEN);
        assert_eq!(config.max_message_size, Some(websocket::FRAME_CAP));
        assert_eq!(config.max_frame_size, Some(websocket::FRAME_CAP));
        assert!(config.max_write_buffer_size > config.write_buffer_size + websocket::FRAME_CAP);
    }

    #[tokio::test]
    async fn an_oversized_message_is_rejected_by_the_protocol_layer() {
        let (client_io, server_io) = tokio::io::duplex(SOCKET_BUFFER_LEN);
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let mut server =
            WebSocketStream::from_raw_socket(server_io, Role::Server, Some(config())).await;
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
