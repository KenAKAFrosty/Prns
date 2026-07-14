//! RNS 1.3.5 Resource bz2, the half the pure engine leaves to its host. `prns-core`
//! carries a resource as an opaque stream and flags it compressed or not; the codec is
//! the host's, because bz2 is unavailable on the embedded targets the core also serves.
//! The tokio host has no such limit, so it compresses every outgoing resource and inflates
//! every compressed one, and a std node speaks the reference's wire compression byte-for-byte.
//!
//! Pure-Rust bz2 (bzip2's default `libbz2-rs-sys` backend): no C toolchain, and nothing
//! for the crate's `forbid(unsafe_code)` to trip on.

use std::vec::Vec;

use bzip2::{Compress, Compression, Decompress, Status};

/// RNS 1.3.5 `Resource.__init__`: `bz2.compress` at level 9 (its default), kept only when
/// it comes out strictly smaller than the input. `None` is the reference's else-branch: send
/// the payload as-is with the `c` flag clear. For already-dense bytes bz2 only adds overhead,
/// so the reference, and we, decline it.
///
/// The reference pays the full attempt on every input; on dense data that is where a bulk
/// sender's whole core goes (~80 ms per 1 MiB segment against ~3 ms of engine work), so past
/// [`SAMPLE_GATE_LEN`] we first compress a head/middle/tail sample and decline outright when
/// even the sample refuses to shrink. A kept stream is still the whole-input level-9 attempt,
/// byte-identical to the reference's; the sample only buys the decline early. The corner this
/// trades away: a large payload whose only compressible run hides between the sample points
/// ships uncompressed — wire-legal, just larger than the reference would have sent it.
///
/// The corner has a proven radius, because the input is bounded by the segment size and the
/// windows sit at both ends and the midpoint: the largest unsampled gap is
/// `(len − 3·SAMPLE_SLICE_LEN) / 2`, so any contiguous compressible region longer than a gap
/// plus two windows must contain a whole window and is always detected. Only redundancy
/// entirely below that length — under half the payload — can hide.
#[must_use]
pub fn compress_if_smaller(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() >= SAMPLE_GATE_LEN && !sample_shrinks(data) {
        return None;
    }
    bz2_if_smaller(data)
}

/// Sampling pays ~5 ms to sidestep an ~80 ms attempt at 1 MiB; below this the full attempt
/// is cheap enough to just run, and every input stays on the reference's exact path.
pub const SAMPLE_GATE_LEN: usize = 256 * 1024;

const SAMPLE_SLICE_LEN: usize = 16 * 1024;

fn sample_shrinks(data: &[u8]) -> bool {
    let mut sample = Vec::with_capacity(3 * SAMPLE_SLICE_LEN);
    sample.extend_from_slice(&data[..SAMPLE_SLICE_LEN]);
    sample.extend_from_slice(&data[(data.len() - SAMPLE_SLICE_LEN) / 2..][..SAMPLE_SLICE_LEN]);
    sample.extend_from_slice(&data[data.len() - SAMPLE_SLICE_LEN..]);
    bz2_if_smaller(&sample).is_some()
}

fn bz2_if_smaller(data: &[u8]) -> Option<Vec<u8>> {
    let mut compressor = Compress::new(Compression::best(), 0);
    let mut compressed = Vec::with_capacity(bz2_worst_case_len(data.len()));
    match compressor.compress_vec(data, &mut compressed, bzip2::Action::Finish) {
        Ok(Status::StreamEnd) => (compressed.len() < data.len()).then_some(compressed),
        _ => None,
    }
}

