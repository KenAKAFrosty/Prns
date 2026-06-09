//! The live counterpart to [`InterfaceConfig`](super::InterfaceConfig): where the config is how
//! an interface *is* (its static capabilities, mode, medium), this is how it is *doing* right
//! now. The application reads it directly, never through the engine — the interface owns this
//! state (it touches the wire), and the app pulls it on its own render cadence through a
//! cheap-clone handle. Each host impls the handle behind this trait (atomics on std, …); the
//! app's render code reads only the trait, identical across every platform.
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
