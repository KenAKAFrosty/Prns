//! Wire contract boundary.
//!
//! Packet byte layouts belong here. Internal engine state should use typed
//! values and cross this boundary exactly once when decoding or encoding wire
//! bytes.

/// Error returned when bytes do not satisfy a wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    BufferTooShort,
    InvalidField,
}
