use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant as EmbassyInstant, Timer};

use super::core::{decode_frame, EspNowFrameWriter, ESP_NOW_MAX_FRAME_PAYLOAD};
use crate::interfaces::substrate::EmbassyHostSubstrate;
use crate::interfaces::{InboundSink, InterfaceWorkerContext};
use crate::wire::MTU;

/// How long to keep packing a frame after its first packet before transmitting.
/// Coalescing trades this much latency for far fewer transmissions when the
/// engine emits a burst. One millisecond is short next to a frame's airtime and
/// long enough to catch a same-cycle burst.
const COALESCE_LINGER: Duration = Duration::from_millis(1);

/// Both methods are `async` and not
/// `Send`-bounded — the worker runs on the host's single embassy executor,
/// joined with the other workers, never sent across threads.
#[allow(async_fn_in_trait)]
pub trait EspNowLink {
    type Error: core::fmt::Debug;

    async fn broadcast(&mut self, frame: &[u8]) -> Result<(), Self::Error>;

    async fn receive_into(&mut self, buf: &mut [u8]) -> usize;
}

fn submit_frame_packets(frame: &[u8], inbound: &mut impl InboundSink) {
    match decode_frame(frame) {
        Ok(reader) => {
            let mut stamped = 0usize;
            for packet in reader {
                if packet.is_empty() || packet.len() > MTU {
                    continue;
                }
                match inbound.submit(|buf| {
                    buf[..packet.len()].copy_from_slice(packet);
                    packet.len()
                }) {
                    Ok(()) => stamped += 1,
                    Err(_) => {
                        log::warn!("RNS_ESPNOW inbound ring full, dropped {}B", packet.len())
                    }
                }
            }
            if stamped > 0 {
                log::info!(
                    "RNS_ESPNOW rx frame: {stamped} packet(s) in {}B",
                    frame.len()
                );
            }
        }
        Err(e) => log::warn!("RNS_ESPNOW dropping malformed frame: {e:?}"),
    }
}

pub async fn serve<const MAX_BUFFERED_PACKETS: usize, L>(
    mut link: L,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
) where
    L: EspNowLink,
{
    let mut rx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    let mut tx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    let mut pkt_buf = [0u8; MTU];
    let mut leftover_buf = [0u8; MTU];
    let mut leftover: Option<usize> = None;

    loop {
        let first_len = match leftover.take() {
            Some(len) => {
                pkt_buf[..len].copy_from_slice(&leftover_buf[..len]);
                len
            }
            None => match select(link.receive_into(&mut rx_buf), context.outbound.ready()).await {
                Either::First(len) => {
                    submit_frame_packets(&rx_buf[..len], &mut context.inbound);
                    continue;
                }
                Either::Second(()) => match context.outbound.try_next_into(&mut pkt_buf) {
                    Some(len) => len,
                    None => continue,
                },
            },
        };

        let mut writer = EspNowFrameWriter::new(&mut tx_buf);
        if !writer.try_push(&pkt_buf[..first_len]) {
            log::warn!("RNS_ESPNOW packet {first_len}B too large for one frame, dropped");
            continue;
        }

        let deadline = EmbassyInstant::now() + COALESCE_LINGER;
        loop {
            if let Some(len) = context.outbound.try_next_into(&mut pkt_buf) {
                if !writer.try_push(&pkt_buf[..len]) {
                    leftover_buf[..len].copy_from_slice(&pkt_buf[..len]);
                    leftover = Some(len);
                    break;
                }
                continue;
            }
            match select(Timer::at(deadline), context.outbound.ready()).await {
                Either::First(_) => break,
                Either::Second(()) => {
                    if let Some(len) = context.outbound.try_next_into(&mut pkt_buf) {
                        if !writer.try_push(&pkt_buf[..len]) {
                            leftover_buf[..len].copy_from_slice(&pkt_buf[..len]);
                            leftover = Some(len);
                            break;
                        }
                    }
                }
            }
        }

        let packed = writer.packet_count();
        if let Err(e) = link.broadcast(writer.frame()).await {
            log::warn!("RNS_ESPNOW broadcast of {packed} packet(s) failed: {e:?}");
        }
    }
}
