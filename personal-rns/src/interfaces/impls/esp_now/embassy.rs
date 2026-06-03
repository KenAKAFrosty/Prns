//! The embassy ESP-NOW worker runs the Personal-native ESP-NOW broadcast
//! interface over any radio that implements [`EspNowLink`]. The host builds the
//! ESP-NOW endpoint, such as esp-radio's split sender/receiver on the WiFi
//! radio's STA channel, and adapts it to the trait, so this worker stays
//! HAL-agnostic: `personal-rns` names no esp-radio type and pulls no
//! chip-specific dependency.
//!
//! Like the other workers, the outbound queue lives here and the inbound mailbox
//! it stamps into belongs to the runtime. ESP-NOW is a
//! connectionless broadcast medium, so the loop is simpler than LoRa's — there is
//! no half-duplex prepare/tx dance. It awaits either an inbound frame or an
//! outbound packet; on a packet it **coalesces** — packing every packet queued
//! within a short window into one v2 frame ([`super::core`]) before a single
//! broadcast — and a received frame un-coalesces into N whole packets, each
//! stamped into the shared mailbox.

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant as EmbassyInstant, Timer};

use super::core::{decode_frame, EspNowFrameWriter, ESP_NOW_MAX_FRAME_PAYLOAD};
use crate::interfaces::{InboundSink, InterfaceWorkerContext};
use crate::interfaces::substrate::EmbassyHostSubstrate;
use crate::wire::MTU;

/// How long to keep packing a frame after its first packet before transmitting.
/// Coalescing trades this much latency for far fewer transmissions when the
/// engine emits a burst. One millisecond is short next to a frame's airtime and
/// long enough to catch a same-cycle burst.
const COALESCE_LINGER: Duration = Duration::from_millis(1);

/// The radio trait the worker drives: broadcast one frame, await the next one.
/// Implemented by the host over its ESP-NOW endpoint (esp-radio on the S3 / C6),
/// so this crate stays free of any chip HAL. Both methods are `async` and not
/// `Send`-bounded — the worker runs on the host's single embassy executor,
/// joined with the other workers, never sent across threads.
#[allow(async_fn_in_trait)]
pub trait EspNowLink {
    /// What a failed broadcast reports; surfaced in a log line only.
    type Error: core::fmt::Debug;

    /// Broadcast one frame (≤ [`ESP_NOW_MAX_FRAME_PAYLOAD`]) to every neighbor.
    async fn broadcast(&mut self, frame: &[u8]) -> Result<(), Self::Error>;

    /// Await the next received frame, copy it into `buf` (sized to
    /// [`ESP_NOW_MAX_FRAME_PAYLOAD`], so no truncation), and return its length.
    async fn receive_into(&mut self, buf: &mut [u8]) -> usize;
}

/// Un-coalesce one received frame and [`submit`](InboundSink::submit) each whole
/// packet into the interface's inbound ring (the seam stamps arrival + tags the
/// source). The contract-seam twin of [`ingest_frame`]; a malformed frame drops
/// what it can't parse — the engine validates every packet downstream regardless.
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

/// Drive the ESP-NOW link forever over the contract seam. A connectionless broadcast
/// loop: await either a received frame
/// (un-coalesce → `submit` each packet) or the first outbound packet; on a packet,
/// pack every packet queued within [`COALESCE_LINGER`] into one v2 frame, then
/// broadcast once. A packet that doesn't fit starts the next frame (held in
/// `leftover_buf`) so nothing is lost.
///
/// Generic over the seam `MAX_BUFFERED_PACKETS` and the [`EspNowLink`] radio. Inbound packets are
/// submitted through [`InboundSink::submit`]; outbound packets are pulled with
/// [`ready`](crate::interfaces::substrate::EmbassyOutboundDrain::ready) +
/// `try_next_into`.
pub async fn serve<const MAX_BUFFERED_PACKETS: usize, L>(
    mut link: L,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
) where
    L: EspNowLink,
{
    let mut rx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    let mut tx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    // Scratch for one packet pulled off the outbound ring, and the held-back packet
    // that starts the next frame.
    let mut pkt_buf = [0u8; MTU];
    let mut leftover_buf = [0u8; MTU];
    let mut leftover: Option<usize> = None;

    loop {
        // The first packet of the next frame: one held back last time, or while
        // idle, whichever arrives first: an inbound frame or a fresh outbound
        // packet pulled off the ring.
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

        // Coalesce: pack everything queued, then wait up to one fixed window for
        // stragglers, until the frame fills or the window closes.
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
                Either::First(_) => break, // window closed → transmit
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
