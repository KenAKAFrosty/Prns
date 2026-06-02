//! The embassy ESP-NOW worker shell — runs the Personal-native ESP-NOW broadcast
//! interface over any radio that implements [`EspNowLink`]. The host builds the
//! ESP-NOW endpoint (e.g. esp-radio's split sender/receiver, riding the WiFi
//! radio's STA channel) and adapts it to the trait, so this shell stays
//! HAL-agnostic: `personal-rns` names no esp-radio type and pulls no
//! chip-specific dep — the same dependency-inversion the serial shell uses for
//! byte streams.
//!
//! Like the other worker shells, the outbound queue lives here (the shell drains
//! it); the inbound mailbox it stamps into belongs to the runtime. ESP-NOW is a
//! connectionless broadcast medium, so the loop is simpler than LoRa's — there is
//! no half-duplex prepare/tx dance. It awaits either an inbound frame or an
//! outbound packet; on a packet it **coalesces** — packing every packet queued
//! within a short window into one fat v2 frame ([`super::core`]) before a single
//! broadcast — and a received frame un-coalesces into N whole packets, each
//! stamped into the shared mailbox.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Instant as EmbassyInstant, Timer};
use heapless::Vec as HVec;

use super::core::{decode_frame, descriptor, EspNowFrameWriter, ESP_NOW_MAX_FRAME_PAYLOAD};
use crate::engine::{InstantMillis, OutboundPacket};
use crate::interfaces::{
    InboundSink, InterfaceDescriptor, InterfaceId, InterfaceStats, InterfaceWorker,
    InterfaceWorkerContext, LinkState, QueueFull,
};
use crate::runtime::channels::embassy::{InboundSender, InboxEntry};
use crate::runtime::channels::embassy_seam::EmbassyHostSubstrate;
use crate::wire::MTU;

/// How long to keep packing a frame after its first packet before transmitting.
/// Coalescing trades this much latency for far fewer transmissions when the
/// engine emits a burst (announces fanning out, a reply train). One millisecond
/// is a desk-tuned starting point — short next to a frame's airtime, long enough
/// to catch a same-cycle burst; tune on-device.
const COALESCE_LINGER: Duration = Duration::from_millis(1);

/// Outbound queue depth — deeper than a point-to-point shell's so a burst has
/// room to accumulate and coalesce into one frame rather than spilling.
pub const OUTBOX_DEPTH: usize = 8;

/// Outbound packets are raw Reticulum wire packets (≤ engine [`MTU`]); the shell
/// coalesces however many are queued into one ESP-NOW frame on the way out.
pub type PacketBuf = HVec<u8, MTU>;

/// Outbound: packets the runtime hands the worker to transmit. The handle holds
/// the [`OutboundSender`]; the shell drains the [`OutboundReceiver`], coalesces,
/// and broadcasts. It lives here with its draining end — the same rule that puts
/// the inbound mailbox with the runtime that drains it.
pub type OutboundChannel = Channel<CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundSender = Sender<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundReceiver = Receiver<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;

/// A `'static` liveness flag shared between the handle and its shell: true while
/// the radio is operating. ESP-NOW is connectionless, so there is no per-peer
/// link state — `online` just means "the ESP-NOW endpoint is up".
pub type LinkUp = AtomicBool;

/// The radio seam the shell drives: broadcast one frame, await the next one.
/// Implemented by the host over its ESP-NOW endpoint (esp-radio on the S3 / C6),
/// so this crate stays free of any chip HAL. Both methods are `async` and not
/// `Send`-bounded — the worker runs on the host's single embassy executor,
/// joined with the other shells, never sent across threads.
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

/// The worker handle for an ESP-NOW interface on `id`. Cheap — a descriptor, the
/// outbound sender, and the shared liveness flag. The radio I/O runs in [`run`].
pub struct EmbassyEspNowInterface {
    descriptor: InterfaceDescriptor,
    outbound: OutboundSender,
    link_up: &'static LinkUp,
}

impl EmbassyEspNowInterface {
    pub fn new(id: InterfaceId, outbound: OutboundSender, link_up: &'static LinkUp) -> Self {
        Self {
            descriptor: descriptor(id),
            outbound,
            link_up,
        }
    }
}

impl InterfaceWorker for EmbassyEspNowInterface {
    // The engine MTU: each *un-coalesced* inbound packet is one whole Reticulum
    // packet (≤ MTU), so a mailbox sized to this always fits one — the coalesced
    // frame is bigger, but it never reaches the mailbox whole.
    const PACKET_BUFFER_SIZE: usize = MTU;

    fn descriptor(&self) -> InterfaceDescriptor {
        self.descriptor
    }

    fn health(&self) -> InterfaceStats {
        InterfaceStats {
            link: LinkState::from_up(self.link_up.load(Ordering::Relaxed)),
            ..InterfaceStats::default()
        }
    }

    fn submit(&mut self, packet: OutboundPacket) -> Result<(), QueueFull> {
        // `from_slice` rejects a packet larger than the engine MTU; the caller
        // treats that like a full queue and re-emits later.
        let buf = PacketBuf::from_slice(packet.bytes).map_err(|_| QueueFull)?;
        self.outbound.try_send(buf).map_err(|_| QueueFull)
    }
}

fn now_millis() -> InstantMillis {
    InstantMillis(EmbassyInstant::now().as_millis())
}

