use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;

use crate::engine::InstantMillis;
use crate::interfaces::websocket::core;
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::InterfaceSeam;
use crate::reactor::throughput::ThroughputLedger;

pub async fn serve<S, Seam>(
    mut socket: WebSocketStream<S>,
    seam: &mut Seam,
    status: &TokioInterfaceStatus,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    bitrate_bps: Option<u32>,
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
                        if frame.is_empty() || frame.len() > core::FRAME_CAP {
                            continue;
                        }
                        status.add_rx(frame.len() as u64);
                        let now = InstantMillis(started.elapsed().as_millis() as u64);
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
                if outbound.len() > core::FRAME_CAP {
                    continue;
                }
                let frame = outbound.to_vec();
                let len = frame.len();
                if socket.send(Message::binary(frame)).await.is_err() {
                    return;
                }
                status.add_tx(len as u64);
                let now = InstantMillis(started.elapsed().as_millis() as u64);
                throughput.record_tx(now, len as u64);
                status.set_transfer_rates(throughput.rates());
                if let Some(bitrate_bps) = bitrate_bps {
                    let frame_airtime = frame_airtime_us(len, bitrate_bps);
                    status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
}
