//! The embassy serial worker shell — runs the RNS `SerialInterface` over any
//! async byte stream. The host hands it the two halves of its transport (e.g.
//! the ESP32 usb-serial-jtag rx/tx), so this stays hardware-agnostic: it names
//! [`embedded_io_async`], never a HAL.
//!
//! Like the auto-interface shell, the outbound queue lives here (the shell is
//! its draining end); the inbound mailbox the shell *stamps into* belongs to the
//! runtime (`runtime::manifold::impls::embassy`).

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{with_timeout, Duration, Instant as EmbassyInstant, Ticker};
use embedded_io_async::{Read, Write};
use heapless::Vec as HVec;

use super::core::{descriptor, SERIAL_MTU};
use crate::engine::{InstantMillis, OutboundPacket};
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    InterfaceDescriptor, InterfaceId, InterfaceStats, InterfaceWorker, LinkState, QueueFull,
};
use crate::runtime::manifold::impls::embassy::{InboundSender, InboxEntry};

pub const OUTBOX_DEPTH: usize = 4;
/// Upper bound on one frame's write. If nothing is draining the CDC (no USB host
/// attached, or no peer reading), an unbounded write would block the link
/// forever; on timeout we drop the frame — RNS re-announces, so it self-heals —
/// and treat the link as offline.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);
/// How often to send an empty keepalive frame when the link is otherwise idle.
/// It doubles as the liveness probe: whether the write lands (peer draining) or
/// times out (no peer) sets `health().online`, so the link state tracks
/// plug/unplug within this interval. Empty frames are valid RNS keepalives — a
/// stock peer's decoder yields and discards them.
const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(1000);
/// An empty RNS serial frame: `FLAG FLAG`, the canonical keepalive.
const KEEPALIVE_FRAME: [u8; 2] = [rns_serial_framing::FLAG, rns_serial_framing::FLAG];
/// Outbound packets are raw Reticulum wire packets (≤ [`SERIAL_MTU`]); the shell
/// frames them on the way out.
pub type PacketBuf = HVec<u8, SERIAL_MTU>;

/// Outbound: packets the runtime hands the worker to transmit. The handle holds
/// the [`OutboundSender`] (filled via [`EmbassySerialInterface::submit`]); the
/// shell drains the [`OutboundReceiver`], frames each, and writes it. It lives
/// here with its draining end — the same rule that puts the inbound mailbox with
/// the runtime that drains it.
pub type OutboundChannel = Channel<CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundSender = Sender<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;
pub type OutboundReceiver = Receiver<'static, CriticalSectionRawMutex, PacketBuf, OUTBOX_DEPTH>;

/// A `'static` liveness flag shared between the handle and its shell: the shell
/// sets it true once the link is running and the handle's
/// [`health`](EmbassySerialInterface::health) reads it. A plain `AtomicBool` —
/// one flag, two tasks, no ordering needs.
pub type LinkUp = AtomicBool;

/// The worker handle for a serial link on interface `id`. Cheap — a descriptor,
/// the outbound sender, and the shared liveness flag. All byte I/O runs in the
/// shell ([`run`]).
pub struct EmbassySerialInterface {
    descriptor: InterfaceDescriptor,
    outbound: OutboundSender,
    link_up: &'static LinkUp,
}

impl EmbassySerialInterface {
    /// Build the handle for interface `id`, transmitting over `outbound` and
    /// reading liveness from `link_up` (the shell writes it).
    pub fn new(id: InterfaceId, outbound: OutboundSender, link_up: &'static LinkUp) -> Self {
        Self {
            descriptor: descriptor(id),
            outbound,
            link_up,
        }
    }
}

impl InterfaceWorker for EmbassySerialInterface {
    // The RNS engine MTU. A multi-worker host sizes its shared inbound mailbox to
    // the largest worker's buffer, so this worker's ≤ SERIAL_MTU frames always
    // fit a mailbox sized for it or anything larger.
    const PACKET_BUFFER_SIZE: usize = SERIAL_MTU;

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
        let buf = PacketBuf::from_slice(packet.bytes).map_err(|_| QueueFull)?;
        self.outbound.try_send(buf).map_err(|_| QueueFull)
    }
}

fn now_millis() -> InstantMillis {
    InstantMillis(EmbassyInstant::now().as_millis())
}

