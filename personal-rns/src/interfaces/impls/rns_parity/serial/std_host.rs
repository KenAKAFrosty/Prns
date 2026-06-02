//! The std serial worker shell — runs the RNS `SerialInterface` over any blocking
//! [`std::io`] byte stream (a `serialport`, a UART, a TCP stream, a test pipe).
//! The host twin of the `embassy` shell: same shared `core` framing, expressed
//! with a std thread + the contract seam instead of async.
//!
//! [`std_serial_interface`] builds a [`SelfDrivenInterface`] whose launch closure
//! spawns a thread that owns the device lifecycle — open, run a connection,
//! reconnect on unplug — running the read→deframe→submit / drain→frame→write loop
//! against the worker side of the seam it is handed. It never names a HAL: the
//! caller supplies an `open` closure that hands it a fresh `Read + Write` stream, so
//! the same shell serves USB-CDC, a UART, or a test pipe.

use std::io::{self, Read, Write};
use std::time::Duration;

use super::core::{descriptor, SERIAL_MTU};
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceId,
    InterfaceWorkerContext, OutboundDrain, SelfDrivenInterface,
};
use crate::runtime::channels::std_host::StdHostSubstrate;

/// The worker-side seam this shell runs against — the std substrate sized to the
/// serial MTU.
type SerialContext = InterfaceWorkerContext<StdHostSubstrate<SERIAL_MTU>>;

/// Build a self-driven serial [`Interface`](crate::interfaces::Interface) on
/// interface `id`. The returned [`SelfDrivenInterface`]'s launch closure spawns a
/// thread that owns the device lifecycle — open, `serve_connection`, reconnect on
/// unplug — running the loop against the worker side of the seam. `open` is called
/// to (re)acquire the byte stream (a caller closes `serialport` or any HAL inside
/// it, so this shell never names one); `reconnect` is the backoff before re-opening
/// after an unplug or open failure.
pub fn std_serial_interface<Open, Port>(
    id: InterfaceId,
    open: Open,
    reconnect: Duration,
) -> SelfDrivenInterface<impl FnOnce(SerialContext)>
where
    Open: FnMut() -> io::Result<Port> + Send + 'static,
    Port: Read + Write + Send + 'static,
{
    SelfDrivenInterface::new(descriptor(id), move |context| {
        std::thread::spawn(move || serve_until_stopped(open, reconnect, context));
    })
}

/// Own one serial link for the life of the interface: (re)open via `open`, serve the
/// connection until it drops or a stop arrives, back off, repeat — reporting
/// [`Stopped`](ControlReport::Stopped) once a [`Stop`](ControlCommand::Stop) ends the
/// loop. Runs on the thread the launch closure spawned.
fn serve_until_stopped<Open, Port>(mut open: Open, reconnect: Duration, mut context: SerialContext)
where
    Open: FnMut() -> io::Result<Port>,
    Port: Read + Write,
{
    loop {
        match open() {
            Ok(port) => match serve_connection(port, &mut context) {
                ConnectionEnd::Stopped => break,
                ConnectionEnd::Disconnected => {}
            },
            // Open failed (not plugged in yet); back off and retry.
            Err(_) => {}
        }
        // Honor a stop issued while we're between connections, too.
        if matches!(context.control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }
        std::thread::sleep(reconnect);
    }
    context.control.report(ControlReport::Stopped);
}

/// Why one connection ended: the runtime asked us to stop, or the transport died
/// (an unplug) and the caller should reconnect.
enum ConnectionEnd {
    Stopped,
    Disconnected,
}

