mod device;

pub use device::{
    WebUsbAutoClass, WebUsbAutoError, WebUsbAutoRx, WebUsbAutoState, WebUsbAutoTx,
    WebUsbBootloaderEntry, WEBUSB_AUTO_CONTROL_BUFFER_BYTES, WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES,
    WEBUSB_AUTO_PACKET_SIZE,
};

use embassy_futures::select::{select, select3, Either, Either3};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use embedded_io_async::{Error, ErrorKind, Read, Write};

use prns_core::interfaces::usb_auto::{
    self as contract, Capabilities, InboundReaction, Message, NodeTag,
};
use prns_core::interfaces::{
    ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceStatus,
};
use prns_runtime::manifold::driver::EmbassyInterfaceStatus;
use prns_runtime::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};

const WRITE_TIMEOUT: Duration = Duration::from_millis(200);
const IO_RETRY_DELAY: Duration = Duration::from_millis(100);
const PRESENCE_PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PRESENCE_STRIKES_TO_DORMANT: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
enum UsbLifecycle {
    AwaitingHost,
    Linked,
    Degraded,
    Failed,
}

impl UsbLifecycle {
    fn is_linked(&self) -> bool {
        match self {
            Self::Linked | Self::Degraded => true,
            Self::AwaitingHost | Self::Failed => false,
        }
    }

    fn publish(&self, status: &EmbassyInterfaceStatus) {
        let connection = match self {
            Self::AwaitingHost => ConnectionState::Disconnected,
            Self::Linked => ConnectionState::Connected,
            Self::Degraded => ConnectionState::Degraded,
            Self::Failed => ConnectionState::Failed,
        };
        status.set_connection(connection);
    }

    fn connect(&mut self, status: &EmbassyInterfaceStatus) {
        *self = Self::Linked;
        self.publish(status);
    }

    fn recover(&mut self, status: &EmbassyInterfaceStatus) {
        if matches!(self, Self::Degraded) {
            self.connect(status);
        }
    }

    fn degrade(&mut self, status: &EmbassyInterfaceStatus) {
        match self {
            Self::Linked | Self::Degraded => {
                *self = Self::Degraded;
                self.publish(status);
            }
            Self::AwaitingHost | Self::Failed => {}
        }
    }

    fn disconnect(&mut self, status: &EmbassyInterfaceStatus) {
        if matches!(self, Self::Failed) {
            return;
        }
        *self = Self::AwaitingHost;
        self.publish(status);
    }

    fn fail(&mut self, status: &EmbassyInterfaceStatus) {
        *self = Self::Failed;
        self.publish(status);
    }