/// Write one already-framed buffer, bounded by [`WRITE_TIMEOUT`]. Returns
/// whether a peer drained it: `true` if the whole frame went out, `false` on a
/// write error or timeout (nothing reading) — that boolean is the link's
/// liveness.
async fn write_framed<W: Write>(tx: &mut W, framed: &[u8]) -> bool {
    matches!(
        with_timeout(WRITE_TIMEOUT, tx.write_all(framed)).await,
        Ok(Ok(()))
    )
}

/// Drive the serial link forever: de-frame inbound bytes off `rx` into the
/// runtime's shared inbound mailbox, frame outbound packets from the worker
/// queue onto `tx`, and emit an idle keepalive that doubles as the liveness
/// probe.
///
/// Generic over `MAILBOX` — the host's `PACKET_BUFFER_SIZE`, which is ≥
/// [`SERIAL_MTU`] — so several worker kinds can stamp into one shared mailbox;
/// and over the byte stream, so any [`embedded_io_async`] transport works (USB
/// CDC, a UART, a test pipe). `id` stamps provenance on each inbound frame.
/// `link_up` tracks whether a peer is draining the wire: every write (real or
/// keepalive) sets it from whether the frame landed, so plug/unplug shows up in
/// `health().online` within [`KEEPALIVE_INTERVAL`]. Pre-frame noise — e.g. a
/// board sharing this CDC between log text and frames — is skipped by the
/// decoder until a `FLAG`, exactly as stock RNS does.
pub async fn run<const MAILBOX: usize, R, W>(
    mut rx: R,
    mut tx: W,
    id: InterfaceId,
    inbound: InboundSender<MAILBOX>,
    outbound: OutboundReceiver,
    link_up: &'static LinkUp,
) where
    R: Read,
    W: Write,
{
    // Offline until a write proves a peer is draining the link.
    link_up.store(false, Ordering::Relaxed);
    let mut decoder: RnsSerialDecoder<SERIAL_MTU> = RnsSerialDecoder::new();
    let mut read_buf = [0u8; 64];
    let mut frame_buf = [0u8; rns_serial_framing::max_encoded_len(SERIAL_MTU)];
    let mut keepalive = Ticker::every(KEEPALIVE_INTERVAL);

    loop {
        match select3(rx.read(&mut read_buf), outbound.receive(), keepalive.next()).await {
            // Inbound bytes: feed the decoder; each closed non-empty frame is a
            // Reticulum packet → stamp it into the shared mailbox.
            Either3::First(result) => {
                let n = result.unwrap_or(0);
                for &byte in &read_buf[..n] {
                    match decoder.feed(byte) {
                        Ok(Some(frame)) if !frame.is_empty() => {
                            if let Ok(bytes) = HVec::<u8, MAILBOX>::from_slice(frame) {
                                let entry = InboxEntry {
                                    arrived_at: now_millis(),
                                    source: id,
                                    bytes,
                                };
                                if inbound.try_send(entry).is_err() {
                                    log::warn!(
                                        "RNS_SERIAL inbound mailbox full, dropped {}B",
                                        frame.len()
                                    );
                                }
                            }
                        }
                        // Mid-frame, or an empty FLAG-FLAG keepalive: keep going.
                        Ok(_) => {}
                        Err(rns_serial_framing::DecodeError::FrameTooBig) => {
                            log::warn!("RNS_SERIAL oversize frame dropped, decoder reset");
                        }
                    }
                }
            }
            // Outbound packet: frame it and write it; the write result is the
            // freshest liveness signal, so fold it into `link_up`.
            Either3::Second(packet) => match rns_serial_framing::encode(&packet, &mut frame_buf) {
                Ok(m) => {
                    let drained = write_framed(&mut tx, &frame_buf[..m]).await;
                    link_up.store(drained, Ordering::Relaxed);
                    if !drained {
                        log::warn!("RNS_SERIAL TX dropped {}B (no reader?)", packet.len());
                    }
                }
                Err(_) => log::warn!("RNS_SERIAL packet {}B exceeds serial MTU", packet.len()),
            },
            // Idle keepalive: an empty frame that probes whether a peer is still
            // draining the link, refreshing `link_up` between real packets.
            Either3::Third(_) => {
                let drained = write_framed(&mut tx, &KEEPALIVE_FRAME).await;
                link_up.store(drained, Ordering::Relaxed);
            }
        }
    }
}
