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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AirtimeUtilization {
    pub short_per_mille: u16,
    pub long_per_mille: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferRates {
    pub rx_bps: u32,
    pub tx_bps: u32,
}

pub trait InterfaceStatus {
    fn id(&self) -> InterfaceId;
    fn connection(&self) -> ConnectionState;
    fn rx_bytes(&self) -> u64;
    fn tx_bytes(&self) -> u64;
    /// `None` until the interface publishes — a link with no declared bitrate never does.
    fn airtime(&self) -> Option<AirtimeUtilization> {
        None
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        None
    }
}

/// Read a status through a shared reference, so a renderer can feed `&[&Status]` to the
/// card builder when the handle itself can't be cloned (the no_std `&'static` handle a board
/// shares between its interface and display tasks), not only the std `Arc`-clone case.
impl<T: InterfaceStatus + ?Sized> InterfaceStatus for &T {
    fn id(&self) -> InterfaceId {
        (**self).id()
    }

    fn connection(&self) -> ConnectionState {
        (**self).connection()
    }

    fn rx_bytes(&self) -> u64 {
        (**self).rx_bytes()
    }

    fn tx_bytes(&self) -> u64 {
        (**self).tx_bytes()
    }

    fn airtime(&self) -> Option<AirtimeUtilization> {
        (**self).airtime()
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        (**self).transfer_rates()
    }
}