/// Run one connection until the byte stream errors (an unplug) or a stop is
/// requested. Each pass: check for a stop, drain the outbound seam (frame + write
/// each packet), then read a chunk and de-frame it into the inbound seam.
///
/// `port` must have a short read timeout (the host's `open` sets it) so a quiet
/// link still loops back to service outbound and check for a stop. Pre-frame noise
/// — e.g. a board sharing this CDC between log text and frames — is skipped by the
/// decoder until a `FLAG`, exactly as stock RNS does.
fn serve_connection<Port: Read + Write>(
    mut port: Port,
    context: &mut SerialContext,
) -> ConnectionEnd {
    let mut decoder: RnsSerialDecoder<SERIAL_MTU> = RnsSerialDecoder::new();
    let mut frame_buf = [0u8; rns_serial_framing::max_encoded_len(SERIAL_MTU)];
    let mut read_buf = [0u8; 64];

    loop {
        if matches!(context.control.next_command(), Some(ControlCommand::Stop)) {
            return ConnectionEnd::Stopped;
        }

        // Drain outbound first: frame each packet and write the whole frame. A
        // failed write means the link is gone → reconnect.
        let mut transport_failed = false;
        context.outbound.drain_each(|packet| {
            if transport_failed {
                return;
            }
            match rns_serial_framing::encode(packet.bytes, &mut frame_buf) {
                Ok(n) => {
                    if port.write_all(&frame_buf[..n]).is_err() {
                        transport_failed = true;
                    }
                }
                // Oversize packet: drop it (self-heals — RNS re-announces).
                Err(_) => {}
            }
        });
        if transport_failed {
            return ConnectionEnd::Disconnected;
        }

        // Read a chunk and feed the decoder; each closed non-empty frame is a
        // Reticulum packet → submit it. `submit` stamps arrival and wakes the host.
        match port.read(&mut read_buf) {
            Ok(0) => {}
            Ok(n) => {
                for &byte in &read_buf[..n] {
                    if let Ok(Some(frame)) = decoder.feed(byte) {
                        if !frame.is_empty() {
                            let _ = context.inbound.submit(|buf| {
                                buf[..frame.len()].copy_from_slice(frame);
                                frame.len()
                            });
                        }
                    }
                }
            }
            // Idle read window (timeout) — loop back to service outbound.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            // Anything else is a real transport error (unplug) → reconnect.
            Err(_) => return ConnectionEnd::Disconnected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::mpsc::sync_channel;
    use std::sync::Mutex;
    use std::time::Instant;

    use crate::engine::OutboundPacket;
    use crate::interfaces::rns_serial_framing::{self, ESC, FLAG};
    use crate::interfaces::InterfaceHandle;
    use crate::runtime::channels::std_host::StdInterfaceSeam;

    /// In-memory byte pipe: serves preloaded `rx` bytes, then errors so a
    /// connection loop returns (a simulated unplug / end of stream); captures all
    /// writes into a handle the test keeps after the loop consumes the port.
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
                // Stream exhausted → error so the connection loop returns.
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

    fn seam() -> StdInterfaceSeam<SERIAL_MTU> {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        // The receiver is dropped here; `submit`/`report` use `try_send`, so a poke
        // to a gone receiver is a benign no-op for these single-connection tests.
        StdInterfaceSeam::<SERIAL_MTU>::new(test_id(), Instant::now(), 8, wake_tx)
    }

    #[test]
    fn deframes_inbound_bytes_and_submits_them_to_the_seam() {
        // Payload includes FLAG and ESC so the round-trip exercises unstuffing.
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let mut framed = [0u8; 32];
        let n = rns_serial_framing::encode(&payload, &mut framed).unwrap();

        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let (port, _written) = MockPort::new(framed[..n].to_vec());

        // One connection: serves the frame, then the next read errors and returns.
        let _ = serve_connection(port, &mut worker_context);

        let mut received: Vec<Vec<u8>> = Vec::new();
        let drained = runtime_handle.drain_inbound(|pkt| {
            assert_eq!(pkt.source_interface, test_id());
            received.push(pkt.bytes.to_vec());
        });
        assert_eq!(drained, 1);
        assert_eq!(received, std::vec![payload.to_vec()]);
    }

    #[test]
    fn frames_an_outbound_packet_onto_the_wire() {
        // Packet contains a FLAG so the framing must escape it.
        let packet = [0xAAu8, FLAG, 0xBB];

        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        // Queue it via the runtime handle; empty rx → the first read errors, but
        // the outbound drain runs first.
        assert!(runtime_handle.send(OutboundPacket::new(&packet)));
        let (port, written) = MockPort::new(Vec::new());

        let _ = serve_connection(port, &mut worker_context);

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