    fn disable(&mut self) {
        if !matches!(self, Self::Failed) {
            *self = Self::AwaitingHost;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReadOutcome {
    Bytes(usize),
    RetryReady,
    EndOfStream,
    TransientFailure,
    Disconnected,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
enum WriteOutcome {
    Sent(usize),
    TimedOut,
    Disabled,
    Disconnected,
    TransientFailure,
    Failed,
    Rejected,
}

enum IoEvent<'a> {
    Read(ReadOutcome),
    Outbound(&'a [u8]),
}

enum IoPriority {
    Read,
    Outbound,
}

impl IoPriority {
    fn alternate(&mut self) {
        *self = match self {
            Self::Read => Self::Outbound,
            Self::Outbound => Self::Read,
        };
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PresenceVerdict {
    Present,
    SuspectedAbsent,
    Absent,
}

pub struct UsbAutoDeviceInput<'a, R, W, P> {
    pub rx: R,
    pub tx: W,
    pub status: &'a EmbassyInterfaceStatus,
    pub host_present: P,
}

pub struct UsbAutoDevice<'a, R, W, P> {
    id: InterfaceId,
    rx: R,
    tx: W,
    node_tag: NodeTag,
    status: &'a EmbassyInterfaceStatus,
    host_present: P,
}

impl<'a, R, W, P> UsbAutoDevice<'a, R, W, P> {
    #[must_use]
    pub fn new(input: UsbAutoDeviceInput<'a, R, W, P>) -> Self {
        let UsbAutoDeviceInput {
            rx,
            tx,
            status,
            host_present,
        } = input;
        let id = status.id();
        Self {
            id,
            rx,
            tx,
            node_tag: contract::node_tag_for(id),
            status,
            host_present,
        }
    }
}

impl<R, W, P> Interface for UsbAutoDevice<'_, R, W, P>
where
    R: Read,
    W: Write,
    P: FnMut() -> bool,
{
    const HW_MTU: usize = prns_core::interfaces::usb_auto::DEVICE_USB_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::UsbAutoDevice;

    fn descriptor(&self) -> InterfaceDescriptor {
        contract::device_descriptor(self.id)
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
            mut host_present,
        } = self;
        let mut decoder = contract::Decoder::new();
        let mut read_buf = [0u8; contract::READ_CHUNK_BYTES];
        let mut frame_buf = [0u8; contract::MAX_FRAMED_BYTES];
        let mut lifecycle = UsbLifecycle::AwaitingHost;
        let mut absent_probes = 0u8;
        let mut read_retry_at = None;
        let mut presence_probe_at = Instant::now() + PRESENCE_PROBE_INTERVAL;
        let mut io_priority = IoPriority::Read;

        lifecycle.publish(status);

        loop {
            if !status.is_enabled() {
                lifecycle.disable();
                decoder = contract::Decoder::new();
                read_retry_at = None;
                absent_probes = 0;
                status.wait_until_enabled().await;
                lifecycle.publish(status);
                presence_probe_at = Instant::now() + PRESENCE_PROBE_INTERVAL;
            }
            match select3(
                status.wait_until_disabled(),
                Timer::at(presence_probe_at),
                next_io(
                    &mut rx,
                    &mut read_buf,
                    read_retry_at,
                    &mut seam,
                    &io_priority,
                ),
            )
            .await
            {
                Either3::First(()) => {}
                Either3::Second(()) => {
                    presence_probe_at = Instant::now() + PRESENCE_PROBE_INTERVAL;
                    match presence_verdict(host_present(), &mut absent_probes) {
                        PresenceVerdict::Present | PresenceVerdict::SuspectedAbsent => {}
                        PresenceVerdict::Absent => {
                            decoder = contract::Decoder::new();
                            lifecycle.disconnect(status);
                        }
                    }
                }
                Either3::Third(event) => {
                    io_priority.alternate();
                    match event {
                        IoEvent::Read(ReadOutcome::Bytes(n)) => {
                            read_retry_at = None;
                            absent_probes = 0;
                            status.add_rx(n as u64);
                            lifecycle.recover(status);
                            for &byte in &read_buf[..n] {
                                let frame = match decoder.feed(byte) {
                                    Ok(Some(frame)) => frame,
                                    Ok(None) => continue,
                                    Err(_) => {
                                        status.count_frame_undecodable();
                                        continue;
                                    }
                                };
                                if frame.is_empty() {
                                    continue;
                                }
                                status.count_frame_in();
                                let message = match contract::decode_message(frame) {
                                    Ok(message) => message,
                                    Err(_) => {
                                        status.count_frame_malformed();
                                        continue;
                                    }
                                };
                                match contract::react_to(Ok(message)) {
                                    InboundReaction::AnswerHandshake => {
                                        let ack = Message::HelloAck {
                                            tag: node_tag,
                                            capabilities: Capabilities::none(),
                                        };
                                        match write_message(&mut tx, &ack, &mut frame_buf, status)
                                            .await
                                        {
                                            WriteOutcome::Sent(n) => {
                                                status.add_tx(n as u64);
                                                lifecycle.connect(status);
                                            }
                                            WriteOutcome::TimedOut
                                            | WriteOutcome::TransientFailure => {
                                                lifecycle.degrade(status);
                                            }
                                            WriteOutcome::Disconnected => {
                                                decoder = contract::Decoder::new();
                                                lifecycle.disconnect(status);
                                            }
                                            WriteOutcome::Failed | WriteOutcome::Rejected => {
                                                decoder = contract::Decoder::new();
                                                lifecycle.fail(status);
                                            }
                                            WriteOutcome::Disabled => {}
                                        }
                                    }
                                    InboundReaction::Deliver(packet) => {
                                        if lifecycle.is_linked() && !packet.is_empty() {
                                            seam.next_inbound(packet).await;
                                            status.count_frame_delivered();
                                        } else if packet.is_empty() {
                                            status.count_frame_malformed();
                                        }
                                    }
                                    InboundReaction::Ignore => {}
                                }
                            }
                        }
                        IoEvent::Read(ReadOutcome::RetryReady) => {
                            read_retry_at = None;
                        }
                        IoEvent::Read(ReadOutcome::EndOfStream | ReadOutcome::Disconnected) => {
                            decoder = contract::Decoder::new();
                            lifecycle.disconnect(status);
                            read_retry_at = Some(Instant::now() + IO_RETRY_DELAY);
                        }
                        IoEvent::Read(ReadOutcome::TransientFailure) => {
                            lifecycle.degrade(status);
                            read_retry_at = Some(Instant::now() + IO_RETRY_DELAY);
                        }
                        IoEvent::Read(ReadOutcome::Failed) => {
                            decoder = contract::Decoder::new();
                            lifecycle.fail(status);
                            read_retry_at = Some(Instant::now() + IO_RETRY_DELAY);
                        }
                        IoEvent::Outbound(out) => {
                            let disposition = if !lifecycle.is_linked() {
                                OutboundDisposition::Dropped(OutboundDropReason::Disconnected)
                            } else {
                                let data = Message::Data(out);
                                match write_message(&mut tx, &data, &mut frame_buf, status).await {
                                    WriteOutcome::Sent(n) => {
                                        status.add_tx(n as u64);
                                        lifecycle.recover(status);
                                        OutboundDisposition::Sent
                                    }
                                    WriteOutcome::TimedOut => {
                                        lifecycle.degrade(status);
                                        OutboundDisposition::Dropped(OutboundDropReason::TimedOut)
                                    }
                                    WriteOutcome::Disabled => {
                                        OutboundDisposition::Dropped(OutboundDropReason::Disabled)
                                    }
                                    WriteOutcome::Disconnected => {
                                        decoder = contract::Decoder::new();
                                        lifecycle.disconnect(status);
                                        OutboundDisposition::Dropped(
                                            OutboundDropReason::Disconnected,
                                        )
                                    }
                                    WriteOutcome::TransientFailure => {
                                        lifecycle.degrade(status);
                                        OutboundDisposition::Dropped(
                                            OutboundDropReason::TransportFailure,
                                        )
                                    }
                                    WriteOutcome::Failed => {
                                        lifecycle.fail(status);
                                        OutboundDisposition::Dropped(
                                            OutboundDropReason::TransportFailure,
                                        )
                                    }
                                    WriteOutcome::Rejected => {
                                        lifecycle.fail(status);
                                        OutboundDisposition::Dropped(OutboundDropReason::Rejected)
                                    }
                                }
                            };
                            seam.complete_outbound(disposition);
                        }
                    }
                }
            }
        }
    }
}

async fn next_io<'a, R, Seam>(
    rx: &'a mut R,
    read_buf: &'a mut [u8; contract::READ_CHUNK_BYTES],
    read_retry_at: Option<Instant>,
    seam: &'a mut Seam,
    priority: &IoPriority,
) -> IoEvent<'a>
where
    R: Read,
    Seam: InterfaceSeam,
{
    if matches!(priority, IoPriority::Outbound) {
        return match select(seam.next_outbound(), read_once(rx, read_buf, read_retry_at)).await {
            Either::First(out) => IoEvent::Outbound(out),
            Either::Second(outcome) => IoEvent::Read(outcome),
        };
    }
    match select(read_once(rx, read_buf, read_retry_at), seam.next_outbound()).await {
        Either::First(outcome) => IoEvent::Read(outcome),
        Either::Second(out) => IoEvent::Outbound(out),
    }
}

async fn read_once<R: Read>(
    rx: &mut R,
    read_buf: &mut [u8; contract::READ_CHUNK_BYTES],
    retry_at: Option<Instant>,
) -> ReadOutcome {
    if let Some(retry_at) = retry_at {
        Timer::at(retry_at).await;
        return ReadOutcome::RetryReady;
    }
    match rx.read(read_buf).await {
        Ok(0) => ReadOutcome::EndOfStream,
        Ok(n) => ReadOutcome::Bytes(n),
        Err(error) => match classify_io_error(error.kind()) {
            IoFailure::Transient => ReadOutcome::TransientFailure,
            IoFailure::Disconnected => ReadOutcome::Disconnected,
            IoFailure::Failed => ReadOutcome::Failed,
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
enum IoFailure {
    Transient,
    Disconnected,
    Failed,
}

fn classify_io_error(kind: ErrorKind) -> IoFailure {
    match kind {
        ErrorKind::TimedOut | ErrorKind::Interrupted => IoFailure::Transient,
        ErrorKind::NotFound
        | ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::AddrNotAvailable
        | ErrorKind::BrokenPipe => IoFailure::Disconnected,
        _ => IoFailure::Failed,
    }
}

fn presence_verdict(present: bool, absent_probes: &mut u8) -> PresenceVerdict {
    if present {
        *absent_probes = 0;
        return PresenceVerdict::Present;
    }
    *absent_probes = absent_probes.saturating_add(1);
    if *absent_probes >= PRESENCE_STRIKES_TO_DORMANT {
        PresenceVerdict::Absent
    } else {
        PresenceVerdict::SuspectedAbsent
    }
}

async fn write_message<W: Write>(
    tx: &mut W,
    message: &Message<'_>,
    frame_buf: &mut [u8; contract::MAX_FRAMED_BYTES],
    status: &EmbassyInterfaceStatus,
) -> WriteOutcome {
    let Ok(n) = message.write_framed(frame_buf) else {
        return WriteOutcome::Rejected;
    };
    match select(
        status.wait_until_disabled(),
        with_timeout(WRITE_TIMEOUT, async {
            tx.write_all(&frame_buf[..n]).await?;
            tx.flush().await
        }),
    )
    .await
    {
        Either::First(()) => WriteOutcome::Disabled,
        Either::Second(Err(_)) => WriteOutcome::TimedOut,
        Either::Second(Ok(Ok(()))) => WriteOutcome::Sent(n),
        Either::Second(Ok(Err(error))) => match classify_io_error(error.kind()) {
            IoFailure::Transient => WriteOutcome::TransientFailure,
            IoFailure::Disconnected => WriteOutcome::Disconnected,
            IoFailure::Failed => WriteOutcome::Failed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::{
        FrameAccounting, FrameSink, InterfaceOriginKind, InterfaceStatus, IFAC_MAX_SIZE,
    };
    use prns_runtime::manifold::driver::{leaked_grant_lane, EmbassyInterfaceSeam};
    use prns_runtime::manifold::grant::{GrantConsumer, GrantProducer};

    use ::core::cell::{Cell, RefCell};
    use ::core::convert::Infallible;
    use ::core::future::pending;
    use embassy_futures::block_on;
    use embassy_futures::join::join;
    use embassy_futures::select::{select, Either};
    use embassy_futures::yield_now;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_time::{with_timeout, Duration};
    use std::collections::VecDeque;

    const WATCHDOG: Duration = Duration::from_secs(5);

    const DEVICE_SLOT: usize = prns_core::interfaces::usb_auto::DEVICE_USB_HW_MTU + IFAC_MAX_SIZE;

    struct MockStream<'a> {
        buf: &'a RefCell<VecDeque<u8>>,
    }

    struct RecordingSeam<'a, S> {
        inner: S,
        dispositions: &'a RefCell<Vec<OutboundDisposition>>,
    }

    impl<S: InterfaceSeam> InterfaceSeam for RecordingSeam<'_, S> {
        fn interface_origin(&self) -> InterfaceOriginKind {
            self.inner.interface_origin()
        }

        fn fill_entropy(&mut self, bytes: &mut [u8]) {
            self.inner.fill_entropy(bytes);
        }

        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            self.inner.inbound_sink().await
        }

        async fn commit_inbound(&mut self) {
            self.inner.commit_inbound().await;
        }

        async fn next_inbound(&mut self, frame: &[u8]) {
            self.inner.next_inbound(frame).await;
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.inner.next_outbound().await
        }

        fn complete_outbound(&mut self, disposition: OutboundDisposition) {
            self.inner.complete_outbound(disposition.clone());
            self.dispositions.borrow_mut().push(disposition);
        }
    }

    #[derive(Debug)]
    struct MockIoError(ErrorKind);

    impl embedded_io_async::Error for MockIoError {
        fn kind(&self) -> ErrorKind {
            self.0
        }
    }

    enum MockReadAction {
        Bytes(Vec<u8>),
        EndOfStream,
        Error(ErrorKind),
        Pending,
    }

    struct ScriptedReader<'a> {
        actions: &'a RefCell<VecDeque<MockReadAction>>,
        calls: &'a Cell<usize>,
        cancellations: &'a Cell<usize>,
    }

    struct CancellationGuard<'a> {
        cancellations: &'a Cell<usize>,
    }

    impl Drop for CancellationGuard<'_> {
        fn drop(&mut self) {
            self.cancellations.set(self.cancellations.get() + 1);
        }
    }

