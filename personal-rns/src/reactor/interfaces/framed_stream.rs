//! The serve loop every framed byte-stream interface shares under tokio: serial and TCP
//! speak the same RNS HDLC-style framing over an async byte stream, differing only in how
//! they open that stream and how large their frames run — so the read-deframe-up /
//! drain-frame-down loop lives here once, const-parameterized by those sizes. An interface
//! body calls [`serve`] per connection; a fresh decoder per call discards any half-frame
//! an earlier drop interrupted.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::engine::InstantMillis;
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::InterfaceSeam;
use crate::reactor::throughput::ThroughputLedger;

/// Serve one connection until the stream drops: read bytes and deframe them up to the seam,
/// drain the seam and frame outbound onto the wire. Returns on any IO error so the caller
/// can reconnect.
pub async fn serve<
    const READ_LEN: usize,
    const FRAME_CAP: usize,
    const FRAMED_LEN: usize,
    S,
    Seam,
>(
    mut stream: S,
    seam: &mut Seam,
    status: &TokioInterfaceStatus,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    bitrate_bps: Option<u32>,
    started: tokio::time::Instant,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let mut decoder = RnsSerialDecoder::<FRAME_CAP>::new();
    let mut read_buf = [0u8; READ_LEN];
    let mut frame_buf = [0u8; FRAMED_LEN];

    loop {
        tokio::select! {
            read = stream.read(&mut read_buf) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                status.add_rx(read as u64);
                let now = InstantMillis(started.elapsed().as_millis() as u64);
                throughput.record_rx(now, read as u64);
                status.set_transfer_rates(throughput.rates(now));
                for &byte in &read_buf[..read] {
                    if let Ok(Some(frame)) = decoder.feed(byte) {
                        if !frame.is_empty() {
                            seam.next_inbound(frame).await;
                        }
                    }
                }
            }
            outbound = seam.next_outbound() => {
                if let Ok(framed) = rns_serial_framing::encode(outbound, &mut frame_buf) {
                    if stream.write_all(&frame_buf[..framed]).await.is_err() {
                        return;
                    }
                    status.add_tx(framed as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, framed as u64);
                    status.set_transfer_rates(throughput.rates(now));
                    if let Some(bitrate_bps) = bitrate_bps {
                        let frame_airtime = frame_airtime_us(framed, bitrate_bps);
                        status.set_airtime(airtime.record_tx(now, frame_airtime));
                    }
                }
            }
        }
    }
}
