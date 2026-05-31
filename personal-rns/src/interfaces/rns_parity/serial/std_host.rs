//! The std serial worker shell — runs the RNS `SerialInterface` over any
//! blocking [`std::io`] byte stream (a `serialport`, a UART, a TCP stream, a
//! test pipe). The host twin of [`embassy`](super::embassy): same shared `core`
//! framing, expressed with std threads + channels instead of async.
//!
//! The handle ([`StdSerialInterface`]) is what the manifold holds and routes to;
//! the byte I/O runs in [`run`], which a host calls on its own thread.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Instant;
use std::vec::Vec;

use super::core::{descriptor, SERIAL_MTU};
use crate::engine::InstantMillis;
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    InterfaceDescriptor, InterfaceId, InterfaceStats, InterfaceWorker, QueueFull,
};
use crate::runtime::manifold::impls::std_host::InboxEntry;

/// Outbound packets are raw Reticulum wire packets (the shell frames them); the
/// manifold fills this, the shell drains it.
pub type OutboundSender = Sender<Vec<u8>>;
pub type OutboundReceiver = Receiver<Vec<u8>>;

/// Shared liveness: the shell sets it from whether a peer drains the link, the
/// handle's [`health`](StdSerialInterface::health) reads it. `Arc<AtomicBool>`
/// because the handle (runtime thread) and shell (worker thread) share it.
pub type LinkUp = Arc<AtomicBool>;

/// The worker handle for a serial link on interface `id`. Cheap — a descriptor,
/// the outbound sender, and the shared liveness flag. All byte I/O runs in [`run`].
pub struct StdSerialInterface {
    descriptor: InterfaceDescriptor,
    outbound: OutboundSender,
    link_up: LinkUp,
}

impl StdSerialInterface {
    pub fn new(id: InterfaceId, outbound: OutboundSender, link_up: LinkUp) -> Self {
        Self {
            descriptor: descriptor(id),
            outbound,
            link_up,
        }
    }
}

impl InterfaceWorker for StdSerialInterface {
    // Required by the trait for embedded mailbox sizing; a std host's mailbox is
    // a `Vec`-backed mpsc channel, so this is informational here.
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
        // The mpsc channel is unbounded; the only failure is a gone receiver
        // (the worker thread exited), which the caller treats like a full queue.
        self.outbound.send(packet.to_vec()).map_err(|_| QueueFull)
    }
}

/// Run one connection of the serial link until the byte stream errors (an
/// unplug), then return so the caller can reopen. Each pass: drain the outbound
/// queue (frame + write each packet), then read a chunk and de-frame it into the
/// runtime mailbox.
///
/// `port` must have a short read timeout (the host sets it) so a quiet link still
/// loops back to service outbound and check liveness. `clock_base` is the shared
/// monotonic reference the runtime measures against, so each stamped frame's
/// `arrived_at` shares the engine's timebase. Pre-frame noise — e.g. a board
/// sharing this CDC between log text and frames — is skipped by the decoder until
/// a `FLAG`, exactly as stock RNS does.
pub fn run<P: Read + Write>(
    mut port: P,
    id: InterfaceId,
    inbound: &Sender<InboxEntry>,
    outbound: &Receiver<Vec<u8>>,
    link_up: &LinkUp,
    clock_base: Instant,
) {
    let mut decoder: RnsSerialDecoder<SERIAL_MTU> = RnsSerialDecoder::new();
    let mut frame_buf = [0u8; rns_serial_framing::max_encoded_len(SERIAL_MTU)];
    let mut read_buf = [0u8; 64];

    loop {
        // Drain outbound first: frame each packet and write the whole frame. A
        // failed write means the link is gone → return so the caller reopens.
        loop {
            match outbound.try_recv() {
                Ok(packet) => match rns_serial_framing::encode(&packet, &mut frame_buf) {
                    Ok(n) => {
                        if port.write_all(&frame_buf[..n]).is_ok() {
                            link_up.store(true, Ordering::Relaxed);
                        } else {
                            link_up.store(false, Ordering::Relaxed);
                            return;
                        }
                    }
                    // Oversize packet: drop it (self-heals — RNS re-announces).
                    Err(_) => {}
                },
                Err(TryRecvError::Empty) => break,
                // The manifold's sender is gone — the host is shutting down.
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // Read a chunk and feed the decoder; each closed non-empty frame is a
        // Reticulum packet → stamp it into the shared mailbox.
        match port.read(&mut read_buf) {
            Ok(0) => {}
            Ok(n) => {
                for &byte in &read_buf[..n] {
                    if let Ok(Some(frame)) = decoder.feed(byte) {
                        if !frame.is_empty() {
                            // A peer is sending → the link is alive.
                            link_up.store(true, Ordering::Relaxed);
                            let entry = InboxEntry {
                                arrived_at: InstantMillis(
                                    clock_base.elapsed().as_millis() as u64
                                ),
                                source: id,
                                bytes: frame.to_vec(),
                            };
                            // A gone receiver means the runtime stopped; exit.
                            if inbound.send(entry).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            // Idle read window (timeout) — loop back to service outbound.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) => {}
            // Anything else is a real transport error (unplug) → reopen.
            Err(_) => {
                link_up.store(false, Ordering::Relaxed);
                return;
            }
        }
    }
}
