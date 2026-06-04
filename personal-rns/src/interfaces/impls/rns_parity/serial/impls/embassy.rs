use embassy_futures::select::{select3, Either3};
use embassy_time::{with_timeout, Duration};
use embedded_io_async::{Read, Write};

use super::super::core::SERIAL_MTU;
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::substrate::EmbassyHostSubstrate;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceWorkerContext,
};

/// Upper bound on one frame's write. If nothing is draining the link (no host
/// attached, or no peer reading), an unbounded write would wedge the loop.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

pub async fn serve<R, W, const MAX_BUFFERED_PACKETS: usize>(
    mut rx: R,
    mut tx: W,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<SERIAL_MTU, MAX_BUFFERED_PACKETS>>,
) where
    R: Read,
    W: Write,
{
    let mut decoder: RnsSerialDecoder<SERIAL_MTU> = RnsSerialDecoder::new();
    let mut read_buf = [0u8; 64];
    let mut frame_buf = [0u8; rns_serial_framing::max_encoded_len(SERIAL_MTU)];
    let mut packet_buf = [0u8; SERIAL_MTU];

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
            Either3::Second(()) => {}
            Either3::Third(ControlCommand::Stop) => break,
        }

        while let Some(len) = context.outbound.try_next_into(&mut packet_buf) {
            if let Ok(m) = rns_serial_framing::encode(&packet_buf[..len], &mut frame_buf) {
                let _ = with_timeout(WRITE_TIMEOUT, tx.write_all(&frame_buf[..m])).await;
            }
        }
    }
    context.control.report(ControlReport::Stopped);
}
