//! The std USB-auto discoverer: it owns the host's plugged CDC devices behind the
//! one interface seam. It probes each on arrival, merging their inbound into the
//! seam, and fanning each outbound packet across every confirmed link.
//!

// The driver (below) is the Discoverer's only caller and exists only under
// `usb-auto`; without that feature the device logic compiles but goes unused.
#![cfg_attr(not(feature = "usb-auto"), allow(dead_code))]

use std::io::{self, Read, Write};
use std::string::String;
use std::vec::Vec;

use super::super::core::{
    decode_message, Message, NodeTag, MAX_DATA_BYTES, MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES,
};
use crate::interfaces::framing::rns_serial_framing::RnsSerialDecoder;
use crate::interfaces::substrate::StdHostSubstrate;
use crate::interfaces::{
    ConnectionState, ControlEndpoint, ControlReport, InboundSink, InterfaceWorkerContext,
    OutboundDrain,
};

const USB_AUTO_MTU: usize = MAX_DATA_BYTES;
type UsbAutoContext = InterfaceWorkerContext<StdHostSubstrate<USB_AUTO_MTU>>;

#[derive(Clone, PartialEq, Eq, Debug)]
struct PortId(String);

/// How many discovery scans a freshly probed port gets to answer before we give
/// up. At the driver's ~300 ms scan cadence that's roughly two seconds — long
/// enough for a booting board, short enough that a non-Personal device (someone
/// else's serial gadget) is released promptly.
const PROBE_SCAN_BUDGET: u8 = 7;

enum LinkState {
    /// Probed, awaiting a HelloAck; rejected once `scans_left` hits zero.
    Probing { scans_left: u8 },
    Confirmed(NodeTag),
    /// The port errored this pass and is pruned once servicing finishes.
    Lost,
}

struct Device<Port> {
    id: PortId,
    port: Port,
    decoder: RnsSerialDecoder<MAX_MESSAGE_BYTES>,
    state: LinkState,
}

struct Discoverer<Port> {
    devices: Vec<Device<Port>>,
    /// Ports that were probed but never answered: skipped until they drop out of a
    /// scan, so we neither re-probe a stranger nor keep its port open.
    rejected: Vec<PortId>,
    reported_state: ConnectionState,
}

impl<Port: Read + Write> Discoverer<Port> {
    fn new() -> Self {
        Self {
            devices: Vec::new(),
            rejected: Vec::new(),
            reported_state: ConnectionState::Degraded,
        }
    }

    /// A port is present. If new, open it, send a [`Hello`](Message::Hello), and
    /// track it as probing. Idempotent for a port already known.
    fn note_present(&mut self, id: PortId, open: impl FnOnce(&PortId) -> io::Result<Port>) {
        if self.devices.iter().any(|d| d.id == id) {
            return;
        }
        let Ok(mut port) = open(&id) else { return };
        let mut frame = [0u8; MAX_FRAMED_BYTES];
        let Ok(n) = Message::Hello.write_framed(&mut frame) else {
            return;
        };
        if port.write_all(&frame[..n]).is_ok() {
            self.devices.push(Device {
                id,
                port,
                decoder: RnsSerialDecoder::new(),
                state: LinkState::Probing {
                    scans_left: PROBE_SCAN_BUDGET,
                },
            });
        }
    }

    /// Reconcile tracked links against the ports present right now: spend probe
    /// budgets, reject the probes that ran out, drop links that vanished, and probe
    /// newly-appeared ports. `open` acquires a port for I/O.
    fn reconcile_present(
        &mut self,
        present: &[PortId],
        open: impl Fn(&PortId) -> io::Result<Port>,
    ) {
        // Spend one scan of each probing link's budget.
        for device in &mut self.devices {
            if let LinkState::Probing { scans_left } = &mut device.state {
                *scans_left = scans_left.saturating_sub(1);
            }
        }
        // Re-offer the handshake to every still-probing link each scan. Opening a
        // USB-CDC port can reset the board — the DTR/RTS toggle that drops an ESP32
        // into its download stub — dropping the Hello sent on open; re-sending until
        // the budget runs out rides over that reboot, and any otherwise-lost first
        // Hello, without widening the budget.
        let mut hello = [0u8; MAX_FRAMED_BYTES];
        if let Ok(n) = Message::Hello.write_framed(&mut hello) {
            for device in &mut self.devices {
                if matches!(device.state, LinkState::Probing { .. }) {
                    let _ = device.port.write_all(&hello[..n]);
                }
            }
        }
        // A probe that ran out is rejected: remember the id so we don't immediately
        // re-probe it, then drop the link below (releasing its port).
        for device in &self.devices {
            if matches!(device.state, LinkState::Probing { scans_left: 0 }) {
                self.rejected.push(device.id.clone());
            }
        }
        // Keep only present, un-rejected links; forget a rejection once its port is
        // gone, so a replug there is probed afresh.
        self.devices
            .retain(|d| present.contains(&d.id) && !self.rejected.contains(&d.id));
        self.rejected.retain(|id| present.contains(id));
        for id in present {
            if !self.rejected.contains(id) {
                self.note_present(id.clone(), &open);
            }
        }
    }

