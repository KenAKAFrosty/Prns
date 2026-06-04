//! The std USB-auto discoverer: it owns the host's plugged CDC devices behind the
//! one interface seam. It probes each on arrival, merging their inbound into the
//! seam, and fanning each outbound packet across every confirmed link.
//!

// The serialport driver is the Discoverer's only non-test caller and exists only
// under `usb-auto`; without that feature the device logic compiles but goes
// unused.
#![cfg_attr(not(feature = "usb-auto"), allow(dead_code))]

use std::io::{self, Read, Write};
use std::string::String;
use std::vec::Vec;

use super::core::{
    decode_message, Message, NodeTag, MAX_DATA_BYTES, MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES,
};
use crate::interfaces::framing::rns_serial_framing::RnsSerialDecoder;
use crate::interfaces::substrate::StdHostSubstrate;
use crate::interfaces::{
    ConnectionState, ControlEndpoint, ControlReport, InboundSink, InterfaceWorkerContext,
    OutboundDrain,
};

pub(in crate::interfaces::impls::usb_auto) const USB_AUTO_MTU: usize = MAX_DATA_BYTES;
pub(in crate::interfaces::impls::usb_auto) type UsbAutoContext =
    InterfaceWorkerContext<StdHostSubstrate<USB_AUTO_MTU>>;

#[derive(Clone, PartialEq, Eq, Debug)]
pub(in crate::interfaces::impls::usb_auto) struct PortId(String);

impl PortId {
    pub(in crate::interfaces::impls::usb_auto) fn new(name: String) -> Self {
        Self(name)
    }

    pub(in crate::interfaces::impls::usb_auto) fn as_str(&self) -> &str {
        &self.0
    }
}

/// How many discovery scans a freshly probed port gets to answer before we give
/// up. At the driver's ~300 ms scan cadence that's roughly two seconds — long
/// enough for a booting board, short enough that a non-Personal device (someone
/// else's serial gadget) is released promptly.
const PROBE_SCAN_BUDGET: u8 = 7;

enum LinkState {
    Probing { scans_left: u8 },
    Confirmed(NodeTag),
    Lost,
}

struct Device<Port> {
    id: PortId,
    port: Port,
    decoder: RnsSerialDecoder<MAX_MESSAGE_BYTES>,
    state: LinkState,
}

pub(in crate::interfaces::impls::usb_auto) struct Discoverer<Port> {
    devices: Vec<Device<Port>>,
    rejected: Vec<PortId>,
    reported_state: ConnectionState,
}

impl<Port: Read + Write> Discoverer<Port> {
    pub(in crate::interfaces::impls::usb_auto) fn new() -> Self {
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
    pub(in crate::interfaces::impls::usb_auto) fn reconcile_present(
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
        // Re-offer the handshake to every still-probing link each scan. The Hello
        // sent on open can be missed (a board still booting when first probed, or
        // one rebooted by something other than us) so re-sending until the budget
        // runs out rides over that, without widening the budget. (`open_cdc_port`
        // keeps our own open from resetting the board in the first place.)
        let mut hello = [0u8; MAX_FRAMED_BYTES];
        if let Ok(n) = Message::Hello.write_framed(&mut hello) {
            for device in &mut self.devices {
                if matches!(device.state, LinkState::Probing { .. }) {
                    let _ = device.port.write_all(&hello[..n]);
                }
            }
        }

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

    pub(in crate::interfaces::impls::usb_auto) fn pump(&mut self, ctx: &mut UsbAutoContext) {
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
        PortId::new(id.into())
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
