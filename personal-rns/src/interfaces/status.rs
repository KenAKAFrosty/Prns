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

    /// The number of live Reticulum links carried over this interface, `0` until a source publishes
    /// it. A supervisor's aggregate handle leaves this at the default — its members each report their
    /// own; fleet size is read from the per-member cards, not conflated into a link count.
    fn links(&self) -> u32 {
        0
    }
}

/// An owned, point-in-time read of an [`InterfaceStatus`] — the live facts copied out so a consumer
/// can hold a `Vec` of them past the borrow of the handle they came from. The shared-instance RPC's
/// `interface_stats` collects these from the handles the app holds (the same ones the display reads),
/// then renders them to a stock client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSnapshot {
    pub id: InterfaceId,
    pub connection: ConnectionState,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub transfer_rates: Option<TransferRates>,
    pub links: u32,
}

impl InterfaceSnapshot {
    pub fn of(status: &impl InterfaceStatus) -> Self {
        Self {
            id: status.id(),
            connection: status.connection(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
            links: status.links(),
        }
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

    fn links(&self) -> u32 {
        (**self).links()
    }
}
