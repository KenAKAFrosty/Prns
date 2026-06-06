use embassy_futures::select::{select3, Either3};
use embassy_time::{with_timeout, Duration};
use embedded_io_async::{Read, Write};

use super::super::core::{
    decode_message, react_to, InboundReaction, Message, NodeTag, MAX_DATA_BYTES, MAX_FRAMED_BYTES,
    MAX_MESSAGE_BYTES, READ_CHUNK_BYTES,
};
use crate::interfaces::framing::rns_serial_framing::RnsSerialDecoder;
use crate::interfaces::substrate::EmbassyHostSubstrate;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceWorkerContext,
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

/// Upper bound on one frame's write. With no host reading our link, an unbounded
/// write would wedge the loop; this lets a dropped HelloAck/announce lapse so the
/// next probe (or re-announce) can retry.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

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
                let n = result.unwrap_or(0);
                for &byte in &read_buf[..n] {
                    let Ok(Some(frame)) = decoder.feed(byte) else {
                        continue;
                    };
                    match react_to(decode_message(frame)) {
                        InboundReaction::AnswerHandshake => {
                            host_link = HostLink::Linked;
                            if let Ok(m) = Message::HelloAck(node_tag).write_framed(&mut frame_buf)
                            {
                                let _ = with_timeout(WRITE_TIMEOUT, tx.write_all(&frame_buf[..m]))
                                    .await;
                            }
                        }
                        InboundReaction::Deliver(packet) => {
                            if !packet.is_empty() {
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
            if matches!(host_link, HostLink::Linked) {
                if let Ok(m) = Message::Data(lease.packet()).write_framed(&mut frame_buf) {
                    let _ = with_timeout(WRITE_TIMEOUT, tx.write_all(&frame_buf[..m])).await;
                }
            }
            lease.complete();
        }
    }
    context.control.report(ControlReport::Stopped);
}
