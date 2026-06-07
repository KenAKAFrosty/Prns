use embassy_futures::select::{select3, Either3};
use embassy_time::{with_timeout, Duration};
use embedded_io_async::{Read, Write};

use super::super::core::{
    decode_message, react_to, Capabilities, InboundReaction, Message, NodeTag, MAX_DATA_BYTES,
    MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES, READ_CHUNK_BYTES,
};
use crate::interfaces::framing::rns_serial_framing::RnsSerialDecoder;
use crate::interfaces::substrate::EmbassyHostSubstrate;
use crate::interfaces::{
    ConnectionState, ControlCommand, ControlEndpoint, ControlReport, InboundSink,
    InterfaceWorkerContext,
};

const USB_AUTO_MTU: usize = MAX_DATA_BYTES;

/// Whether a host has completed the handshake on this single link. The device holds
/// the engine's outbound — its announces — until a host has said `Hello`: writing
/// frames into a void with no reader lets a write time out mid-frame
/// ([`WRITE_TIMEOUT`]), and that half-frame would desync the host's decoder the
/// moment it connects. Once a host is `Linked`, it is actively reading, so writes
/// drain and complete.
enum HostLink {
    AwaitingHello,
    Linked,
}

impl HostLink {
    fn is_linked(&self) -> bool {
        matches!(self, HostLink::Linked)
    }

    fn note_connected(&mut self, control: &mut impl ControlEndpoint) {
        if !self.is_linked() {
            *self = HostLink::Linked;
            control.report(ControlReport::ConnectionState(ConnectionState::Connected));
        }
    }

    fn note_degraded(&mut self, control: &mut impl ControlEndpoint) {
        if self.is_linked() {
            *self = HostLink::AwaitingHello;
            control.report(ControlReport::ConnectionState(ConnectionState::Degraded));
        }
    }
}

/// Upper bound on one frame's write. With no host reading our link, an unbounded
/// write would wedge the loop; this lets a dropped HelloAck/announce lapse so the
/// next probe (or re-announce) can retry.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

async fn write_frame<W: Write>(tx: &mut W, frame: &[u8]) -> bool {
    matches!(
        with_timeout(WRITE_TIMEOUT, tx.write_all(frame)).await,
        Ok(Ok(()))
    )
}

pub async fn serve<R, W, const MAX_BUFFERED_PACKETS: usize>(
    mut rx: R,
    mut tx: W,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<USB_AUTO_MTU, MAX_BUFFERED_PACKETS>>,
    node_tag: NodeTag,
) where
    R: Read,
    W: Write,
{
    let mut decoder: RnsSerialDecoder<MAX_MESSAGE_BYTES> = RnsSerialDecoder::new();
    let mut read_buf = [0u8; READ_CHUNK_BYTES];
    let mut frame_buf = [0u8; MAX_FRAMED_BYTES];
    let mut host_link = HostLink::AwaitingHello;

    loop {
        match select3(
            rx.read(&mut read_buf),
            context.outbound.ready(),
            context.control.await_command(),
        )
        .await
        {
            Either3::First(result) => {
                let n = match result {
                    Ok(n) => n,
                    Err(_) => {
                        host_link.note_degraded(&mut context.control);
                        0
                    }
                };
                for &byte in &read_buf[..n] {
                    let Ok(Some(frame)) = decoder.feed(byte) else {
                        continue;
                    };
                    match react_to(decode_message(frame)) {
                        InboundReaction::AnswerHandshake => {
                            if let Ok(m) = (Message::HelloAck {
                                tag: node_tag,
                                capabilities: Capabilities::none(),
                            })
                            .write_framed(&mut frame_buf)
                            {
                                if write_frame(&mut tx, &frame_buf[..m]).await {
                                    host_link.note_connected(&mut context.control);
                                }
                            }
                        }
                        InboundReaction::Deliver(packet) => {
                            if host_link.is_linked() && !packet.is_empty() {
                                let _ = context.inbound.submit(|buf| {
                                    buf[..packet.len()].copy_from_slice(packet);
                                    packet.len()
                                });
                            }
                        }
                        InboundReaction::Ignore => {}
                    }
                }
            }
            Either3::Second(()) => {}
            Either3::Third(ControlCommand::Stop) => break,
        }

        // Always drain whatever the runtime queued — keeping the ring clear so the
        // engine never blocks — but only put it on the wire once a host is linked.
        // Before that, the announces are dropped (there is no peer to hear them), not
        // streamed into a void.
        while let Some(mut lease) = context.outbound.lease() {
            if host_link.is_linked() {
                if let Ok(m) = Message::Data(lease.packet()).write_framed(&mut frame_buf) {
                    if !write_frame(&mut tx, &frame_buf[..m]).await {
                        host_link.note_degraded(&mut context.control);
                    }
                }
            }
            lease.complete();
        }
    }
    context.control.report(ControlReport::Stopped);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Reports {
        reports: std::vec::Vec<ControlReport>,
    }

    impl ControlEndpoint for Reports {
        fn next_command(&mut self) -> Option<ControlCommand> {
            None
        }

        fn report(&mut self, report: ControlReport) {
            self.reports.push(report);
        }
    }

    #[test]
    fn link_state_reports_connected_only_once() {
        let mut link = HostLink::AwaitingHello;
        let mut control = Reports::default();

        link.note_connected(&mut control);
        link.note_connected(&mut control);

        assert!(link.is_linked());
        assert!(matches!(
            control.reports.as_slice(),
            [ControlReport::ConnectionState(ConnectionState::Connected)]
        ));
    }

    #[test]
    fn link_state_reports_degraded_when_a_linked_host_fails() {
        let mut link = HostLink::Linked;
        let mut control = Reports::default();

        link.note_degraded(&mut control);
        link.note_degraded(&mut control);

        assert!(!link.is_linked());
        assert!(matches!(
            control.reports.as_slice(),
            [ControlReport::ConnectionState(ConnectionState::Degraded)]
        ));
    }
}
