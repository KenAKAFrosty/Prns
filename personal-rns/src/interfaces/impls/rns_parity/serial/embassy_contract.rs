//! The embassy serial worker on the contract seam — the embassy twin of the std
//! shell's `serve_until_stopped` ([`std_host`](super::std_host)). Runs the RNS
//! `SerialInterface` over any async byte stream (the ESP32 usb-serial-jtag halves,
//! a UART, a test pipe), meeting the runtime through the three-lane seam.
//!
//! Gated on the lighter `embassy-contract` feature — no `embassy-net`/LoRa — so a
//! USB-only board (ESP32-C6) pulls none of the radio stack.

use embassy_futures::select::{select3, Either3};
use embassy_time::{with_timeout, Duration};
use embedded_io_async::{Read, Write};

use super::core::SERIAL_MTU;
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceWorkerContext,
};
use crate::runtime::channels::embassy_seam::EmbassyHostSubstrate;

/// Upper bound on one frame's write. If nothing is draining the CDC (no USB host
/// attached, or no peer reading), an unbounded write would wedge the loop forever;
/// on timeout we drop the frame — RNS re-announces, so it self-heals.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

/// Drive the serial link over the contract seam. De-frame inbound bytes off `rx` and
/// [`submit`](InboundSink::submit) each Reticulum packet into the interface's inbound
/// ring; drain the outbound the runtime queued, frame each, and write it to `tx`;
/// wind down on [`Stop`](ControlCommand::Stop), reporting
/// [`Stopped`](ControlReport::Stopped).
///
/// The loop `select`s on three wakes — inbound bytes, outbound readiness, a control
/// command — so it parks until something genuinely needs it (no keepalive ticker, no
/// idle spin). Generic over the byte stream (any [`embedded_io_async`] transport) and
/// the seam `DEPTH`. Pre-frame noise — e.g. a board sharing this CDC between log text
/// and frames — is skipped by the decoder until a `FLAG`, exactly as stock RNS does.
///
/// Unlike the legacy `run`, there is no `link_up` / keepalive: liveness rides a
/// control report under the new contract (deferred), so this loop only moves bytes.
/// The write is still [`WRITE_TIMEOUT`]-bounded so a wire with no reader can't wedge
/// the loop; a dropped frame self-heals.
pub async fn serve<R, W, const DEPTH: usize>(
    mut rx: R,
    mut tx: W,
    mut context: InterfaceWorkerContext<EmbassyHostSubstrate<SERIAL_MTU, DEPTH>>,
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
            // Inbound bytes: feed the decoder; each closed non-empty frame is a
            // Reticulum packet → submit it (the sink stamps arrival + wakes the host).
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
            // Outbound queued — fall through to the drain below.
            Either3::Second(()) => {}
            // Wind down on stop.
            Either3::Third(ControlCommand::Stop) => break,
        }

        // Drain whatever the runtime queued (also opportunistic after inbound): pull
        // each packet, frame it, write it. `try_next_into` copies out so the write can
        // cross an `.await`.
        while let Some(len) = context.outbound.try_next_into(&mut packet_buf) {
            if let Ok(m) = rns_serial_framing::encode(&packet_buf[..len], &mut frame_buf) {
                let _ = with_timeout(WRITE_TIMEOUT, tx.write_all(&frame_buf[..m])).await;
            }
        }
    }
    context.control.report(ControlReport::Stopped);
}