    fn pump(&mut self, ctx: &mut UsbAutoContext) {
        self.read_devices(&mut ctx.inbound);
        self.dedup_confirmed_links();
        self.fan_out(&mut ctx.outbound);
        self.devices
            .retain(|device| !matches!(device.state, LinkState::Lost));
        self.sync_connection_state(&mut ctx.control);
    }

    fn read_devices(&mut self, inbound: &mut impl InboundSink) {
        let mut buf = [0u8; 256];
        for device in &mut self.devices {
            match device.port.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    for &byte in &buf[..n] {
                        let confirmed_tag = match device.decoder.feed(byte) {
                            Ok(Some(frame)) if !frame.is_empty() => {
                                service_inbound_frame(frame, &device.state, inbound)
                            }
                            _ => None,
                        };
                        if let Some(tag) = confirmed_tag {
                            device.state = LinkState::Confirmed(tag);
                        }
                    }
                }
                Err(ref e) if would_block(e) => {}
                Err(_) => device.state = LinkState::Lost,
            }
        }
    }

    fn dedup_confirmed_links(&mut self) {
        let mut i = 0;
        while i < self.devices.len() {
            if let LinkState::Confirmed(tag) = self.devices[i].state {
                let superseded = self.devices[i + 1..]
                    .iter()
                    .any(|d| matches!(d.state, LinkState::Confirmed(newer) if newer == tag));
                if superseded {
                    self.devices[i].state = LinkState::Lost;
                }
            }
            i += 1;
        }
    }

    fn fan_out(&mut self, outbound: &mut impl OutboundDrain) {
        let devices = &mut self.devices;
        let mut frame = [0u8; MAX_FRAMED_BYTES];
        outbound.drain_each(|packet| {
            let Ok(n) = Message::Data(packet.bytes).write_framed(&mut frame) else {
                return;
            };
            for device in devices.iter_mut() {
                if matches!(device.state, LinkState::Confirmed(_))
                    && device.port.write_all(&frame[..n]).is_err()
                {
                    device.state = LinkState::Lost;
                }
            }
        });
    }

    fn sync_connection_state(&mut self, control: &mut impl ControlEndpoint) {
        let state = if self
            .devices
            .iter()
            .any(|d| matches!(d.state, LinkState::Confirmed(_)))
        {
            ConnectionState::Connected
        } else {
            // Up but peerless: still routable, just nothing to fan to yet.
            ConnectionState::Degraded
        };
        if state != self.reported_state {
            self.reported_state = state;
            control.report(ControlReport::ConnectionState(state));
        }
    }
}

fn service_inbound_frame(
    frame: &[u8],
    state: &LinkState,
    inbound: &mut impl InboundSink,
) -> Option<NodeTag> {
    match decode_message(frame) {
        Ok(Message::HelloAck(tag)) => Some(tag),
        Ok(Message::Data(packet)) => {
            if matches!(state, LinkState::Confirmed(_)) {
                let _ = inbound.submit(|slot| {
                    slot[..packet.len()].copy_from_slice(packet);
                    packet.len()
                });
            }
            None
        }
        _ => None,
    }
}

