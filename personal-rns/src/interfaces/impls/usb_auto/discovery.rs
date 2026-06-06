#![cfg_attr(not(feature = "usb-auto"), allow(dead_code))]

use std::io::{self, Read, Write};
use std::string::String;
use std::vec::Vec;

use super::core::{
    decode_message, Capabilities, Message, NodeTag, PeerProfile, MAX_DATA_BYTES, MAX_FRAMED_BYTES,
    MAX_MESSAGE_BYTES, READ_CHUNK_BYTES,
};
use crate::interfaces::framing::rns_serial_framing::RnsSerialDecoder;
use crate::interfaces::substrate::{StdHostSubstrate, StdOutboundDrain};
use crate::interfaces::{
    ConnectionState, ControlEndpoint, ControlReport, InboundSink, InterfaceWorkerContext,
};

pub(in crate::interfaces::impls::usb_auto) const USB_AUTO_MTU: usize = MAX_DATA_BYTES;
pub(in crate::interfaces::impls::usb_auto) type UsbAutoContext =
    InterfaceWorkerContext<StdHostSubstrate<USB_AUTO_MTU>>;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
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

const MAX_READS_PER_DEVICE_PER_PUMP: usize = 8;
const MAX_READS_PER_HOST_PEER_PER_PUMP: usize = 32;

enum LinkState {
    Probing { scans_left: u8 },
    Confirmed { tag: NodeTag, profile: PeerProfile },
    Lost,
}

impl LinkState {
    fn read_budget(&self) -> usize {
        match self {
            LinkState::Confirmed {
                profile: PeerProfile::Host,
                ..
            } => MAX_READS_PER_HOST_PEER_PER_PUMP,
            _ => MAX_READS_PER_DEVICE_PER_PUMP,
        }
    }
}

struct PendingWrite {
    buf: [u8; MAX_FRAMED_BYTES],
    len: usize,
    off: usize,
}

impl Default for PendingWrite {
    fn default() -> Self {
        Self {
            buf: [0u8; MAX_FRAMED_BYTES],
            len: 0,
            off: 0,
        }
    }
}

impl PendingWrite {
    fn is_empty(&self) -> bool {
        self.off >= self.len
    }

    fn set(&mut self, frame: &[u8]) {
        self.buf[..frame.len()].copy_from_slice(frame);
        self.len = frame.len();
        self.off = 0;
    }

    fn remaining(&self) -> &[u8] {
        &self.buf[self.off..self.len]
    }

    fn advance(&mut self, written: usize) {
        self.off += written;
    }
}

struct Device<Port> {
    id: PortId,
    port: Port,
    decoder: RnsSerialDecoder<MAX_MESSAGE_BYTES>,
    state: LinkState,
    pending: PendingWrite,
}

impl<Port: Write> Device<Port> {
    fn ingest_bytes(
        &mut self,
        bytes: &[u8],
        own_tag: NodeTag,
        own_capabilities: Capabilities,
        inbound: &mut impl InboundSink,
    ) {
        let mut owes_ack = false;
        let state = &mut self.state;
        self.decoder.feed_slice(bytes, |frame| {
            if !frame.is_empty() {
                service_inbound_frame(
                    frame,
                    state,
                    own_tag,
                    own_capabilities,
                    &mut owes_ack,
                    inbound,
                );
            }
        });
        if owes_ack {
            self.answer_hello(own_tag, own_capabilities);
        }
    }

    fn answer_hello(&mut self, tag: NodeTag, capabilities: Capabilities) {
        let mut frame = [0u8; MAX_FRAMED_BYTES];
        if let Ok(n) = (Message::HelloAck { tag, capabilities }).write_framed(&mut frame) {
            let _ = self.port.write_all(&frame[..n]);
        }
    }

    fn flush_pending(&mut self) {
        if !matches!(self.state, LinkState::Confirmed { .. }) {
            return;
        }
        while !self.pending.is_empty() {
            match self.port.write(self.pending.remaining()) {
                Ok(0) => break,
                Ok(written) => self.pending.advance(written),
                Err(ref e) if is_transient(e) => break,
                Err(_) => {
                    self.state = LinkState::Lost;
                    return;
                }
            }
        }
    }

