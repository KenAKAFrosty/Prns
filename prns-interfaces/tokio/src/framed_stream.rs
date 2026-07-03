//! The serve loop every framed byte-stream interface shares under tokio: read a wire, deframe
//! up to the seam, frame the seam's outbound back down, generic over a [`Framing`] codec.
//! Serial, TCP, and the shared-instance link pass [`HdlcFraming`]; KISS and AX.25 pass
//! [`KissFraming`]. An interface owns a [`FramedBuffers`] and lends it to [`serve`] per
//! connection, reused across reconnects and never re-allocated; `serve` resets the deframer on
//! entry to discard any half-frame an earlier drop left mid-buffer.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use prns_core::engine::InstantMillis;
#[cfg(any(feature = "kiss", feature = "ax25"))]
use prns_core::interfaces::kiss_framing::{self, KissDecoder};
#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use prns_core::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use prns_core::reactor::interface_seam::InterfaceSeam;
use prns_core::reactor::throughput::ThroughputLedger;
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;

/// A streaming deframer [`serve`] drives over one connection: built fresh, reset between
/// connections, then fed wire bytes a chunk at a time and asked for the next decoded frame. Each
/// framing's decoder implements it, so the serve loop names only this contract, not a concrete
/// decoder.
pub trait StreamDeframer {
    fn new() -> Self;
    fn reset(&mut self);
    /// The next complete frame at or after `*offset` in `input`, advancing `offset` past the bytes
    /// consumed. `None` when the chunk is exhausted with no further frame — or when a malformed or
    /// oversized frame was swallowed (the decoder self-heals and realigns at the next delimiter),
    /// in which case `offset` has still advanced so the caller's loop makes progress.
    fn next_frame<'a>(&'a mut self, input: &[u8], offset: &mut usize) -> Option<&'a [u8]>;
}

/// The wire framing a byte-stream interface speaks — a decoder paired with its encoder, named by a
/// zero-sized marker. Parameterized by the frame ceiling so the decoder's buffer sizes to it; the
/// encoder is a stateless associated function.
pub trait Framing<const FRAME_CAP: usize> {
    type Deframer: StreamDeframer;
    /// Frame `input` into `output`, returning the encoded length, or `None` if `output` is too
    /// small (the caller sizes `output` to the framing's worst case, so this does not happen in
    /// practice; a too-small buffer just drops the frame rather than panicking).
    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize>;
}

/// RNS HDLC-like serial framing (`0x7E` flag, `0x7D` escape) — what serial, TCP, and the
/// shared-instance link speak.
#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
pub struct HdlcFraming;

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
impl<const FRAME_CAP: usize> StreamDeframer for RnsSerialDecoder<FRAME_CAP> {
    fn new() -> Self {
        RnsSerialDecoder::new()
    }

    fn reset(&mut self) {
        RnsSerialDecoder::reset(self);
    }

    fn next_frame<'a>(&'a mut self, input: &[u8], offset: &mut usize) -> Option<&'a [u8]> {
        self.feed_slice_next(input, offset).ok().flatten()
    }
}

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
impl<const FRAME_CAP: usize> Framing<FRAME_CAP> for HdlcFraming {
    type Deframer = RnsSerialDecoder<FRAME_CAP>;

    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize> {
        rns_serial_framing::encode(input, output).ok()
    }
}

/// KISS TNC framing (`0xC0` FEND) — what the KISS and AX.25 interfaces speak.
#[cfg(any(feature = "kiss", feature = "ax25"))]
pub struct KissFraming;

#[cfg(any(feature = "kiss", feature = "ax25"))]
impl<const FRAME_CAP: usize> StreamDeframer for KissDecoder<FRAME_CAP> {
    fn new() -> Self {
        KissDecoder::new()
    }

    fn reset(&mut self) {
        KissDecoder::reset(self);
    }

    fn next_frame<'a>(&'a mut self, input: &[u8], offset: &mut usize) -> Option<&'a [u8]> {
        self.feed_slice_next(input, offset).ok().flatten()
    }
}

#[cfg(any(feature = "kiss", feature = "ax25"))]
impl<const FRAME_CAP: usize> Framing<FRAME_CAP> for KissFraming {
    type Deframer = KissDecoder<FRAME_CAP>;

    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize> {
        kiss_framing::encode(input, output).ok()
    }
}

/// The reusable scratch a framed serve loop works in: the decoder and the read and
/// outbound-frame buffers, heap-held so no megabyte of buffer ever rides the stack. An
/// interface lends one to [`serve`] per connection (allocated once across reconnects; a target
/// that never answers, holding one behind an `Option`, never allocates at all).
pub struct FramedBuffers<F, const READ_LEN: usize, const FRAME_CAP: usize, const FRAMED_LEN: usize>
where
    F: Framing<FRAME_CAP>,
{
    deframer: std::boxed::Box<F::Deframer>,
    read_buf: std::boxed::Box<[u8]>,
    frame_buf: std::boxed::Box<[u8]>,
}

impl<F, const READ_LEN: usize, const FRAME_CAP: usize, const FRAMED_LEN: usize> Default
    for FramedBuffers<F, READ_LEN, FRAME_CAP, FRAMED_LEN>
where
    F: Framing<FRAME_CAP>,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<F, const READ_LEN: usize, const FRAME_CAP: usize, const FRAMED_LEN: usize>
    FramedBuffers<F, READ_LEN, FRAME_CAP, FRAMED_LEN>
where
    F: Framing<FRAME_CAP>,
{
    pub fn new() -> Self {
        Self {
            deframer: std::boxed::Box::new(<F::Deframer as StreamDeframer>::new()),
            read_buf: std::vec![0u8; READ_LEN].into_boxed_slice(),
            frame_buf: std::vec![0u8; FRAMED_LEN].into_boxed_slice(),
        }
    }
}

/// Serve one connection until the stream drops: read bytes and deframe them up to the seam,
/// drain the seam and frame outbound onto the wire. Returns on any IO error so the caller
/// can reconnect. The deframer and buffers are the caller's [`FramedBuffers`], reset on entry and
/// reused across reconnects. The framing `F` is the same the buffers were minted with.
/// The per-connection accounting one served stream reports into: the shared
/// status handle, the airtime and throughput ledgers, the nominal bitrate that
/// prices a frame's airtime (`None` on media without one), and the serve epoch
/// the ledgers' clocks count from.
pub struct WireMeters<'a> {
    pub status: &'a TokioInterfaceStatus,
    pub airtime: &'a mut AirtimeLedger,
    pub throughput: &'a mut ThroughputLedger,
    pub bitrate_bps: Option<u32>,
    pub started: tokio::time::Instant,
}

pub async fn serve<
    F,
    const READ_LEN: usize,
    const FRAME_CAP: usize,
    const FRAMED_LEN: usize,
    S,
    Seam,
>(
    mut stream: S,
    buffers: &mut FramedBuffers<F, READ_LEN, FRAME_CAP, FRAMED_LEN>,
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
) where
    F: Framing<FRAME_CAP>,
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let FramedBuffers {
        deframer,
        read_buf,
        frame_buf,
    } = buffers;
    let WireMeters {
        status,
        airtime,
        throughput,
        bitrate_bps,
        started,
    } = meters;
    let (bitrate_bps, started) = (*bitrate_bps, *started);
    deframer.reset();
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
                    if let Some(frame) = deframer.next_frame(chunk, &mut offset) {
                        if !frame.is_empty() {
                            seam.next_inbound(frame).await;
                        }
                    }
                }
            }
            outbound = seam.next_outbound() => {
                if let Some(framed) = F::encode(outbound, &mut *frame_buf) {
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