fn would_block(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

/// The production driver: polls serialport a few times a second for the USB CDC
/// ports present, reconciles them into the Discoverer's links, and runs the
/// servicing loop. Cross-platform — serialport cfg-gates its own per-OS backends;
/// opt-in via `usb-auto`.
#[cfg(feature = "usb-auto")]
mod driver {
    use std::io::{self, Read, Write};
    use std::thread;
    use std::time::{Duration, Instant};
    use std::vec::Vec;

    use super::super::super::core::host_descriptor;
    use super::{Discoverer, PortId, UsbAutoContext};
    use crate::interfaces::{
        ControlCommand, ControlEndpoint, ControlReport, InterfaceId, SelfDrivenInterface,
    };

    /// USB CDC ignores baud, but the serialport API still wants a number.
    const CDC_BAUD: u32 = 115_200;
    const READ_TIMEOUT: Duration = Duration::from_millis(5);
    const SERVICE_INTERVAL: Duration = Duration::from_millis(10);
    const SCAN_INTERVAL: Duration = Duration::from_millis(300);

    /// Build the plug-and-play USB-auto interface on `id`: a self-driven worker that
    /// discovers and owns the host's USB CDC links — no port argument, no config.
    pub fn usb_auto_interface(id: InterfaceId) -> SelfDrivenInterface<impl FnOnce(UsbAutoContext)> {
        SelfDrivenInterface::new(host_descriptor(id), move |ctx| {
            thread::spawn(move || serve(ctx));
        })
    }

    fn serve(mut ctx: UsbAutoContext) {
        let mut discoverer = Discoverer::new();
        let mut last_scan: Option<Instant> = None;
        loop {
            if last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
                last_scan = Some(Instant::now());
                discoverer.reconcile_present(&scan_cdc_ports(), open_cdc_port);
            }
            discoverer.pump(&mut ctx);
            if matches!(ctx.control.next_command(), Some(ControlCommand::Stop)) {
                break;
            }
            thread::sleep(SERVICE_INTERVAL);
        }
        ctx.control.report(ControlReport::Stopped);
    }

    /// Every USB serial port present right now. serialport classifies the medium
    /// per platform, so the USB filter excludes built-in / PCI serial without
    /// naming any OS-specific device path.
    fn scan_cdc_ports() -> Vec<PortId> {
        serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .filter(|info| matches!(info.port_type, serialport::SerialPortType::UsbPort(_)))
            .map(|info| PortId(info.port_name))
            .collect()
    }

    /// A serialport CDC link as a plain `Read + Write` stream for the Discoverer.
    struct CdcPort(Box<dyn serialport::SerialPort>);

    impl Read for CdcPort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for CdcPort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    fn open_cdc_port(id: &PortId) -> io::Result<CdcPort> {
        let mut port = serialport::new(id.0.as_str(), CDC_BAUD)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(io::Error::other)?;
        // An ESP32's native USB-serial-jtag maps the modem lines to its boot/reset
        // pins (RTS→EN, DTR→GPIO0), so a host asserting them pulses the board into
        // reset — or its download stub — mid-handshake. Hold both deasserted so the
        // board keeps running across an open. (A board behind a USB-UART bridge
        // ignores these, so it is harmless there.)
        let _ = port.write_data_terminal_ready(false);
        let _ = port.write_request_to_send(false);
        Ok(CdcPort(port))
    }
}

