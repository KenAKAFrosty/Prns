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

/// Where an interface sits in the runtime's topology: standing on its own, or one of a supervisor's
/// fleet. The runtime records this when the interface attaches, so a face can fold a supervisor's
/// members under it instead of showing every peer at the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Independent,
    FleetMember { supervisor_id: InterfaceId },
}

/// The facts an interface owns first-hand because it touches the wire. A status handle yields these;
/// the runtime joins them with the engine's counts and the topology to mint an [`InterfaceSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceVitals {
    pub id: InterfaceId,
    pub connection: ConnectionState,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub transfer_rates: Option<TransferRates>,
}

impl InterfaceVitals {
    pub fn of(status: &impl InterfaceStatus) -> Self {
        Self {
            id: status.id(),
            connection: status.connection(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
        }
    }
}

/// One interface's complete live view: the [`InterfaceVitals`] it owns, the engine counts that ride
/// over it, and where it sits in the fleet. Only the runtime mints one — it alone holds the status
/// handle, the count store, and the topology together — so a snapshot can never carry zeroed counts
/// by accident the way a status-only read could.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSnapshot {
    pub id: InterfaceId,
    pub connection: ConnectionState,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub transfer_rates: Option<TransferRates>,
    pub destinations: u32,
    pub links: u32,
    pub transported_links: u32,
    pub membership: Membership,
}

/// A live view of an interface's status, yielding the current [`InterfaceVitals`] on each call. It
/// is a closure over the interface's cheap-clone status handle, so it outlives the interface the
/// runtime consumed when it attached it.
#[cfg(feature = "tokio-host")]
pub type StatusView = std::sync::Arc<dyn Fn() -> std::vec::Vec<InterfaceVitals> + Send + Sync>;

/// What a host interface (or supervisor) hands the runtime so it can track interface status
/// centrally: a [`StatusView`] over its own status handle, or `None` for a type that owns no live
/// status (a bare supervisor like the local-instance server). The runtime stores one per interface
/// attached through the handle, so a capability such as the shared-instance control RPC reads the
/// whole fleet without the app collecting status handles by hand.
#[cfg(feature = "tokio-host")]
pub trait ReportsStatus {
    fn status_view(&self) -> Option<StatusView> {
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
