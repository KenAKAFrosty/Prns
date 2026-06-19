//! The serve loop every framed byte-stream interface shares under tokio: serial and TCP
//! speak the same RNS HDLC-style framing over an async byte stream, differing only in how
//! they open that stream and how large their frames run — so the read-deframe-up /
//! drain-frame-down loop lives here once, const-parameterized by those sizes. An interface
//! owns a [`FramedBuffers`] and lends it to [`serve`] per connection — reused across reconnects,
//! never re-allocated — and `serve` resets the decoder on entry to discard any half-frame an
//! earlier drop left mid-buffer.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::engine::InstantMillis;
use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::InterfaceSeam;
use crate::reactor::throughput::ThroughputLedger;

/// The reusable scratch a framed serve loop works in: the deframing decoder and the read and
/// outbound-frame buffers, sized to the interface's frame ceiling and heap-held so no megabyte of
/// buffer ever rides the stack. An interface owns one and lends it to [`serve`] per connection, so a
/// reconnecting link allocates these once and reuses them across reconnects rather than once per
/// connection — and a target that never answers, holding one behind an `Option`, never allocates at
/// all. [`serve`] resets the decoder on entry, discarding any half-frame an earlier drop left mid-buffer.
pub struct FramedBuffers<const READ_LEN: usize, const FRAME_CAP: usize, const FRAMED_LEN: usize> {
    decoder: std::boxed::Box<RnsSerialDecoder<FRAME_CAP>>,
    read_buf: std::boxed::Box<[u8]>,
    frame_buf: std::boxed::Box<[u8]>,
}

impl<const READ_LEN: usize, const FRAME_CAP: usize, const FRAMED_LEN: usize> Default
    for FramedBuffers<READ_LEN, FRAME_CAP, FRAMED_LEN>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const READ_LEN: usize, const FRAME_CAP: usize, const FRAMED_LEN: usize>
    FramedBuffers<READ_LEN, FRAME_CAP, FRAMED_LEN>
{
    pub fn new() -> Self {
        Self {
            decoder: std::boxed::Box::new(RnsSerialDecoder::new()),
            read_buf: std::vec![0u8; READ_LEN].into_boxed_slice(),
            frame_buf: std::vec![0u8; FRAMED_LEN].into_boxed_slice(),
        }
    }
}

/// Serve one connection until the stream drops: read bytes and deframe them up to the seam,
/// drain the seam and frame outbound onto the wire. Returns on any IO error so the caller
/// can reconnect. The decoder and buffers are the caller's [`FramedBuffers`], reset on entry and
/// reused across reconnects.
pub async fn serve<
    const READ_LEN: usize,
    const FRAME_CAP: usize,
    const FRAMED_LEN: usize,
    S,
    Seam,
>(
    mut stream: S,
    buffers: &mut FramedBuffers<READ_LEN, FRAME_CAP, FRAMED_LEN>,
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
    let FramedBuffers {
        decoder,
        read_buf,
        frame_buf,
    } = buffers;
    decoder.reset();
    let read_buf: &mut [u8] = read_buf;
    let frame_buf: &mut [u8] = frame_buf;

    loop {
        tokio::select! {
            read = stream.read(&mut *read_buf) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                status.add_rx(read as u64);
                let now = InstantMillis(started.elapsed().as_millis() as u64);
                throughput.record_rx(now, read as u64);
                status.set_transfer_rates(throughput.rates());
                let mut offset = 0;
                let chunk = &read_buf[..read];
                while offset < chunk.len() {
                    if let Ok(Some(frame)) = decoder.feed_slice_next(chunk, &mut offset) {
                        if !frame.is_empty() {
                            seam.next_inbound(frame).await;
                        }
                    }
                }
            }
            outbound = seam.next_outbound() => {
                if let Ok(framed) = rns_serial_framing::encode(outbound, &mut *frame_buf) {
                    if stream.write_all(&frame_buf[..framed]).await.is_err() {
                        return;
                    }
                    status.add_tx(framed as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, framed as u64);
                    status.set_transfer_rates(throughput.rates());
                    if let Some(bitrate_bps) = bitrate_bps {
                        let frame_airtime = frame_airtime_us(framed, bitrate_bps);
                        status.set_airtime(airtime.record_tx(now, frame_airtime));
                    }
                }
            }
        }
    }
}