    impl embedded_io_async::ErrorType for ScriptedReader<'_> {
        type Error = MockIoError;
    }

    impl Read for ScriptedReader<'_> {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
            self.calls.set(self.calls.get() + 1);
            let action = self.actions.borrow_mut().pop_front();
            match action {
                Some(MockReadAction::Bytes(bytes)) => {
                    let n = bytes.len().min(out.len());
                    out[..n].copy_from_slice(&bytes[..n]);
                    Ok(n)
                }
                Some(MockReadAction::EndOfStream) => Ok(0),
                Some(MockReadAction::Error(kind)) => Err(MockIoError(kind)),
                Some(MockReadAction::Pending) | None => {
                    let _guard = CancellationGuard {
                        cancellations: self.cancellations,
                    };
                    pending().await
                }
            }
        }
    }

    enum MockWriteAction {
        Accept,
        Error(ErrorKind),
        Pending,
    }

    struct ScriptedWriter<'a> {
        actions: &'a RefCell<VecDeque<MockWriteAction>>,
        cancellations: &'a Cell<usize>,
    }

    struct FlushRecordingWriter<'a> {
        bytes: &'a RefCell<Vec<u8>>,
        flushes: &'a Cell<usize>,
    }

    impl embedded_io_async::ErrorType for ScriptedWriter<'_> {
        type Error = MockIoError;
    }

    impl Write for ScriptedWriter<'_> {
        async fn write(&mut self, data: &[u8]) -> Result<usize, Self::Error> {
            let action = self.actions.borrow_mut().pop_front();
            match action {
                Some(MockWriteAction::Accept) => Ok(data.len()),
                Some(MockWriteAction::Error(kind)) => Err(MockIoError(kind)),
                Some(MockWriteAction::Pending) | None => {
                    let _guard = CancellationGuard {
                        cancellations: self.cancellations,
                    };
                    pending().await
                }
            }
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    impl embedded_io_async::ErrorType for FlushRecordingWriter<'_> {
        type Error = Infallible;
    }

    impl Write for FlushRecordingWriter<'_> {
        async fn write(&mut self, data: &[u8]) -> Result<usize, Self::Error> {
            self.bytes.borrow_mut().extend_from_slice(data);
            Ok(data.len())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            self.flushes.set(self.flushes.get() + 1);
            Ok(())
        }
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

    async fn read_until<T>(
        wire: &RefCell<VecDeque<u8>>,
        decoder: &mut contract::Decoder,
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
                    if let Ok(message) = contract::decode_message(frame) {
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
        let dispositions = RefCell::new(Vec::new());
        let status =
            EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Initializing);

        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 2> = Channel::new();
        let (in_tx, mut in_rx) = leaked_grant_lane::<DEVICE_SLOT>(2);
        let (mut out_tx, out_rx) = leaked_grant_lane::<DEVICE_SLOT>(1);

        block_on(async {
            let device = UsbAutoDevice::new(UsbAutoDeviceInput {
                rx: MockStream {
                    buf: &host_to_device,
                },
                tx: MockStream {
                    buf: &device_to_host,
                },
                status: &status,
                host_present: || true,
            });
            let inner =
                EmbassyInterfaceSeam::new(device_id(), in_tx, notify.sender(), out_rx, |bytes| {
                    bytes.fill(0)
                });
            let seam = RecordingSeam {
                inner,
                dispositions: &dispositions,
            };
            let device_run = device.run(seam);

            let driver = async {
                let mut frame = [0u8; contract::MAX_FRAMED_BYTES];
                let mut decoder = contract::Decoder::new();

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
                while dispositions.borrow().is_empty() {
                    yield_now().await;
                }
                assert_eq!(
                    dispositions.borrow().as_slice(),
                    &[OutboundDisposition::Sent]
                );
                assert_eq!(
                    status.frame_accounting(),
                    Some(FrameAccounting {
                        frames_in: 2,
                        malformed: 0,
                        protocol_violations: 0,
                        undecodable: 0,
                        delivered: 1,
                    })
                );
                let next = with_timeout(WATCHDOG, out_tx.grant())
                    .await
                    .expect("completed outbound releases its lane slot");
                next.fill_for(device_id(), &[0x44]);
            };

            match select(device_run, with_timeout(WATCHDOG, driver)).await {
                Either::Second(result) => result.expect("the link completes before the watchdog"),
                Either::First(()) => unreachable!("the device loop never returns"),
            }
        });
    }

    #[test]
    fn outbound_while_unlinked_is_typed_and_releases_its_lane_slot() {
        let host_to_device = RefCell::new(VecDeque::new());
        let device_to_host = RefCell::new(VecDeque::new());
        let dispositions = RefCell::new(Vec::new());
        let status =
            EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Initializing);
        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 1> = Channel::new();
        let (in_tx, _in_rx) = leaked_grant_lane::<DEVICE_SLOT>(1);
        let (mut out_tx, out_rx) = leaked_grant_lane::<DEVICE_SLOT>(1);

        block_on(async {
            let device = UsbAutoDevice::new(UsbAutoDeviceInput {
                rx: MockStream {
                    buf: &host_to_device,
                },
                tx: MockStream {
                    buf: &device_to_host,
                },
                status: &status,
                host_present: || true,
            });
            let inner =
                EmbassyInterfaceSeam::new(device_id(), in_tx, notify.sender(), out_rx, |bytes| {
                    bytes.fill(0)
                });
            let seam = RecordingSeam {
                inner,
                dispositions: &dispositions,
            };
            let device_run = device.run(seam);

            let driver = async {
                out_tx.grant().await.fill_for(device_id(), &[0x55]);
                out_tx.commit();
                while dispositions.borrow().is_empty() {
                    yield_now().await;
                }
                assert_eq!(
                    dispositions.borrow().as_slice(),
                    &[OutboundDisposition::Dropped(
                        OutboundDropReason::Disconnected
                    )]
                );
                with_timeout(WATCHDOG, out_tx.grant())
                    .await
                    .expect("discarded outbound releases its lane slot");
            };

            match select(device_run, with_timeout(WATCHDOG, driver)).await {
                Either::Second(result) => {
                    result.expect("the discard completes before the watchdog")
                }
                Either::First(()) => unreachable!("the device loop never returns"),
            }
        });
    }

    #[test]
    fn read_outcomes_distinguish_bytes_eof_transient_disconnect_and_failure() {
        let actions = RefCell::new(VecDeque::from([
            MockReadAction::Bytes(vec![0xAA, 0xBB]),
            MockReadAction::EndOfStream,
            MockReadAction::Error(ErrorKind::Interrupted),
            MockReadAction::Error(ErrorKind::NotConnected),
            MockReadAction::Error(ErrorKind::Other),
        ]));
        let calls = Cell::new(0);
        let cancellations = Cell::new(0);
        let mut reader = ScriptedReader {
            actions: &actions,
            calls: &calls,
            cancellations: &cancellations,
        };
        let mut read_buf = [0u8; contract::READ_CHUNK_BYTES];

        block_on(async {
            assert_eq!(
                read_once(&mut reader, &mut read_buf, None).await,
                ReadOutcome::Bytes(2)
            );
            assert_eq!(&read_buf[..2], &[0xAA, 0xBB]);
            assert_eq!(
                read_once(&mut reader, &mut read_buf, None).await,
                ReadOutcome::EndOfStream
            );
            assert_eq!(
                read_once(&mut reader, &mut read_buf, None).await,
                ReadOutcome::TransientFailure
            );
            assert_eq!(
                read_once(&mut reader, &mut read_buf, None).await,
                ReadOutcome::Disconnected
            );
            assert_eq!(
                read_once(&mut reader, &mut read_buf, None).await,
                ReadOutcome::Failed
            );
        });
        assert_eq!(calls.get(), 5);
        assert_eq!(cancellations.get(), 0);
    }

    #[test]
    fn retry_backoff_does_not_poll_an_immediately_failing_reader() {
        let actions = RefCell::new(VecDeque::from([MockReadAction::Error(ErrorKind::Other)]));
        let calls = Cell::new(0);
        let cancellations = Cell::new(0);
        let mut reader = ScriptedReader {
            actions: &actions,
            calls: &calls,
            cancellations: &cancellations,
        };
        let mut read_buf = [0u8; contract::READ_CHUNK_BYTES];

        block_on(async {
            assert_eq!(
                read_once(&mut reader, &mut read_buf, None).await,
                ReadOutcome::Failed
            );
            assert_eq!(
                read_once(&mut reader, &mut read_buf, Some(Instant::now())).await,
                ReadOutcome::RetryReady
            );
        });
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn disabling_cancels_a_blocked_write_before_its_timeout() {
        let actions = RefCell::new(VecDeque::from([MockWriteAction::Pending]));
        let cancellations = Cell::new(0);
        let mut writer = ScriptedWriter {
            actions: &actions,
            cancellations: &cancellations,
        };
        let status = EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Connected);
        let mut frame_buf = [0u8; contract::MAX_FRAMED_BYTES];

        block_on(async {
            let (outcome, ()) = join(
                write_message(
                    &mut writer,
                    &Message::Data(&[0x11]),
                    &mut frame_buf,
                    &status,
                ),
                async {
                    yield_now().await;
                    status.disable();
                },
            )
            .await;
            assert_eq!(outcome, WriteOutcome::Disabled);
        });
        assert_eq!(cancellations.get(), 1);
    }

    #[test]
    fn each_framed_message_flushes_its_usb_transfer_boundary() {
        let bytes = RefCell::new(Vec::new());
        let flushes = Cell::new(0);
        let mut writer = FlushRecordingWriter {
            bytes: &bytes,
            flushes: &flushes,
        };
        let status = EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Connected);
        let mut frame_buf = [0u8; contract::MAX_FRAMED_BYTES];

        block_on(async {
            assert!(matches!(
                write_message(
                    &mut writer,
                    &Message::Data(&[0x11; 64]),
                    &mut frame_buf,
                    &status,
                )
                .await,
                WriteOutcome::Sent(_)
            ));
        });

        assert!(!bytes.borrow().is_empty());
        assert_eq!(flushes.get(), 1);
    }

    #[test]
    fn write_outcomes_preserve_transport_failure_structure() {
        let actions = RefCell::new(VecDeque::from([
            MockWriteAction::Accept,
            MockWriteAction::Error(ErrorKind::NotConnected),
            MockWriteAction::Error(ErrorKind::Interrupted),
            MockWriteAction::Error(ErrorKind::Other),
        ]));
        let cancellations = Cell::new(0);
        let mut writer = ScriptedWriter {
            actions: &actions,
            cancellations: &cancellations,
        };
        let status = EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Connected);
        let mut frame_buf = [0u8; contract::MAX_FRAMED_BYTES];

        block_on(async {
            assert!(matches!(
                write_message(
                    &mut writer,
                    &Message::Data(&[0x11]),
                    &mut frame_buf,
                    &status
                )
                .await,
                WriteOutcome::Sent(_)
            ));
            assert_eq!(
                write_message(
                    &mut writer,
                    &Message::Data(&[0x22]),
                    &mut frame_buf,
                    &status
                )
                .await,
                WriteOutcome::Disconnected
            );
            assert_eq!(
                write_message(
                    &mut writer,
                    &Message::Data(&[0x33]),
                    &mut frame_buf,
                    &status
                )
                .await,
                WriteOutcome::TransientFailure
            );
            assert_eq!(
                write_message(
                    &mut writer,
                    &Message::Data(&[0x44]),
                    &mut frame_buf,
                    &status
                )
                .await,
                WriteOutcome::Failed
            );
        });
    }

    #[test]
    fn blocked_write_times_out_and_cancels_the_transport_future() {
        let actions = RefCell::new(VecDeque::from([MockWriteAction::Pending]));
        let cancellations = Cell::new(0);
        let mut writer = ScriptedWriter {
            actions: &actions,
            cancellations: &cancellations,
        };
        let status = EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Connected);
        let mut frame_buf = [0u8; contract::MAX_FRAMED_BYTES];

        block_on(async {
            assert_eq!(
                write_message(
                    &mut writer,
                    &Message::Data(&[0x77]),
                    &mut frame_buf,
                    &status
                )
                .await,
                WriteOutcome::TimedOut
            );
        });
        assert_eq!(cancellations.get(), 1);
    }

    #[test]
    fn failure_state_survives_disable_and_reenable() {
        let status =
            EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Initializing);
        let mut lifecycle = UsbLifecycle::Failed;
        lifecycle.publish(&status);
        assert_eq!(status.connection(), ConnectionState::Failed);

        status.disable();
        lifecycle.disable();
        assert_eq!(status.connection(), ConnectionState::Disabled);

        status.enable();
        lifecycle.publish(&status);
        assert_eq!(status.connection(), ConnectionState::Failed);
    }

    #[test]
    fn data_before_handshake_is_not_delivered() {
        let host_to_device = RefCell::new(VecDeque::new());
        let device_to_host = RefCell::new(VecDeque::new());
        let status =
            EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Initializing);
        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 1> = Channel::new();
        let (in_tx, _in_rx) = leaked_grant_lane::<DEVICE_SLOT>(1);
        let (_out_tx, out_rx) = leaked_grant_lane::<DEVICE_SLOT>(1);
        let mut frame = [0u8; contract::MAX_FRAMED_BYTES];
        let n = Message::Data(&[0xAA, 0xBB])
            .write_framed(&mut frame)
            .expect("frames pre-handshake data");
        host_to_device
            .borrow_mut()
            .extend(frame[..n].iter().copied());

        block_on(async {
            let device = UsbAutoDevice::new(UsbAutoDeviceInput {
                rx: MockStream {
                    buf: &host_to_device,
                },
                tx: MockStream {
                    buf: &device_to_host,
                },
                status: &status,
                host_present: || true,
            });
            let seam =
                EmbassyInterfaceSeam::new(device_id(), in_tx, notify.sender(), out_rx, |bytes| {
                    bytes.fill(0)
                });
            match select(
                device.run(seam),
                with_timeout(Duration::from_millis(20), notify.receive()),
            )
            .await
            {
                Either::Second(result) => {
                    assert!(result.is_err(), "pre-handshake data reached the manifold");
                }
                Either::First(()) => unreachable!("the device loop never returns"),
            }
        });
        assert_eq!(status.connection(), ConnectionState::Disconnected);
    }

    #[test]
    fn failed_hello_ack_never_publishes_connected() {
        let mut frame = [0u8; contract::MAX_FRAMED_BYTES];
        let n = Message::Hello(Capabilities::host())
            .write_framed(&mut frame)
            .expect("frames hello");
        let read_actions = RefCell::new(VecDeque::from([
            MockReadAction::Bytes(frame[..n].to_vec()),
            MockReadAction::Pending,
        ]));
        let read_calls = Cell::new(0);
        let read_cancellations = Cell::new(0);
        let write_actions = RefCell::new(VecDeque::from([MockWriteAction::Error(
            ErrorKind::NotConnected,
        )]));
        let write_cancellations = Cell::new(0);
        let status =
            EmbassyInterfaceStatus::new_accounted(device_id(), ConnectionState::Initializing);
        let notify: Channel<CriticalSectionRawMutex, InterfaceId, 1> = Channel::new();
        let (in_tx, _in_rx) = leaked_grant_lane::<DEVICE_SLOT>(1);
        let (_out_tx, out_rx) = leaked_grant_lane::<DEVICE_SLOT>(1);

        block_on(async {
            let device = UsbAutoDevice::new(UsbAutoDeviceInput {
                rx: ScriptedReader {
                    actions: &read_actions,
                    calls: &read_calls,
                    cancellations: &read_cancellations,
                },
                tx: ScriptedWriter {
                    actions: &write_actions,
                    cancellations: &write_cancellations,
                },
                status: &status,
                host_present: || true,
            });
            let seam =
                EmbassyInterfaceSeam::new(device_id(), in_tx, notify.sender(), out_rx, |bytes| {
                    bytes.fill(0)
                });
            match select(device.run(seam), Timer::after(Duration::from_millis(20))).await {
                Either::Second(()) => {}
                Either::First(()) => unreachable!("the device loop never returns"),
            }
        });
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert!(write_actions.borrow().is_empty());
    }

    #[test]
    fn cancelled_pending_read_consumes_no_scripted_data() {
        let actions = RefCell::new(VecDeque::from([
            MockReadAction::Pending,
            MockReadAction::Bytes(vec![0xAB]),
        ]));
        let calls = Cell::new(0);
        let cancellations = Cell::new(0);
        let mut reader = ScriptedReader {
            actions: &actions,
            calls: &calls,
            cancellations: &cancellations,
        };
        let mut read_buf = [0u8; contract::READ_CHUNK_BYTES];

        block_on(async {
            assert!(matches!(
                select(read_once(&mut reader, &mut read_buf, None), async {
                    yield_now().await
                })
                .await,
                Either::Second(())
            ));
            assert_eq!(cancellations.get(), 1);
            assert_eq!(
                read_once(&mut reader, &mut read_buf, None).await,
                ReadOutcome::Bytes(1)
            );
            assert_eq!(read_buf[0], 0xAB);
        });
    }

    #[test]
    fn presence_present_clears_strikes_and_holds_the_link() {
        let mut absent = 0u8;
        assert_eq!(
            presence_verdict(true, &mut absent),
            PresenceVerdict::Present
        );
        assert_eq!(absent, 0);

        absent = 1;
        assert_eq!(
            presence_verdict(true, &mut absent),
            PresenceVerdict::Present
        );
        assert_eq!(absent, 0);
    }

    #[test]
    fn presence_absent_disconnects_only_after_the_strike_threshold() {
        let mut absent = 0u8;
        assert_eq!(
            presence_verdict(false, &mut absent),
            PresenceVerdict::SuspectedAbsent
        );
        assert_eq!(absent, 1);
        assert_eq!(
            presence_verdict(false, &mut absent),
            PresenceVerdict::Absent
        );

        let mut recovered = 1u8;
        assert_eq!(
            presence_verdict(true, &mut recovered),
            PresenceVerdict::Present
        );
        assert_eq!(
            presence_verdict(false, &mut recovered),
            PresenceVerdict::SuspectedAbsent
        );
    }
}
