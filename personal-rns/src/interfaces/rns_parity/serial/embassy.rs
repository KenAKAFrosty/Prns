//! The embassy serial worker shell — runs the RNS `SerialInterface` over any
//! async byte stream. The host hands it the two halves of its transport (e.g.
//! the ESP32 usb-serial-jtag rx/tx), so this stays hardware-agnostic: it names
//! [`embedded_io_async`], never a HAL.
//!
//! Like the auto-interface shell, the outbound queue lives here (the shell is
//! its draining end); the inbound mailbox the shell *stamps into* belongs to the
//! runtime (`runtime::manifold::impls::embassy`).

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{with_timeout, Duration, Instant as EmbassyInstant};
use embedded_io_async::{Read, Write};
use heapless::Vec as HVec;

use super::core::{descriptor, SERIAL_MTU};
use crate::engine::InstantMillis;
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    InterfaceDescriptor, InterfaceId, InterfaceStats, InterfaceWorker, QueueFull,
};
use crate::runtime::manifold::impls::embassy::{InboundSender, InboxEntry};

pub const OUTBOX_DEPTH: usize = 4;
/// Upper bound on one frame's write. If nothing is draining the CDC (e.g. the
/// board is on battery with no USB host attached), an unbounded write would
/// block the link forever; on timeout we drop the frame instead — RNS
/// re-announces, so it self-heals — and get back to reading.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);
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
            online: self.link_up.load(Ordering::Relaxed),
            ..InterfaceStats::default()
        }
    }

    fn submit(&mut self, packet: &[u8]) -> Result<(), QueueFull> {
        let buf = PacketBuf::from_slice(packet).map_err(|_| QueueFull)?;
        self.outbound.try_send(buf).map_err(|_| QueueFull)
    }
}

fn now_millis() -> InstantMillis {
    InstantMillis(EmbassyInstant::now().as_millis())
}

/// Drive the serial link forever: de-frame inbound bytes off `rx` into the
/// runtime's shared inbound mailbox, and frame outbound packets from the worker
/// queue onto `tx`.
///
/// Generic over `MAILBOX` — the host's `PACKET_BUFFER_SIZE`, which is ≥
/// [`SERIAL_MTU`] — so several worker kinds can stamp into one shared mailbox;
/// and over the byte stream, so any [`embedded_io_async`] transport works (USB
/// CDC, a UART, a test pipe). `id` stamps provenance on each inbound frame.
/// `link_up` goes true once running (a raw serial cable has no link-state to
/// poll; a host with a real disconnect signal can clear it). Pre-frame noise —
/// e.g. a board sharing this CDC between log text and frames — is skipped by the
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
    link_up.store(true, Ordering::Relaxed);
    let mut decoder: RnsSerialDecoder<SERIAL_MTU> = RnsSerialDecoder::new();
    let mut read_buf = [0u8; 64];
    let mut frame_buf = [0u8; rns_serial_framing::max_encoded_len(SERIAL_MTU)];

    loop {
        match select(rx.read(&mut read_buf), outbound.receive()).await {
            // Inbound bytes: feed the decoder; each closed non-empty frame is a
            // Reticulum packet → stamp it into the shared mailbox.
            Either::First(result) => {
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
            // Outbound packet: frame it and write the whole frame to the wire,
            // bounded by WRITE_TIMEOUT so a stalled (unread) link can't wedge us.
            Either::Second(packet) => match rns_serial_framing::encode(&packet, &mut frame_buf) {
                Ok(m) => match with_timeout(WRITE_TIMEOUT, tx.write_all(&frame_buf[..m])).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => {
                        log::warn!("RNS_SERIAL TX write failed, dropped {}B", packet.len())
                    }
                    Err(_) => {
                        log::warn!("RNS_SERIAL TX stalled (no reader?), dropped {}B", packet.len())
                    }
                },
                Err(_) => log::warn!("RNS_SERIAL packet {}B exceeds serial MTU", packet.len()),
            },
        }
    }
}
