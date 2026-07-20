use crate::interfaces::rns_serial_framing;
use crate::interfaces::{EMBEDDED_MAX_WIRE_FRAME_LEN, MAX_WIRE_FRAME_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpWireFraming {
    Hdlc,
    Kiss,
}

/// One socket read's worth. The reference uses 4 KiB, which is fine for slow links
/// but makes local-gigabit resource frames trickle through hundreds of userspace
/// reads. Keep this TCP-only; serial's read buffer stays sized for its byte stream.
/// A TCP read can now absorb one worst-case encoded engine frame.
pub const READ_BUF_LEN: usize = FRAMED_LEN;

/// Capacity, not a claim: the largest IFAC'd frame the engine can emit or accept, so the
/// serve loop's buffers carry any MTU the descriptor below can declare.
pub const FRAME_CAP: usize = MAX_WIRE_FRAME_LEN;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(FRAME_CAP);
pub const KISS_FRAMED_LEN: usize = crate::interfaces::kiss_framing::max_encoded_len(FRAME_CAP);

/// The embedded twins of [`FRAME_CAP`]/[`FRAMED_LEN`]/[`READ_BUF_LEN`]: an embassy TCP client
/// sizes its decoder, frame, and read buffers to the board's embedded wire ceiling
/// ([`EMBEDDED_MAX_WIRE_FRAME_LEN`]),
/// never the host's absolute one — the same host-vs-embedded split the reactor lanes draw, so a
/// no-heap board never inlines the giga ceiling into a socket buffer.
pub const EMBEDDED_FRAME_CAP: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
pub const EMBEDDED_FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(EMBEDDED_FRAME_CAP);
/// One socket read's worth on embedded — a chunk, not a whole frame: the decoder reassembles across
/// reads, so this trades a few extra reads for DRAM the board would rather keep for its stack.
pub const EMBEDDED_READ_BUF_LEN: usize = 1_024;