/// libbz2's guaranteed output ceiling, `len + len/100 + 600`: enough spare capacity that
/// even incompressible input (which bz2 grows) finishes in one pass, so the keep-smaller
/// test below is a true size comparison, never an artifact of a filled buffer.
fn bz2_worst_case_len(len: usize) -> usize {
    len + len / 100 + 600
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecompressError {
    Malformed,
    Overlong,
}

const DECOMPRESS_CHUNK_LEN: usize = 64 * 1024;

/// RNS 1.3.5's bounded bz2 inflate, `BZ2Decompressor(...).decompress(data, max_length=…)` with
/// the `eof` check: inflate to at most `max_len`, refusing a stream that would run past it. Both
/// callers cap `max_len` at host policy (a resource's advertised length, already gated by the
/// link's `ResourceStrategy`; a stream chunk's channel MDU), so a bz2 bomb can force neither an
/// unbounded allocation nor an unbounded inflate. A resource caller passes its exact advertised
/// length and re-checks it on assembly, so a stream that inflates short is caught there.
pub fn decompress_bounded(stream: &[u8], max_len: u64) -> Result<Vec<u8>, DecompressError> {
    let cap = usize::try_from(max_len).map_err(|_| DecompressError::Overlong)?;
    let mut out = Vec::with_capacity(cap.min(DECOMPRESS_CHUNK_LEN));
    let mut chunk = std::vec![0u8; DECOMPRESS_CHUNK_LEN];
    let mut decoder = Decompress::new(false);
    let mut input_at = 0usize;
    loop {
        let remaining = cap.saturating_sub(out.len());
        let offered = remaining.saturating_add(1).min(DECOMPRESS_CHUNK_LEN);
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let input = stream.get(input_at..).ok_or(DecompressError::Malformed)?;
        let output = chunk.get_mut(..offered).ok_or(DecompressError::Overlong)?;
        let status = decoder
            .decompress(input, output)
            .map_err(|_| DecompressError::Malformed)?;
        let consumed = usize::try_from(decoder.total_in().saturating_sub(before_in))
            .map_err(|_| DecompressError::Malformed)?;
        let produced = usize::try_from(decoder.total_out().saturating_sub(before_out))
            .map_err(|_| DecompressError::Overlong)?;
        input_at = input_at
            .checked_add(consumed)
            .ok_or(DecompressError::Malformed)?;
        if input_at > stream.len() {
            return Err(DecompressError::Malformed);
        }
        if produced > remaining || produced > offered {
            return Err(DecompressError::Overlong);
        }
        let produced_bytes = chunk.get(..produced).ok_or(DecompressError::Overlong)?;
        out.extend_from_slice(produced_bytes);
        if status == Status::StreamEnd {
            return Ok(out);
        }
        if consumed == 0 && produced == 0 {
            return Err(DecompressError::Malformed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// The exact input behind the resource family's `CASE1_BZ2` reference vector:
    /// RNS 1.3.5 compresses this 1360-byte payload to the 90-byte stream below.
    fn reference_input() -> Vec<u8> {
        b"reticulum resources ride the link "
            .iter()
            .copied()
            .cycle()
            .take(34 * 40)
            .collect()
    }

    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

    #[test]
    fn our_bz2_is_byte_identical_to_the_reference_compressor() {
        assert_eq!(
            compress_if_smaller(&reference_input()),
            Some(bytes_from_hex(CASE1_BZ2)),
        );
    }

    #[test]
    fn the_reference_stream_inflates_back_to_its_input() {
        let input = reference_input();
        let inflated = decompress_bounded(&bytes_from_hex(CASE1_BZ2), input.len() as u64);
        assert_eq!(inflated, Ok(input));
    }

    /// Deterministic high-entropy bytes (xorshift64*, all eight output bytes per step —
    /// single low bytes of the raw state correlate enough for bz2's BWT to shrink them):
    /// bz2 cannot shrink these, so they exercise the decline branch and the incompressible
    /// round trip without a real RNG in the test.
    fn xorshift_bytes(len: usize) -> Vec<u8> {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut out = Vec::with_capacity(len + 8);
        while out.len() < len {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            out.extend_from_slice(&x.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes());
        }
        out.truncate(len);
        out
    }

    #[test]
    fn compression_round_trips_compressible_data() {
        let data: Vec<u8> = (0..8192u32).map(|i| (i / 32) as u8).collect();
        let stream = compress_if_smaller(&data).expect("long runs compress");
        assert_eq!(decompress_bounded(&stream, data.len() as u64), Ok(data));
    }

    #[test]
    fn incompressible_data_declines_compression() {
        assert_eq!(compress_if_smaller(&xorshift_bytes(1024)), None);
    }

    #[test]
    fn a_sampled_dense_payload_declines_compression() {
        assert_eq!(compress_if_smaller(&xorshift_bytes(SAMPLE_GATE_LEN)), None);
    }

    /// The doc's coverage radius, executable: with windows at `0`, `(L−w)/2`, and `L−w`, the
    /// largest unsampled gap is `(L−3w)/2`, so a compressible run of `gap + 2·w` bytes contains
    /// a whole window wherever it sits.
    #[test]
    fn a_compressible_run_past_the_coverage_radius_is_detected_at_any_placement() {
        let gap = (SAMPLE_GATE_LEN - 3 * SAMPLE_SLICE_LEN) / 2;
        let run_len = gap + 2 * SAMPLE_SLICE_LEN;
        for start in [
            0,
            1,
            (SAMPLE_GATE_LEN - run_len) / 2,
            SAMPLE_GATE_LEN - run_len,
        ] {
            let mut data = xorshift_bytes(SAMPLE_GATE_LEN);
            data[start..start + run_len].fill(0);
            assert!(
                compress_if_smaller(&data).is_some(),
                "a {run_len}-byte run starting at {start} always overlaps a whole window",
            );
        }
    }

    /// The corner the radius bounds: a run no longer than one gap, placed exactly between two
    /// windows, is invisible to the sample even though the whole input compresses.
    #[test]
    fn a_compressible_run_that_fits_between_the_windows_is_the_traded_corner() {
        let gap = (SAMPLE_GATE_LEN - 3 * SAMPLE_SLICE_LEN) / 2;
        let mut data = xorshift_bytes(SAMPLE_GATE_LEN);
        data[SAMPLE_SLICE_LEN..SAMPLE_SLICE_LEN + gap].fill(0);
        assert!(
            bz2_if_smaller(&data).is_some(),
            "the reference's whole-input attempt would keep this stream",
        );
        assert_eq!(
            compress_if_smaller(&data),
            None,
            "no window sees the run, so the screen declines — the documented wire-size trade",
        );
    }

    #[test]
    fn a_sampled_payload_compressible_only_at_its_tail_still_compresses() {
        let mut data = xorshift_bytes(SAMPLE_GATE_LEN * 2 / 3);
        data.resize(SAMPLE_GATE_LEN, 0);
        let stream = compress_if_smaller(&data).expect("the tail sample shrinks");
        assert_eq!(decompress_bounded(&stream, data.len() as u64), Ok(data));
    }

    #[test]
    fn a_stream_shorter_than_the_bound_inflates_to_its_true_length() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i / 16) as u8).collect();
        let stream = compress_if_smaller(&data).expect("runs compress");
        assert_eq!(
            decompress_bounded(&stream, 1 << 20),
            Ok(data),
            "a chunk inflating well under the ceiling is accepted at its own length",
        );
    }

    #[test]
    fn an_empty_payload_declines_compression() {
        assert_eq!(compress_if_smaller(&[]), None);
    }

    #[test]
    fn a_stream_that_inflates_past_its_bound_is_rejected() {
        let big: Vec<u8> = std::vec![0u8; 64 * 1024];
        let stream = compress_if_smaller(&big).expect("a run of zeros compresses");
        assert_eq!(
            decompress_bounded(&stream, (big.len() - 1) as u64),
            Err(DecompressError::Overlong),
            "one byte short of the true length must not silently truncate",
        );
    }

    #[test]
    fn a_truncated_stream_is_malformed() {
        let data: Vec<u8> = std::vec![7u8; 8192];
        let stream = compress_if_smaller(&data).expect("a run compresses");
        let truncated = &stream[..stream.len() - 4];
        assert!(decompress_bounded(truncated, data.len() as u64).is_err());
    }

    #[test]
    fn garbage_is_malformed_not_a_panic() {
        assert_eq!(
            decompress_bounded(b"not a bz2 stream at all", 64),
            Err(DecompressError::Malformed),
        );
    }

    #[test]
    fn an_unbounded_claim_allocates_only_the_inflated_payload() {
        let input = reference_input();
        assert_eq!(
            decompress_bounded(&bytes_from_hex(CASE1_BZ2), u64::MAX),
            Ok(input),
        );
    }

    #[test]
    fn malformed_input_with_an_unbounded_claim_is_rejected() {
        assert_eq!(
            decompress_bounded(b"not a bz2 stream at all", u64::MAX),
            Err(DecompressError::Malformed),
        );
    }
}
