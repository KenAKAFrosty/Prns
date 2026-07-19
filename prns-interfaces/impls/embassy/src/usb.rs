//! The embassy device side of the plug-and-play USB-auto interface: one link to one host (the desktop or a board acting as USB host), built fresh against the reactor's [`InterfaceSeam`](prns_runtime::reactor::interface_seam). It reuses the legacy interface's framing core wholesale — the same `Prns`-magic handshake — so it speaks the exact wire the reactor (and legacy) host already does.
//!
//! Two behaviours the bare seam doesn't carry, both lifted from the live-proven legacy `serve`: the device answers a host's `Hello` with a `HelloAck` (only then is a host actively reading), and it **holds the engine's outbound until a host has linked** — writing announces into a void with no reader times out mid-frame (`WRITE_TIMEOUT`), and that half-frame would desync the host's decoder the moment it connects. Before a host links, outbound frames are drained and dropped (there is no peer to hear them), not streamed into nothing.

use embassy_futures::select::{select3, Either3};
use embassy_time::{with_timeout, Duration, Timer};
use embedded_io_async::{Read, Write};

use prns_core::interfaces::usb_auto::core::{
    self, Capabilities, InboundReaction, Message, NodeTag,
};
use prns_core::interfaces::{ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind};
use prns_runtime::reactor::driver::EmbassyInterfaceStatus;
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};

/// Upper bound on one frame's write. With no host reading the link, an unbounded write would wedge the loop; this lets a dropped HelloAck/announce lapse so the next probe (or re-announce) can retry.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

const PRESENCE_PROBE_INTERVAL: Duration = Duration::from_secs(2);

const PRESENCE_STRIKES_TO_DORMANT: u8 = 2;

/// A USB-auto device link over an async byte stream pair (the board's USB-Serial-JTAG rx/tx). It
/// holds a `&'a` status handle the firmware shares with its display task; the link writes it as
/// the wire moves, the display reads it lock-free.
pub struct UsbAutoDevice<'a, R, W, P> {
    id: InterfaceId,
    rx: R,
    tx: W,
    node_tag: NodeTag,
    status: &'a EmbassyInterfaceStatus,
    presence: P,
}

impl<'a, R, W, P> UsbAutoDevice<'a, R, W, P> {
    #[must_use]
    pub fn new(
        id: InterfaceId,
        rx: R,
        tx: W,
        status: &'a EmbassyInterfaceStatus,
        presence: P,
    ) -> Self {
        Self {
            id,
            rx,
            tx,
            node_tag: core::node_tag_for(id),
            status,
            presence,
        }
    }
}

impl<R, W, P> Interface for UsbAutoDevice<'_, R, W, P>
where
    R: Read,
    W: Write,
    P: FnMut() -> bool,
{
    const HW_MTU: usize = prns_core::interfaces::usb_auto::core::DEVICE_USB_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::UsbAutoDevice;

    fn descriptor(&self) -> InterfaceDescriptor {
        core::device_descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let UsbAutoDevice {
            id: _,
            mut rx,
            mut tx,
            node_tag,
            status,
            mut presence,
        } = self;
        let mut decoder = core::Decoder::new();
        let mut read_buf = [0u8; core::READ_CHUNK_BYTES];
        let mut frame_buf = [0u8; core::MAX_FRAMED_BYTES];
        let mut linked = false;
        let mut absent_probes = 0u8;

        loop {
            match select3(
                rx.read(&mut read_buf),
                seam.next_outbound(),
                Timer::after(PRESENCE_PROBE_INTERVAL),
            )
            .await
            {
                Either3::First(result) => {
                    absent_probes = 0;
                    let n = result.unwrap_or(0);
                    if n > 0 {
                        status.add_rx(n as u64);
                    }
                    if !status.is_enabled() {
                        continue;
                    }
                    for &byte in &read_buf[..n] {
                        let Ok(Some(frame)) = decoder.feed(byte) else {
                            continue;
                        };
                        if frame.is_empty() {
                            continue;
                        }
                        match core::react_to(core::decode_message(frame)) {
                            InboundReaction::AnswerHandshake => {
                                if !linked {
                                    linked = true;
                                    status.set_connection(ConnectionState::Connected);
                                }
                                let ack = Message::HelloAck {
                                    tag: node_tag,
                                    capabilities: Capabilities::none(),
                                };
                                write_message(&mut tx, &ack, &mut frame_buf, status).await;
                            }
                            InboundReaction::Deliver(packet) => {
                                if !packet.is_empty() {
                                    seam.next_inbound(packet).await;
                                }
                            }
                            InboundReaction::Ignore => {}
                        }
                    }
                }
                Either3::Second(out) => {
                    if linked && status.is_enabled() {
                        let data = Message::Data(out);
                        write_message(&mut tx, &data, &mut frame_buf, status).await;
                    }
                }
                Either3::Third(()) => {
                    if !status.is_enabled() {
                        linked = false;
                        status.set_connection(ConnectionState::Disabled);
                    } else if let Some(connection) =
                        presence_verdict(presence(), &mut absent_probes)
                    {
                        linked = false;
                        status.set_connection(connection);
                    }
                }
            }
        }
    }
}