/// Un-coalesce one received frame and stamp each whole packet into the shared
/// mailbox. A malformed frame (bad version, truncated tail) drops what it can't
/// parse — the engine validates every packet downstream regardless.
fn ingest_frame<const MAILBOX: usize>(
    frame: &[u8],
    id: InterfaceId,
    inbound: &InboundSender<MAILBOX>,
) {
    match decode_frame(frame) {
        Ok(reader) => {
            let mut stamped = 0usize;
            for packet in reader {
                if packet.is_empty() {
                    continue;
                }
                match HVec::<u8, MAILBOX>::from_slice(packet) {
                    Ok(bytes) => {
                        let entry = InboxEntry {
                            arrived_at: now_millis(),
                            source: id,
                            bytes,
                        };
                        if inbound.try_send(entry).is_err() {
                            log::warn!(
                                "RNS_ESPNOW inbound mailbox full, dropped {}B",
                                packet.len()
                            );
                        } else {
                            stamped += 1;
                        }
                    }
                    Err(_) => {
                        log::warn!("RNS_ESPNOW packet {}B exceeds mailbox", packet.len())
                    }
                }
            }
            // The direct OTA proof, and the coalescing factor made visible: one
            // received frame carried `stamped` whole packets.
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

/// Drive the ESP-NOW link forever. While idle, await either an inbound frame
/// (un-coalesce → stamp) or the first outbound packet. On an outbound packet,
/// open a frame, then drain the queue and linger briefly ([`COALESCE_LINGER`]),
/// packing every packet that fits into the one frame before a single broadcast;
/// a packet that doesn't fit is held for the next frame so nothing is lost.
///
/// Generic over `MAILBOX` (the host's `PACKET_BUFFER_SIZE`, ≥ this worker's) so
/// several worker kinds share one mailbox, and over the [`EspNowLink`] radio so
/// any host (S3, C6) plugs in its esp-radio endpoint. `id` stamps provenance on
/// each inbound packet.
pub async fn run<const MAILBOX: usize, L>(
    mut link: L,
    id: InterfaceId,
    inbound: InboundSender<MAILBOX>,
    outbound: OutboundReceiver,
    link_up: &'static LinkUp,
) where
    L: EspNowLink,
{
    link_up.store(true, Ordering::Relaxed);

    let mut rx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    let mut tx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    // A packet packed off the queue that didn't fit the frame just sent; it leads
    // the next frame so it's never dropped.
    let mut leftover: Option<PacketBuf> = None;

    loop {
        // The first packet of the next frame: one held back last time, or — while
        // idle — whichever comes first, an inbound frame (ingest and loop) or a
        // fresh outbound packet.
        let first_out = match leftover.take() {
            Some(pkt) => pkt,
            None => match select(link.receive_into(&mut rx_buf), outbound.receive()).await {
                Either::First(len) => {
                    ingest_frame::<MAILBOX>(&rx_buf[..len], id, &inbound);
                    continue;
                }
                Either::Second(pkt) => pkt,
            },
        };

        let mut writer = EspNowFrameWriter::new(&mut tx_buf);
        if !writer.try_push(&first_out) {
            // Unreachable for ≤ MTU packets (an MTU packet is tiny next to a
            // frame); guard so an oversize packet can't loop forever as leftover.
            log::warn!(
                "RNS_ESPNOW packet {}B too large for one frame, dropped",
                first_out.len()
            );
            continue;
        }

        // Coalesce: collect everything queued, then wait up to one fixed window
        // for stragglers, until the frame fills or the window closes.
        let deadline = EmbassyInstant::now() + COALESCE_LINGER;
        loop {
            if let Ok(pkt) = outbound.try_receive() {
                if !writer.try_push(&pkt) {
                    leftover = Some(pkt);
                    break;
                }
                continue;
            }
            match select(Timer::at(deadline), outbound.receive()).await {
                Either::First(_) => break, // window closed → transmit
                Either::Second(pkt) => {
                    if !writer.try_push(&pkt) {
                        leftover = Some(pkt);
                        break;
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

/// Drive the ESP-NOW link forever over the contract seam — the going-forward twin of
/// [`run`]. Same connectionless broadcast loop: await either a received frame
/// (un-coalesce → `submit` each packet) or the first outbound packet; on a packet,
/// pack every packet queued within [`COALESCE_LINGER`] into one fat v2 frame, then
/// broadcast once. A packet that doesn't fit leads the next frame (held in
/// `leftover_buf`) so nothing is lost.
///
/// Generic over the seam `DEPTH` and the [`EspNowLink`] radio. Re-plumbed to the seam:
/// inbound rides [`InboundSink::submit`], outbound is pulled with
/// [`ready`](crate::runtime::channels::embassy_seam::EmbassyOutboundDrain::ready) +
/// `try_next_into` (the copy-out the async write needs). No `link_up` — liveness rides
/// a control report under the new contract (deferred).
pub async fn serve<const DEPTH: usize, L>(
    mut link: L,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, DEPTH>>,
) where
    L: EspNowLink,
{
    let mut rx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    let mut tx_buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
    // Scratch for one packet pulled off the outbound ring, and the held-back packet
    // that leads the next frame (the copy-out flavor of the legacy owned `leftover`).
    let mut pkt_buf = [0u8; MTU];
    let mut leftover_buf = [0u8; MTU];
    let mut leftover: Option<usize> = None;

    loop {
        // The first packet of the next frame: one held back last time, or — while
        // idle — whichever comes first, a received frame (ingest and loop) or a fresh
        // outbound packet pulled off the ring.
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
