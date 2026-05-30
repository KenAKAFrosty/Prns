//! Real USB-serial transport for the std daemon — the host end of a cable to
//! an ESP32-C6 (or any stock RNS serial peer).
//!
//! Incoming bytes are de-framed by the shared [`RnsSerialDecoder`]; outgoing
//! packets use the same canonical RNS serial frame layout as a stock
//! `SerialInterface` and are written back.
//! Pre-frame noise on the link — e.g. an ESP32-C6 sharing its USB Serial/JTAG
//! port between `println!` logs and Reticulum frames — is ignored by the
//! decoder until a `FLAG` opens a frame.
//!
//! `serialport` is wrapped behind [`SerialUsbInterface`] so the rest of the
//! daemon and the engine only ever see the [`Interface`] surface; the type is
//! generic over any [`Read`] + [`Write`] backend, so a later swap to raw
//! termios / a different backend (or a test mock) never touches callers.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use personal_rns::engine::{InboundPacket, InstantMillis};
use personal_rns::host::HostAdapter;
use personal_rns::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use personal_rns::interfaces::{
    Capabilities, Interface, InterfaceId, InterfaceMode, InterfaceState, MediumKind,
    PointToPointInterface,
};
use personal_rns::wire::MTU;

/// RNS serial framing worst case (every payload byte escaped) for an
/// MTU-sized packet.
const ENCODE_BUF_LEN: usize = rns_serial_framing::max_encoded_len(MTU);
const USB_CDC_NOMINAL_BAUD: u32 = 115_200;
const READ_IDLE_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum SerialUsbError {
    Io(io::Error),
    FrameLargerThanCallerBuffer,
    FrameLargerThanInterfaceBuffer,
    PayloadLargerThanMtu,
}

pub struct SerialUsbInterface<P: Read + Write> {
    id: InterfaceId,
    port: P,
    decoder: RnsSerialDecoder<MTU>,
}

impl SerialUsbInterface<Box<dyn serialport::SerialPort>> {
    /// Open `path` (e.g. `/dev/ttyACM0`) as a point-to-point interface with
    /// the given stable id.
    pub fn open(id: InterfaceId, path: &str) -> Result<Self, SerialUsbError> {
        let port = serialport::new(path, USB_CDC_NOMINAL_BAUD)
            .timeout(READ_IDLE_TIMEOUT)
            .open()
            .map_err(|e| SerialUsbError::Io(io::Error::other(e)))?;
        Ok(Self::from_io(id, port))
    }
}

impl<P: Read + Write> SerialUsbInterface<P> {
    /// Build the interface over any byte backend. The open-by-path path uses
    /// this with a real serial port; tests use it with an in-memory mock.
    pub fn from_io(id: InterfaceId, port: P) -> Self {
        Self {
            id,
            port,
            decoder: RnsSerialDecoder::new(),
        }
    }
}

impl<P: Read + Write> Interface for SerialUsbInterface<P> {
    type Error = SerialUsbError;

    fn id(&self) -> InterfaceId {
        self.id
    }

    fn capabilities(&self) -> Capabilities {
        // CDC-ACM to a single peer: full-duplex byte stream with no broadcast
        // or in-medium repeat semantics.
        Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: false,
        }
    }

    fn mode(&self) -> InterfaceMode {
        InterfaceMode::PointToPoint
    }

    fn medium_kind(&self) -> MediumKind {
        MediumKind::DirectPeer
    }

    fn state(&self) -> InterfaceState {
        // Plug/unplug lifecycle detection is a later slice; while the port is
        // open we report Connected.
        InterfaceState::Connected
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        // Drain bytes one at a time and feed the RNS serial decoder until a
        // frame closes, then copy it into the caller's buffer. A read timeout
        // (or a zero-length read at end-of-stream) means the link is idle →
        // Ok(None); the remaining bytes of any in-flight burst wait for the
        // next call.
        loop {
            let mut byte = [0u8; 1];
            match self.port.read(&mut byte) {
                Ok(0) => return Ok(None),
                Ok(_) => match self.decoder.feed(byte[0]) {
                    Ok(None) => continue,
                    Ok(Some(frame)) => {
                        if frame.is_empty() {
                            continue;
                        }
                        if frame.len() > buf.len() {
                            return Err(SerialUsbError::FrameLargerThanCallerBuffer);
                        }
                        let n = frame.len();
                        buf[..n].copy_from_slice(frame);
                        return Ok(Some(n));
                    }
                    Err(rns_serial_framing::DecodeError::FrameTooBig) => {
                        return Err(SerialUsbError::FrameLargerThanInterfaceBuffer);
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::TimedOut => return Ok(None),
                Err(e) => return Err(SerialUsbError::Io(e)),
            }
        }
    }

    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error> {
        let mut framed = [0u8; ENCODE_BUF_LEN];
        let n = rns_serial_framing::encode(packet, &mut framed)
            .map_err(|_| SerialUsbError::PayloadLargerThanMtu)?;
        self.port
            .write_all(&framed[..n])
            .map_err(SerialUsbError::Io)
    }
}

impl<P: Read + Write> PointToPointInterface for SerialUsbInterface<P> {}

/// Per-step host view for the USB receive/transmit loop: the monotonic clock
/// and OS CSPRNG the engine needs, the inbound batch the caller already
/// de-framed this step, and the interface to transmit egress on.
///
/// Built fresh each `step` so the borrowed inbound batch can reference the
/// caller's per-step decode scratch — the engine seam lends inbound as a
/// borrowed slice (see [`HostAdapter::drain_inbound_packets`]), which a
/// long-lived owned host can't supply without a self-referential struct. The
/// interface is borrowed for egress only; the caller reads inbound (releasing
/// the interface) before building this view, so the two never alias.
pub struct UsbHost<'a, P: Read + Write> {
    clock: Instant,
    iface: &'a mut SerialUsbInterface<P>,
    inbound: &'a [InboundPacket<'a>],
}