fn presence_verdict(present: bool, absent_probes: &mut u8) -> Option<ConnectionState> {
    if present {
        *absent_probes = 0;
        return None;
    }
    *absent_probes = absent_probes.saturating_add(1);
    (*absent_probes >= PRESENCE_STRIKES_TO_DORMANT).then_some(ConnectionState::Disconnected)
}

async fn write_message<W: Write>(
    tx: &mut W,
    message: &Message<'_>,
    frame_buf: &mut [u8; core::MAX_FRAMED_BYTES],
    status: &EmbassyInterfaceStatus,
) {
    let Ok(n) = message.write_framed(frame_buf) else {
        return;
    };
    if matches!(
        with_timeout(WRITE_TIMEOUT, tx.write_all(&frame_buf[..n])).await,
        Ok(Ok(()))
    ) {
        status.add_tx(n as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::ifac::IFAC_MAX_SIZE;
    use prns_core::interfaces::InterfaceStatus;
    use prns_runtime::reactor::driver::{leaked_grant_lane, EmbassyInterfaceSeam};
    use prns_runtime::reactor::grant::{GrantConsumer, GrantProducer};

    use ::core::cell::RefCell;
    use ::core::convert::Infallible;
    use embassy_futures::block_on;
    use embassy_futures::select::{select, Either};
    use embassy_futures::yield_now;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_time::{with_timeout, Duration};
    use std::collections::VecDeque;

    const WATCHDOG: Duration = Duration::from_secs(5);

    /// The slot this device's lanes are sized by: its own declared hardware MTU
    /// plus the access tag — not the engine-wide ceiling.
    const DEVICE_SLOT: usize =
        prns_core::interfaces::usb_auto::core::DEVICE_USB_HW_MTU + IFAC_MAX_SIZE;

    /// An in-memory async byte stream over a shared queue: `read` parks (yields) until bytes are
    /// available, `write` appends. One queue is the host->device wire, another the device->host
    /// wire, so the test drives both ends of one link.
    struct MockStream<'a> {
        buf: &'a RefCell<VecDeque<u8>>,
    }

    impl embedded_io_async::ErrorType for MockStream<'_> {
        type Error = Infallible;
    }

    impl Read for MockStream<'_> {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
            loop {
                {
                    let mut queue = self.buf.borrow_mut();
                    if !queue.is_empty() {
                        let n = queue.len().min(out.len());
                        for slot in out.iter_mut().take(n) {
                            *slot = queue.pop_front().expect("non-empty");
                        }
                        return Ok(n);
                    }
                }
                yield_now().await;
            }
        }
    }

    impl Write for MockStream<'_> {
        async fn write(&mut self, data: &[u8]) -> Result<usize, Self::Error> {
            self.buf.borrow_mut().extend(data.iter().copied());
            Ok(data.len())
        }
    }

    fn device_id() -> InterfaceId {
        InterfaceId::new([0xD0; 8])
    }

    /// Read the device->host wire until a decoded frame satisfies `pick`.
    async fn read_until<T>(
        wire: &RefCell<VecDeque<u8>>,
        decoder: &mut core::Decoder,
        mut pick: impl FnMut(Message<'_>) -> Option<T>,
    ) -> T {
        loop {
            let byte = loop {
                if let Some(byte) = wire.borrow_mut().pop_front() {
                    break byte;
                }
                yield_now().await;
            };
            if let Ok(Some(frame)) = decoder.feed(byte) {
                if !frame.is_empty() {
                    if let Ok(message) = core::decode_message(frame) {
                        if let Some(picked) = pick(message) {
                            return picked;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_device_handshakes_a_host_then_carries_data_both_ways() {
        let host_to_device = RefCell::new(VecDeque::new());
        let device_to_host = RefCell::new(VecDeque::new());
        let status = EmbassyInterfaceStatus::new(device_id(), ConnectionState::Initializing);

        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 2> = Channel::new();
        let (in_tx, mut in_rx) = leaked_grant_lane::<DEVICE_SLOT>(2);
        let (mut out_tx, out_rx) = leaked_grant_lane::<DEVICE_SLOT>(2);

        block_on(async {
            let device = UsbAutoDevice::new(
                device_id(),
                MockStream {
                    buf: &host_to_device,
                },
                MockStream {
                    buf: &device_to_host,
                },
                &status,
                || true,
            );
            let seam =
                EmbassyInterfaceSeam::new(device_id(), in_tx, notify.sender(), out_rx, |bytes| {
                    bytes.fill(0)
                });
            let device_run = device.run(seam);

            let driver = async {
                let mut frame = [0u8; core::MAX_FRAMED_BYTES];
                let mut decoder = core::Decoder::new();

                // The host probes with a Hello; the device answers HelloAck and turns Connected.
                let hello = Message::Hello(Capabilities::host());
                let n = hello.write_framed(&mut frame).expect("frames the hello");
                host_to_device
                    .borrow_mut()
                    .extend(frame[..n].iter().copied());

                read_until(&device_to_host, &mut decoder, |message| {
                    matches!(message, Message::HelloAck { .. }).then_some(())
                })
                .await;
                assert_eq!(status.connection(), ConnectionState::Connected);

                // Inbound: a Data frame from the host lands in the device's grant lane,
                // announced on the notify funnel.
                let inbound_packet = [0xAAu8, 0xBB, 0xCC, 0xDD];
                let data = Message::Data(&inbound_packet);
                let n = data.write_framed(&mut frame).expect("frames the data");
                host_to_device
                    .borrow_mut()
                    .extend(frame[..n].iter().copied());
                assert_eq!(notify.receive().await, device_id());
                let received = in_rx.peek().await;
                assert_eq!(received.frame(), &inbound_packet);
                in_rx.release();

                // Outbound: a frame granted into the egress lane is framed onto the
                // device->host wire.
                let outbound_packet = [0x11u8, 0x22, 0x33];
                out_tx.grant().await.fill_for(device_id(), &outbound_packet);
                out_tx.commit();
                let delivered =
                    read_until(&device_to_host, &mut decoder, |message| match message {
                        Message::Data(packet) => Some(packet.to_vec()),
                        _ => None,
                    })
                    .await;
                assert_eq!(delivered, outbound_packet);
            };

            match select(device_run, with_timeout(WATCHDOG, driver)).await {
                Either::Second(result) => result.expect("the link completes before the watchdog"),
                Either::First(()) => unreachable!("the device loop never returns"),
            }
        });
    }

    #[test]
    fn presence_present_clears_strikes_and_holds_the_link() {
        let mut absent = 0u8;
        assert_eq!(presence_verdict(true, &mut absent), None);
        assert_eq!(absent, 0);

        absent = 1;
        assert_eq!(presence_verdict(true, &mut absent), None);
        assert_eq!(absent, 0);
    }

    #[test]
    fn presence_absent_drops_to_dormant_only_after_the_strike_threshold() {
        let mut absent = 0u8;
        assert_eq!(presence_verdict(false, &mut absent), None);
        assert_eq!(absent, 1);
        assert_eq!(
            presence_verdict(false, &mut absent),
            Some(ConnectionState::Disconnected)
        );

        let mut recovered = 1u8;
        assert_eq!(presence_verdict(true, &mut recovered), None);
        assert_eq!(presence_verdict(false, &mut recovered), None);
    }
}