#[cfg(feature = "usb-auto")]
pub use driver::usb_auto_interface;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::substrate::StdInterfaceSeam;
    use crate::interfaces::{InterfaceHandle, InterfaceId, OutboundPacket};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[derive(Clone)]
    struct MockWire {
        inbox: Arc<Mutex<VecDeque<u8>>>,
        outbox: Arc<Mutex<Vec<u8>>>,
        broken: Arc<AtomicBool>,
    }

    impl MockWire {
        fn new() -> Self {
            Self {
                inbox: Arc::new(Mutex::new(VecDeque::new())),
                outbox: Arc::new(Mutex::new(Vec::new())),
                broken: Arc::new(AtomicBool::new(false)),
            }
        }
        fn device_sends(&self, msg: Message) {
            self.inbox.lock().unwrap().extend(framed(msg));
        }
        fn host_wrote(&self) -> Vec<u8> {
            self.outbox.lock().unwrap().clone()
        }
        fn unplug(&self) {
            self.broken.store(true, Ordering::SeqCst);
        }
        fn port(&self) -> MockPort {
            MockPort { wire: self.clone() }
        }
    }

    struct MockPort {
        wire: MockWire,
    }

    impl Read for MockPort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.wire.broken.load(Ordering::SeqCst) {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let mut q = self.wire.inbox.lock().unwrap();
            if q.is_empty() {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            let n = q.len().min(buf.len());
            for slot in buf.iter_mut().take(n) {
                *slot = q.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for MockPort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.wire.broken.load(Ordering::SeqCst) {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            self.wire.outbox.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn framed(msg: Message) -> Vec<u8> {
        let mut buf = [0u8; MAX_FRAMED_BYTES];
        let n = msg.write_framed(&mut buf).unwrap();
        buf[..n].to_vec()
    }

    /// Every `Data` packet (deframed + decoded) in a host-written byte stream.
    fn data_frames(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut decoder = RnsSerialDecoder::<MAX_MESSAGE_BYTES>::new();
        let mut out = Vec::new();
        for &b in bytes {
            if let Ok(Some(frame)) = decoder.feed(b) {
                if !frame.is_empty() {
                    if let Ok(Message::Data(p)) = decode_message(frame) {
                        out.push(p.to_vec());
                    }
                }
            }
        }
        out
    }

    fn first_frame_is(bytes: &[u8], pred: impl FnOnce(Message) -> bool) -> bool {
        let mut decoder = RnsSerialDecoder::<MAX_MESSAGE_BYTES>::new();
        for &b in bytes {
            if let Ok(Some(frame)) = decoder.feed(b) {
                if !frame.is_empty() {
                    return decode_message(frame).map(pred).unwrap_or(false);
                }
            }
        }
        false
    }

    fn seam() -> StdInterfaceSeam<USB_AUTO_MTU> {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        StdInterfaceSeam::<USB_AUTO_MTU>::new(
            InterfaceId::new([0xD1; 16]),
            Instant::now(),
            8,
            wake_tx,
        )
    }

    fn port(id: &str) -> PortId {
        PortId(id.into())
    }

    #[test]
    fn a_newly_present_port_is_probed_with_a_hello() {
        let mut disc = Discoverer::new();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        assert!(first_frame_is(&wire.host_wrote(), |m| matches!(
            m,
            Message::Hello
        )));
    }

    #[test]
    fn a_hello_ack_confirms_the_link_and_reports_connected() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = Discoverer::new();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck(NodeTag([7; 8])));
        disc.pump(&mut worker_context);

        match &disc.devices[0].state {
            LinkState::Confirmed(tag) => assert_eq!(*tag, NodeTag([7; 8])),
            LinkState::Probing { .. } | LinkState::Lost => {
                panic!("expected the link to be confirmed")
            }
        }
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::ConnectionState(ConnectionState::Connected))
        ));
    }

    #[test]
    fn data_from_a_confirmed_device_reaches_the_seam() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = Discoverer::new();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck(NodeTag([1; 8])));
        let packet = [0xDE, 0xAD, 0xBE, 0xEF];
        wire.device_sends(Message::Data(&packet));
        disc.pump(&mut worker_context);

        let mut got: Vec<Vec<u8>> = Vec::new();
        let drained = runtime_handle.drain_inbound(|pkt| got.push(pkt.bytes.to_vec()));
        assert_eq!(drained, 1);
        assert_eq!(got, std::vec![packet.to_vec()]);
    }

    #[test]
    fn data_before_the_handshake_completes_is_ignored() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = Discoverer::new();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        // Data arrives while still probing — no HelloAck yet.
        wire.device_sends(Message::Data(&[0x01, 0x02]));
        disc.pump(&mut worker_context);

        assert_eq!(runtime_handle.drain_inbound(|_| {}), 0);
    }

    #[test]
    fn outbound_fans_out_to_confirmed_links_only() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = Discoverer::new();

        let confirmed = MockWire::new();
        let probing = MockWire::new();
        let (c, q) = (confirmed.port(), probing.port());
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(c));
        disc.note_present(port("/dev/ttyACM1"), move |_| Ok(q));

        // Only the first answers the handshake.
        confirmed.device_sends(Message::HelloAck(NodeTag([2; 8])));
        disc.pump(&mut worker_context);

        let packet = [0x11, 0x22, 0x33];
        runtime_handle.send(OutboundPacket::new(&packet)).unwrap();
        disc.pump(&mut worker_context);

        assert_eq!(
            data_frames(&confirmed.host_wrote()),
            std::vec![packet.to_vec()]
        );
        assert!(data_frames(&probing.host_wrote()).is_empty());
    }

    #[test]
    fn the_same_node_on_two_ports_keeps_only_the_newest_link() {
        let StdInterfaceSeam {
            mut worker_context, ..
        } = seam();
        let mut disc = Discoverer::new();
        let old = MockWire::new();
        let new = MockWire::new();
        let (o, n) = (old.port(), new.port());
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(o));
        disc.note_present(port("/dev/ttyACM1"), move |_| Ok(n));

        // One node identity answers on both ports.
        old.device_sends(Message::HelloAck(NodeTag([9; 8])));
        new.device_sends(Message::HelloAck(NodeTag([9; 8])));
        disc.pump(&mut worker_context);

        assert_eq!(disc.devices.len(), 1);
        assert_eq!(disc.devices[0].id, port("/dev/ttyACM1"));
    }

    #[test]
    fn a_scan_probes_new_ports_and_drops_vanished_ones() {
        let mut disc = Discoverer::new();
        let wire = MockWire::new();

        // A USB port shows up in the scan → probed with a Hello.
        disc.reconcile_present(&[port("/dev/ttyACM0")], |_| Ok(wire.port()));
        assert_eq!(disc.devices.len(), 1);
        assert!(first_frame_is(&wire.host_wrote(), |m| matches!(
            m,
            Message::Hello
        )));

        // Same port still present next scan → idempotent, still one link.
        disc.reconcile_present(&[port("/dev/ttyACM0")], |_| Ok(wire.port()));
        assert_eq!(disc.devices.len(), 1);

        // Gone from the scan → dropped.
        disc.reconcile_present(&[], |_| Ok(wire.port()));
        assert!(disc.devices.is_empty());
    }

    #[test]
    fn a_port_that_never_answers_is_rejected_then_left_alone() {
        let mut disc = Discoverer::new();
        let wire = MockWire::new();
        let present = [port("/dev/ttyACM0")];

        // First scan probes it.
        disc.reconcile_present(&present, |_| Ok(wire.port()));
        assert_eq!(disc.devices.len(), 1);

        // It never answers; run scans until the probe budget is spent.
        for _ in 0..PROBE_SCAN_BUDGET {
            disc.reconcile_present(&present, |_| Ok(wire.port()));
        }
        assert!(disc.devices.is_empty(), "an unanswered probe is released");

        // Still plugged but rejected → left alone, not re-probed.
        disc.reconcile_present(&present, |_| Ok(wire.port()));
        assert!(disc.devices.is_empty());

        // Unplugged → rejection forgotten; back again → probed afresh.
        disc.reconcile_present(&[], |_| Ok(wire.port()));
        disc.reconcile_present(&present, |_| Ok(wire.port()));
        assert_eq!(disc.devices.len(), 1, "a replugged port is probed again");
    }

    #[test]
    fn an_unplugged_port_is_dropped_and_the_interface_goes_degraded() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = Discoverer::new();
        let wire = MockWire::new();
        disc.reconcile_present(&[port("/dev/ttyACM0")], |_| Ok(wire.port()));
        wire.device_sends(Message::HelloAck(NodeTag([3; 8])));
        disc.pump(&mut worker_context);
        let _ = runtime_handle.next_report(); // consume the Connected report

        // Unplugged: absent from the next scan.
        disc.reconcile_present(&[], |_| Ok(wire.port()));
        disc.pump(&mut worker_context);

        assert!(disc.devices.is_empty());
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::ConnectionState(ConnectionState::Degraded))
        ));
    }

    #[test]
    fn a_read_error_prunes_the_device() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = Discoverer::new();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));
        wire.device_sends(Message::HelloAck(NodeTag([4; 8])));
        disc.pump(&mut worker_context);
        let _ = runtime_handle.next_report();

        wire.unplug();
        disc.pump(&mut worker_context);

        assert!(disc.devices.is_empty());
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::ConnectionState(ConnectionState::Degraded))
        ));
    }
}
