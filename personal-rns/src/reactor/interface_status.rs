//! What the application reads about an interface — directly, never through the engine. The
//! interface owns this state (it touches the wire), and the app pulls it on its own render
//! cadence through a cheap-clone handle. Each host impls the handle behind this trait (atomics
//! on std, …); the app's render code reads only the trait, identical across every platform.
//!
//! Today it carries the two live facts the interface knows first-hand — its connection and the
//! bytes it has moved. Route counts (engine state) and rate / last-activity / link counts are
//! separate, later additions; the UI dummies them until then.

use crate::interfaces::{ConnectionState, InterfaceId};

pub trait InterfaceStatus {
    fn id(&self) -> InterfaceId;
    fn connection(&self) -> ConnectionState;
    fn rx_bytes(&self) -> u64;
    fn tx_bytes(&self) -> u64;
}

/// Encode a [`ConnectionState`] into the `u8` a lock-free status handle stores in an atomic.
/// Paired exhaustively with [`decode_connection`] so the two can never drift.
#[cfg(feature = "tokio-host")]
pub(crate) fn encode_connection(connection: ConnectionState) -> u8 {
    match connection {
        ConnectionState::Initializing => 0,
        ConnectionState::Connected => 1,
        ConnectionState::Degraded => 2,
        ConnectionState::Reconnecting => 3,
        ConnectionState::Failed => 4,
        ConnectionState::Disconnected => 5,
    }
}

/// Decode the `u8` from a status handle's atomic back into a [`ConnectionState`].
#[cfg(feature = "tokio-host")]
pub(crate) fn decode_connection(code: u8) -> ConnectionState {
    match code {
        1 => ConnectionState::Connected,
        2 => ConnectionState::Degraded,
        3 => ConnectionState::Reconnecting,
        4 => ConnectionState::Failed,
        5 => ConnectionState::Disconnected,
        _ => ConnectionState::Initializing,
    }
}