/// The only way this host can fail a step: the OS RNG refusing entropy.
/// Surfaced honestly so crypto callers never see silent zeros. Egress write
/// failures are logged and swallowed per the [`HostAdapter`] contract, so they
/// are not an error variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbHostError {
    EntropySourceUnavailable,
}

impl<'a, P: Read + Write> UsbHost<'a, P> {
    pub fn for_step(
        clock: Instant,
        iface: &'a mut SerialUsbInterface<P>,
        inbound: &'a [InboundPacket<'a>],
    ) -> Self {
        Self {
            clock,
            iface,
            inbound,
        }
    }
}

impl<P: Read + Write> HostAdapter for UsbHost<'_, P> {
    type Error = UsbHostError;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(self.clock.elapsed().as_millis() as u64))
    }

    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        // OS CSPRNG — same source RNS uses (`os.urandom`).
        getrandom::getrandom(buf).map_err(|_| UsbHostError::EntropySourceUnavailable)
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(self.inbound)
    }

    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error> {
        // Faithful-pump contract (same as the C6 host): write to each id in
        // fire_on, log-and-swallow a per-interface write failure so one bad
        // directive can't halt the engine step.
        for id in fire_on {
            if *id == self.iface.id() {
                if let Err(e) = self.iface.write(bytes) {
                    eprintln!("RNSD_USB_EGRESS_ERR {e:?}");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// In-memory byte backend: reads from a preloaded stream, captures writes.
    struct MockPort {
        rx: Cursor<Vec<u8>>,
        tx: Vec<u8>,
    }

    impl Read for MockPort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.rx.read(buf)
        }
    }

    impl Write for MockPort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.tx.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn mock(rx: Vec<u8>) -> SerialUsbInterface<MockPort> {
        SerialUsbInterface::from_io(
            InterfaceId::new([0xD0; 16]),
            MockPort {
                rx: Cursor::new(rx),
                tx: Vec::new(),
            },
        )
    }

    #[test]
    fn try_read_deframes_one_rns_serial_frame_then_reports_idle() {
        // Payload deliberately contains FLAG and ESC bytes so the round-trip
        // exercises the decoder's byte-unstuffing.
        let payload = [
            0x01u8,
            0x02,
            rns_serial_framing::FLAG,
            rns_serial_framing::ESC,
            0x03,
        ];
        let mut framed = [0u8; 32];
        let n = rns_serial_framing::encode(&payload, &mut framed).unwrap();
        let mut iface = mock(framed[..n].to_vec());

        let mut buf = [0u8; MTU];
        assert_eq!(iface.try_read(&mut buf).unwrap(), Some(payload.len()));
        assert_eq!(&buf[..payload.len()], &payload);
        // Stream exhausted (Cursor returns Ok(0)) → idle.
        assert_eq!(iface.try_read(&mut buf).unwrap(), None);
    }

    #[test]
    fn try_read_ignores_pre_frame_noise() {
        // A C6 shares its USB port between text logs and RNS serial frames;
        // the decoder must skip the non-FLAG prefix and still surface the frame.
        let payload = [0xAAu8, 0xBB, 0xCC];
        let mut framed = [0u8; 16];
        let n = rns_serial_framing::encode(&payload, &mut framed).unwrap();
        let mut rx = b"ESP32C6_HOST: boot\r\n".to_vec();
        rx.extend_from_slice(&framed[..n]);
        let mut iface = mock(rx);

        let mut buf = [0u8; MTU];
        assert_eq!(iface.try_read(&mut buf).unwrap(), Some(payload.len()));
        assert_eq!(&buf[..payload.len()], &payload);
    }

    #[test]
    fn write_delimits_the_packet_with_flags() {
        let mut iface = mock(Vec::new());
        iface
            .write(&[0xAA, rns_serial_framing::FLAG, 0xBB])
            .unwrap();
        let tx = &iface.port.tx;
        assert_eq!(tx.first(), Some(&rns_serial_framing::FLAG));
        assert_eq!(tx.last(), Some(&rns_serial_framing::FLAG));
        // Interior FLAG must have been escaped, so the only two raw FLAGs are
        // the opening and closing delimiters.
        assert_eq!(
            tx.iter()
                .filter(|&&b| b == rns_serial_framing::FLAG)
                .count(),
            2
        );
    }
}
