use crate::interfaces::rns_serial_framing;
use crate::interfaces::{EMBEDDED_MAX_WIRE_FRAME_LEN, MAX_WIRE_FRAME_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpWireFraming {
    Hdlc,
    Kiss,
}

/// A TCP read absorbs one worst-case encoded engine frame; serial retains its smaller byte-stream buffer.
pub const READ_BUF_LEN: usize = FRAMED_LEN;

pub const FRAME_CAP: usize = MAX_WIRE_FRAME_LEN;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(FRAME_CAP);
pub const KISS_FRAMED_LEN: usize = crate::interfaces::kiss_framing::max_encoded_len(FRAME_CAP);

/// Embassy buffers use the embedded wire ceiling so a no-heap board never inlines the host ceiling into a socket buffer.
pub const EMBEDDED_FRAME_CAP: usize = EMBEDDED_MAX_WIRE_FRAME_LEN;
pub const EMBEDDED_FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(EMBEDDED_FRAME_CAP);
/// The embedded decoder reassembles across reads, trading extra reads for stack DRAM.
pub const EMBEDDED_READ_BUF_LEN: usize = 1_024;
