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
use crate::engine::{InstantMillis, OutboundPacket};
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    InterfaceDescriptor, InterfaceId, InterfaceStats, InterfaceWorker, LinkState, QueueFull,
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
            link: LinkState::from_up(self.link_up.load(Ordering::Relaxed)),
            ..InterfaceStats::default()
        }
    }

    fn submit(&mut self, packet: OutboundPacket) -> Result<(), QueueFull> {
        // The mpsc channel is unbounded; the only failure is a gone receiver
        // (the worker thread exited), which the caller treats like a full queue.
        self.outbound
            .send(packet.bytes.to_vec())
            .map_err(|_| QueueFull)
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
                                arrived_at: InstantMillis(clock_base.elapsed().as_millis() as u64),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::mpsc;
    use std::sync::Mutex;

    use crate::interfaces::rns_serial_framing::{self, ESC, FLAG};

    /// In-memory byte pipe: serves preloaded `rx` bytes, then errors so [`run`]
    /// returns (a simulated unplug / end of stream); captures all writes into a
    /// handle the test keeps after `run` consumes the port.
    struct MockPort {
        rx: Vec<u8>,
        pos: usize,
        tx: std::sync::Arc<Mutex<Vec<u8>>>,
    }

    impl MockPort {
        fn new(rx: Vec<u8>) -> (Self, std::sync::Arc<Mutex<Vec<u8>>>) {
            let tx = std::sync::Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    rx,
                    pos: 0,
                    tx: tx.clone(),
                },
                tx,
            )
        }
    }

    impl io::Read for MockPort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.rx.len() {
                // Stream exhausted → error so the worker's read loop returns.
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let n = (self.rx.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.rx[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    impl io::Write for MockPort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.tx.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_id() -> InterfaceId {
        InterfaceId::new([0xD0; 16])
    }

    #[test]
    fn deframes_inbound_bytes_and_stamps_them_into_the_mailbox() {
        // Payload includes FLAG and ESC so the round-trip exercises unstuffing.
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let mut framed = [0u8; 32];
        let n = rns_serial_framing::encode(&payload, &mut framed).unwrap();

        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (_outbound_tx, outbound_rx) = mpsc::channel::<Vec<u8>>();
        let link_up: LinkUp = std::sync::Arc::new(AtomicBool::new(false));
        let (port, _tx) = MockPort::new(framed[..n].to_vec());

        run(
            port,
            test_id(),
            &inbound_tx,
            &outbound_rx,
            &link_up,
            Instant::now(),
        );

        let entry = inbound_rx
            .try_recv()
            .expect("the worker stamped one frame into the mailbox");
        assert_eq!(entry.bytes, payload);
        assert_eq!(entry.source, test_id());
    }

    #[test]
    fn frames_an_outbound_packet_onto_the_wire() {
        // Packet contains a FLAG so the framing must escape it.
        let packet = [0xAAu8, FLAG, 0xBB];
        let (inbound_tx, _inbound_rx) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel::<Vec<u8>>();
        outbound_tx.send(packet.to_vec()).unwrap();
        let link_up: LinkUp = std::sync::Arc::new(AtomicBool::new(false));
        // Empty rx → the first read errors, but the outbound drain runs first.
        let (port, written) = MockPort::new(Vec::new());

        run(
            port,
            test_id(),
            &inbound_tx,
            &outbound_rx,
            &link_up,
            Instant::now(),
        );

        // De-frame what was written; it must reconstruct the original packet.
        let bytes = written.lock().unwrap().clone();
        let mut decoder = RnsSerialDecoder::<SERIAL_MTU>::new();
        let mut decoded = None;
        for byte in bytes {
            if let Ok(Some(frame)) = decoder.feed(byte) {
                if !frame.is_empty() {
                    decoded = Some(frame.to_vec());
                }
            }
        }
        assert_eq!(decoded.expect("a framed packet was written"), packet);
    }
}