    fn offer_frame(&mut self, frame: &[u8]) {
        if !matches!(self.state, LinkState::Confirmed { .. }) || !self.pending.is_empty() {
            return;
        }
        match self.port.write(frame) {
            Ok(written) if written >= frame.len() => {}
            Ok(written) => self.pending.set(&frame[written..]),
            Err(ref e) if is_transient(e) => self.pending.set(frame),
            Err(_) => self.state = LinkState::Lost,
        }
    }
}

pub(in crate::interfaces::impls::usb_auto) enum PumpCadence {
    Busy,
    Idle,
}

pub(in crate::interfaces::impls::usb_auto) struct Discoverer<Port> {
    node_tag: NodeTag,
    capabilities: Capabilities,
    devices: Vec<Device<Port>>,
    rejected: Vec<PortId>,
    reported_state: ConnectionState,
}

impl<Port> Discoverer<Port> {
    pub(in crate::interfaces::impls::usb_auto) fn ports_mut(
        &mut self,
    ) -> impl Iterator<Item = (&PortId, &mut Port)> + '_ {
        self.devices
            .iter_mut()
            .map(|device| (&device.id, &mut device.port))
    }
}

impl<Port: Read + Write> Discoverer<Port> {
    pub(in crate::interfaces::impls::usb_auto) fn new(
        node_tag: NodeTag,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            node_tag,
            capabilities,
            devices: Vec::new(),
            rejected: Vec::new(),
            reported_state: ConnectionState::Degraded,
        }
    }

    fn note_present(&mut self, id: PortId, open: impl FnOnce(&PortId) -> io::Result<Port>) {
        if self.devices.iter().any(|d| d.id == id) {
            return;
        }
        let Ok(mut port) = open(&id) else { return };
        let mut frame = [0u8; MAX_FRAMED_BYTES];
        let Ok(n) = Message::Hello(self.capabilities).write_framed(&mut frame) else {
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
                pending: PendingWrite::default(),
            });
        }
    }

    pub(in crate::interfaces::impls::usb_auto) fn reconcile_present(
        &mut self,
        present: &[PortId],
        open: impl Fn(&PortId) -> io::Result<Port>,
    ) {
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
        if let Ok(n) = Message::Hello(self.capabilities).write_framed(&mut hello) {
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
        self.devices
            .retain(|d| present.contains(&d.id) && !self.rejected.contains(&d.id));
        self.rejected.retain(|id| present.contains(id));
        for id in present {
            if !self.rejected.contains(id) {
                self.note_present(id.clone(), &open);
            }
        }
    }

    pub(in crate::interfaces::impls::usb_auto) fn pump(
        &mut self,
        ctx: &mut UsbAutoContext,
    ) -> PumpCadence {
        let saturated = self.read_devices(&mut ctx.inbound);
        self.dedup_confirmed_links();
        self.fan_out(&mut ctx.outbound);
        self.devices
            .retain(|device| !matches!(device.state, LinkState::Lost));
        self.sync_connection_state(&mut ctx.control);
        if saturated {
            PumpCadence::Busy
        } else {
            PumpCadence::Idle
        }
    }

    fn read_devices(&mut self, inbound: &mut impl InboundSink) -> bool {
        let (own_tag, own_capabilities) = (self.node_tag, self.capabilities);
        let mut buf = [0u8; READ_CHUNK_BYTES];
        let mut any_saturated = false;
        for device in &mut self.devices {
            let mut drained = false;
            for _ in 0..device.state.read_budget() {
                match device.port.read(&mut buf) {
                    Ok(0) => {
                        drained = true;
                        break;
                    }
                    Ok(n) => {
                        device.ingest_bytes(&buf[..n], own_tag, own_capabilities, inbound);
                        if n < buf.len() {
                            drained = true;
                            break;
                        }
                    }
                    Err(ref e) if is_transient(e) => {
                        drained = true;
                        break;
                    }
                    Err(_) => {
                        device.state = LinkState::Lost;
                        drained = true;
                        break;
                    }
                }
            }
            if !drained {
                any_saturated = true;
            }
        }
        any_saturated
    }

    fn dedup_confirmed_links(&mut self) {
        let mut i = 0;
        while i < self.devices.len() {
            if let LinkState::Confirmed { tag, .. } = self.devices[i].state {
                let superseded = self.devices[i + 1..].iter().any(
                    |d| matches!(d.state, LinkState::Confirmed { tag: newer, .. } if newer == tag),
                );
                if superseded {
                    self.devices[i].state = LinkState::Lost;
                }
            }
            i += 1;
        }
    }

    fn fan_out(&mut self, outbound: &mut StdOutboundDrain<USB_AUTO_MTU>) {
        for device in &mut self.devices {
            device.flush_pending();
        }
        let mut frame = [0u8; MAX_FRAMED_BYTES];
        loop {
            let blocked = self.devices.iter().any(|device| {
                matches!(device.state, LinkState::Confirmed { .. }) && !device.pending.is_empty()
            });
            if blocked {
                break;
            }
            let devices = &mut self.devices;
            let pulled = outbound.drain_one(|packet| {
                let Ok(n) = Message::Data(packet.bytes).write_framed(&mut frame) else {
                    return;
                };
                for device in devices.iter_mut() {
                    device.offer_frame(&frame[..n]);
                }
            });
            if !pulled {
                break;
            }
        }
    }

    pub(in crate::interfaces::impls::usb_auto) fn has_pending_writes(&self) -> bool {
        self.devices.iter().any(|device| !device.pending.is_empty())
    }

    fn sync_connection_state(&mut self, control: &mut impl ControlEndpoint) {
        let state = if self
            .devices
            .iter()
            .any(|d| matches!(d.state, LinkState::Confirmed { .. }))
        {
            ConnectionState::Connected
        } else {
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
    state: &mut LinkState,
    own_tag: NodeTag,
    own_capabilities: Capabilities,
    owes_ack: &mut bool,
    inbound: &mut impl InboundSink,
) {
    match decode_message(frame) {
        Ok(Message::Hello(_)) => *owes_ack = true,
        Ok(Message::HelloAck { tag, capabilities }) if tag != own_tag => {
            *state = LinkState::Confirmed {
                tag,
                profile: PeerProfile::negotiate(own_capabilities, capabilities),
            };
        }
        Ok(Message::Data(packet)) => {
            if matches!(*state, LinkState::Confirmed { .. }) {
                let _ = inbound.submit(|slot| {
                    slot[..packet.len()].copy_from_slice(packet);
                    packet.len()
                });
            }
        }
        _ => {}
    }
}

fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::substrate::StdInterfaceSeam;
    use crate::interfaces::{InterfaceHandle, InterfaceId};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    const HOST_TAG: NodeTag = NodeTag([0xAA; 8]);

    fn host() -> Discoverer<MockPort> {
        Discoverer::new(HOST_TAG, Capabilities::host())
    }

    #[derive(Clone)]
    struct MockWire {
        inbox: Arc<Mutex<VecDeque<u8>>>,
        outbox: Arc<Mutex<Vec<u8>>>,
        broken: Arc<AtomicBool>,
        write_fault: Arc<Mutex<Option<io::ErrorKind>>>,
        tx_budget: Arc<Mutex<Option<usize>>>,
    }

    impl MockWire {
        fn new() -> Self {
            Self {
                inbox: Arc::new(Mutex::new(VecDeque::new())),
                outbox: Arc::new(Mutex::new(Vec::new())),
                broken: Arc::new(AtomicBool::new(false)),
                write_fault: Arc::new(Mutex::new(None)),
                tx_budget: Arc::new(Mutex::new(None)),
            }
        }
        fn limit_tx(&self, bytes: usize) {
            *self.tx_budget.lock().unwrap() = Some(bytes);
        }
        fn refill_tx(&self) {
            *self.tx_budget.lock().unwrap() = None;
        }
        fn device_sends(&self, msg: Message) {
            self.inbox.lock().unwrap().extend(framed(msg));
        }
        fn device_floods_raw(&self, len: usize) {
            self.inbox
                .lock()
                .unwrap()
                .extend(std::iter::repeat_n(0u8, len));
        }
        fn host_wrote(&self) -> Vec<u8> {
            self.outbox.lock().unwrap().clone()
        }
        fn unplug(&self) {
            self.broken.store(true, Ordering::SeqCst);
        }
        fn set_write_fault(&self, kind: io::ErrorKind) {
            *self.write_fault.lock().unwrap() = Some(kind);
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
            let write_fault = *self.wire.write_fault.lock().unwrap();
            if let Some(kind) = write_fault {
                return Err(io::Error::from(kind));
            }
            let accepted = {
                let mut budget = self.wire.tx_budget.lock().unwrap();
                match budget.as_mut() {
                    Some(0) if !buf.is_empty() => {
                        return Err(io::Error::from(io::ErrorKind::WouldBlock));
                    }
                    Some(remaining) => {
                        let n = buf.len().min(*remaining);
                        *remaining -= n;
                        n
                    }
                    None => buf.len(),
                }
            };
            self.wire
                .outbox
                .lock()
                .unwrap()
                .extend_from_slice(&buf[..accepted]);
            Ok(accepted)
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

    fn any_frame_is(bytes: &[u8], pred: impl Fn(&Message) -> bool) -> bool {
        let mut decoder = RnsSerialDecoder::<MAX_MESSAGE_BYTES>::new();
        let mut found = false;
        for &b in bytes {
            if let Ok(Some(frame)) = decoder.feed(b) {
                if !frame.is_empty() {
                    if let Ok(message) = decode_message(frame) {
                        found |= pred(&message);
                    }
                }
            }
        }
        found
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
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        assert!(first_frame_is(&wire.host_wrote(), |m| matches!(
            m,
            Message::Hello(_)
        )));
    }

    #[test]
    fn a_hello_ack_confirms_the_link_and_reports_connected() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck {
            tag: NodeTag([7; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);

        match &disc.devices[0].state {
            LinkState::Confirmed { tag, .. } => assert_eq!(*tag, NodeTag([7; 8])),
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
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck {
            tag: NodeTag([1; 8]),
            capabilities: Capabilities::none(),
        });
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
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

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
        let mut disc = host();

        let confirmed = MockWire::new();
        let probing = MockWire::new();
        let (c, q) = (confirmed.port(), probing.port());
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(c));
        disc.note_present(port("/dev/ttyACM1"), move |_| Ok(q));

        confirmed.device_sends(Message::HelloAck {
            tag: NodeTag([2; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);

        let packet = [0x11, 0x22, 0x33];
        runtime_handle
            .acquire_send_grant(|buf| {
                buf[..packet.len()].copy_from_slice(&packet);
                packet.len()
            })
            .unwrap();
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
        let mut disc = host();
        let old = MockWire::new();
        let new = MockWire::new();
        let (o, n) = (old.port(), new.port());
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(o));
        disc.note_present(port("/dev/ttyACM1"), move |_| Ok(n));

        old.device_sends(Message::HelloAck {
            tag: NodeTag([9; 8]),
            capabilities: Capabilities::none(),
        });
        new.device_sends(Message::HelloAck {
            tag: NodeTag([9; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);

        assert_eq!(disc.devices.len(), 1);
        assert_eq!(disc.devices[0].id, port("/dev/ttyACM1"));
    }

    #[test]
    fn a_scan_probes_new_ports_and_drops_vanished_ones() {
        let mut disc = host();
        let wire = MockWire::new();

        disc.reconcile_present(&[port("/dev/ttyACM0")], |_| Ok(wire.port()));
        assert_eq!(disc.devices.len(), 1);
        assert!(first_frame_is(&wire.host_wrote(), |m| matches!(
            m,
            Message::Hello(_)
        )));

        disc.reconcile_present(&[port("/dev/ttyACM0")], |_| Ok(wire.port()));
        assert_eq!(disc.devices.len(), 1);

        disc.reconcile_present(&[], |_| Ok(wire.port()));
        assert!(disc.devices.is_empty());
    }

    #[test]
    fn a_port_that_never_answers_is_rejected_then_left_alone() {
        let mut disc = host();
        let wire = MockWire::new();
        let present = [port("/dev/ttyACM0")];

        disc.reconcile_present(&present, |_| Ok(wire.port()));
        assert_eq!(disc.devices.len(), 1);

        for _ in 0..PROBE_SCAN_BUDGET {
            disc.reconcile_present(&present, |_| Ok(wire.port()));
        }
        assert!(disc.devices.is_empty(), "an unanswered probe is released");

        disc.reconcile_present(&present, |_| Ok(wire.port()));
        assert!(disc.devices.is_empty());

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
        let mut disc = host();
        let wire = MockWire::new();
        disc.reconcile_present(&[port("/dev/ttyACM0")], |_| Ok(wire.port()));
        wire.device_sends(Message::HelloAck {
            tag: NodeTag([3; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);
        let _ = runtime_handle.next_report();

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
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));
        wire.device_sends(Message::HelloAck {
            tag: NodeTag([4; 8]),
            capabilities: Capabilities::none(),
        });
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

    #[test]
    fn a_confirmed_link_survives_a_transient_write_timeout() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));
        wire.device_sends(Message::HelloAck {
            tag: NodeTag([5; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::ConnectionState(ConnectionState::Connected))
        ));

        wire.set_write_fault(io::ErrorKind::TimedOut);
        let packet = [0xAB; 4];
        runtime_handle
            .acquire_send_grant(|buf| {
                buf[..packet.len()].copy_from_slice(&packet);
                packet.len()
            })
            .unwrap();
        disc.pump(&mut worker_context);

        assert!(
            matches!(disc.devices[0].state, LinkState::Confirmed { .. }),
            "a transient write timeout must not drop a healthy link"
        );
        assert!(
            runtime_handle.next_report().is_none(),
            "a transient write timeout reports no state change"
        );
    }

    #[test]
    fn a_hard_write_error_prunes_the_device() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));
        wire.device_sends(Message::HelloAck {
            tag: NodeTag([6; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);
        let _ = runtime_handle.next_report();

        wire.set_write_fault(io::ErrorKind::NotConnected);
        let packet = [0x01, 0x02, 0x03];
        runtime_handle
            .acquire_send_grant(|buf| {
                buf[..packet.len()].copy_from_slice(&packet);
                packet.len()
            })
            .unwrap();
        disc.pump(&mut worker_context);

        assert!(disc.devices.is_empty());
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::ConnectionState(ConnectionState::Degraded))
        ));
    }

    #[test]
    fn a_saturated_port_asks_for_an_immediate_repump() {
        let StdInterfaceSeam {
            mut worker_context, ..
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_floods_raw(READ_CHUNK_BYTES * MAX_READS_PER_DEVICE_PER_PUMP + 1);
        assert!(matches!(disc.pump(&mut worker_context), PumpCadence::Busy));
    }

    #[test]
    fn a_drained_port_lets_the_worker_idle() {
        let StdInterfaceSeam {
            mut worker_context, ..
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck {
            tag: NodeTag([7; 8]),
            capabilities: Capabilities::none(),
        });
        wire.device_sends(Message::Data(&[0x01, 0x02, 0x03]));
        assert!(matches!(disc.pump(&mut worker_context), PumpCadence::Idle));
    }

    #[test]
    fn a_burst_larger_than_one_read_is_drained_in_a_single_pump() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck {
            tag: NodeTag([8; 8]),
            capabilities: Capabilities::none(),
        });
        let packets: Vec<[u8; 100]> = (0..5).map(|i| [i as u8; 100]).collect();
        for packet in &packets {
            wire.device_sends(Message::Data(packet));
        }
        disc.pump(&mut worker_context);

        let mut got: Vec<Vec<u8>> = Vec::new();
        runtime_handle.drain_inbound(|pkt| got.push(pkt.bytes.to_vec()));
        assert_eq!(got.len(), packets.len());
        for packet in &packets {
            assert!(got.iter().any(|g| g.as_slice() == packet.as_slice()));
        }
    }

    #[test]
    fn a_full_tx_buffer_holds_the_frame_until_a_later_pump_flushes_it() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));
        wire.device_sends(Message::HelloAck {
            tag: NodeTag([5; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);
        let _ = runtime_handle.next_report();

        wire.limit_tx(0);
        let packet = [0xCC; 8];
        runtime_handle
            .acquire_send_grant(|buf| {
                buf[..packet.len()].copy_from_slice(&packet);
                packet.len()
            })
            .unwrap();
        disc.pump(&mut worker_context);
        assert!(
            matches!(disc.devices[0].state, LinkState::Confirmed { .. }),
            "a full tx buffer must not drop the link"
        );
        assert!(
            data_frames(&wire.host_wrote()).is_empty(),
            "nothing reaches the wire while the tx buffer is full"
        );

        wire.refill_tx();
        disc.pump(&mut worker_context);
        assert_eq!(
            data_frames(&wire.host_wrote()),
            std::vec![packet.to_vec()],
            "the buffered frame flushes once the tx buffer drains"
        );
    }

    #[test]
    fn a_partial_write_buffers_the_remainder_and_completes_it_later() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));
        wire.device_sends(Message::HelloAck {
            tag: NodeTag([6; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);
        let _ = runtime_handle.next_report();

        wire.limit_tx(3);
        let packet = [0x11, 0x22, 0x33, 0x44, 0x55];
        runtime_handle
            .acquire_send_grant(|buf| {
                buf[..packet.len()].copy_from_slice(&packet);
                packet.len()
            })
            .unwrap();
        disc.pump(&mut worker_context);
        assert!(matches!(disc.devices[0].state, LinkState::Confirmed { .. }));
        assert!(
            data_frames(&wire.host_wrote()).is_empty(),
            "a partial frame is not yet a decodable frame"
        );

        wire.refill_tx();
        disc.pump(&mut worker_context);
        assert_eq!(data_frames(&wire.host_wrote()), std::vec![packet.to_vec()]);
    }

    #[test]
    fn a_blocked_link_back_pressures_the_ring_without_dropping() {
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));
        wire.device_sends(Message::HelloAck {
            tag: NodeTag([7; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);
        let _ = runtime_handle.next_report();

        wire.limit_tx(0);
        let packets: [[u8; 4]; 5] = [[1; 4], [2; 4], [3; 4], [4; 4], [5; 4]];
        for packet in &packets {
            runtime_handle
                .acquire_send_grant(|buf| {
                    buf[..packet.len()].copy_from_slice(packet);
                    packet.len()
                })
                .unwrap();
        }
        disc.pump(&mut worker_context);
        assert!(
            data_frames(&wire.host_wrote()).is_empty(),
            "a blocked link writes nothing while its tx buffer is full"
        );

        wire.refill_tx();
        for _ in 0..packets.len() {
            disc.pump(&mut worker_context);
        }
        let got = data_frames(&wire.host_wrote());
        assert_eq!(
            got.len(),
            packets.len(),
            "every queued frame is delivered once tx drains — the ring back-pressures, it never drops"
        );
        for packet in &packets {
            assert!(got.iter().any(|g| g.as_slice() == packet.as_slice()));
        }
    }

    #[test]
    fn an_incoming_hello_is_answered_with_our_own_hello_ack() {
        let StdInterfaceSeam {
            mut worker_context, ..
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::Hello(Capabilities::host()));
        disc.pump(&mut worker_context);

        assert!(
            any_frame_is(&wire.host_wrote(), |m| matches!(
                m,
                Message::HelloAck { tag, .. } if *tag == HOST_TAG
            )),
            "a peer's Hello must draw our HelloAck — the half that lets two hosts confirm each other"
        );
    }

    #[test]
    fn a_host_capable_peer_confirms_on_the_host_lane() {
        let StdInterfaceSeam {
            mut worker_context, ..
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck {
            tag: NodeTag([0x42; 8]),
            capabilities: Capabilities::host(),
        });
        disc.pump(&mut worker_context);

        assert!(matches!(
            disc.devices[0].state,
            LinkState::Confirmed {
                profile: PeerProfile::Host,
                ..
            }
        ));
    }

    #[test]
    fn a_peripheral_peer_confirms_on_the_peripheral_lane() {
        let StdInterfaceSeam {
            mut worker_context, ..
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck {
            tag: NodeTag([0x42; 8]),
            capabilities: Capabilities::none(),
        });
        disc.pump(&mut worker_context);

        assert!(matches!(
            disc.devices[0].state,
            LinkState::Confirmed {
                profile: PeerProfile::Peripheral,
                ..
            }
        ));
    }

    #[test]
    fn a_hello_ack_bearing_our_own_tag_is_rejected_as_self() {
        let StdInterfaceSeam {
            mut worker_context, ..
        } = seam();
        let mut disc = host();
        let wire = MockWire::new();
        let p = wire.port();
        disc.note_present(port("/dev/ttyACM0"), move |_| Ok(p));

        wire.device_sends(Message::HelloAck {
            tag: HOST_TAG,
            capabilities: Capabilities::host(),
        });
        disc.pump(&mut worker_context);

        assert!(
            matches!(disc.devices[0].state, LinkState::Probing { .. }),
            "a HelloAck carrying our own tag is us reflected over a loopback — never confirm a link to ourselves"
        );
    }
}
