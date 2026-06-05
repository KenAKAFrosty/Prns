use std::io::{self, Read, Write};
use std::time::Duration;

use super::super::core::{descriptor, SERIAL_MTU};
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::substrate::StdHostSubstrate;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceId,
    InterfaceWorkerContext, OutboundDrain, SelfDrivenInterface,
};

type SerialContext = InterfaceWorkerContext<StdHostSubstrate<SERIAL_MTU>>;

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

fn serve_until_stopped<Open, Port>(mut open: Open, reconnect: Duration, mut context: SerialContext)
where
    Open: FnMut() -> io::Result<Port>,
    Port: Read + Write,
{
    loop {
        if let Ok(port) = open() {
            match serve_connection(port, &mut context) {
                ConnectionEnd::Stopped => break,
                ConnectionEnd::Disconnected => {}
            }
        }
        if matches!(context.control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }
        std::thread::sleep(reconnect);
    }
    context.control.report(ControlReport::Stopped);
}

enum ConnectionEnd {
    Stopped,
    Disconnected,
}

/// `port` must have a short read timeout (the host's `open` sets it) so a quiet
/// link still loops back to service outbound and check for a stop. Pre-frame noise
/// — e.g. a board sharing this link between log text and frames — is skipped by the
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

        let mut transport_failed = false;
        context.outbound.drain_each(|packet| {
            if transport_failed {
                return;
            }
            if let Ok(n) = rns_serial_framing::encode(packet.bytes, &mut frame_buf) {
                if port.write_all(&frame_buf[..n]).is_err() {
                    transport_failed = true;
                }
            }
        });
        if transport_failed {
            return ConnectionEnd::Disconnected;
        }

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
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
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

    use crate::interfaces::rns_serial_framing::{self, ESC, FLAG};
    use crate::interfaces::substrate::StdInterfaceSeam;
    use crate::interfaces::InterfaceHandle;

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
        StdInterfaceSeam::<SERIAL_MTU>::new(test_id(), Instant::now(), 8, wake_tx)
    }

    #[test]
    fn deframes_inbound_bytes_and_submits_them_to_the_seam() {
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let mut framed = [0u8; 32];
        let n = rns_serial_framing::encode(&payload, &mut framed).unwrap();

        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let (port, _written) = MockPort::new(framed[..n].to_vec());

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
        let packet = [0xAAu8, FLAG, 0xBB];

        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        assert_eq!(
            runtime_handle.acquire_send_grant(|buf| {
                buf[..packet.len()].copy_from_slice(&packet);
                packet.len()
            }),
            Ok(packet.len())
        );
        let (port, written) = MockPort::new(Vec::new());

        let _ = serve_connection(port, &mut worker_context);

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
