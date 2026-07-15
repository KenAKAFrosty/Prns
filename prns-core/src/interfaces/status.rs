//! The live counterpart to [`InterfaceDescriptor`](super::InterfaceDescriptor): where the descriptor is how
//! an interface *is*, this is how it is *doing* right now. The interface owns this state (it
//! touches the wire) and the app pulls it directly on its own render cadence through a
//! cheap-clone handle, never through the engine. Engine state (route and link counts) stays
//! separate: the runtime joins it with these vitals to mint an [`InterfaceSnapshot`] that a
//! face renders. Each host impls the handle behind this trait; the app reads only the trait.

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
    fn failure_reason(&self) -> Option<&'static str> {
        None
    }
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

/// Where an interface sits in the runtime's topology, recorded at attach so a face can fold a
/// supervisor's fleet members under it instead of showing every peer at the root.
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
    pub failure_reason: Option<&'static str>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub transfer_rates: Option<TransferRates>,
}

impl InterfaceVitals {
    pub fn of(status: &impl InterfaceStatus) -> Self {
        Self {
            id: status.id(),
            connection: status.connection(),
            failure_reason: status.failure_reason(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
        }
    }
}

impl From<InterfaceSnapshot> for InterfaceVitals {
    fn from(snapshot: InterfaceSnapshot) -> Self {
        Self {
            id: snapshot.id,
            connection: snapshot.connection,
            failure_reason: snapshot.failure_reason,
            rx_bytes: snapshot.rx_bytes,
            tx_bytes: snapshot.tx_bytes,
            transfer_rates: snapshot.transfer_rates,
        }
    }
}

/// One interface's complete live view: the [`InterfaceVitals`] it owns, the engine counts that
/// ride over it, and where it sits in the fleet. Zero counts are a valid state (an idle
/// interface, or a face with no engine), not an accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSnapshot {
    pub id: InterfaceId,
    pub connection: ConnectionState,
    pub failure_reason: Option<&'static str>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub transfer_rates: Option<TransferRates>,
    pub destinations: u32,
    pub links: u32,
    pub transported_links: u32,
    pub membership: Membership,
}

/// A live view yielding the current [`InterfaceVitals`] on each call: a closure over the
/// interface's cheap-clone status handle, so it outlives the interface the runtime consumed at attach.
#[cfg(feature = "tokio-host")]
pub type StatusView = std::sync::Arc<dyn Fn() -> std::vec::Vec<InterfaceVitals> + Send + Sync>;

#[cfg(feature = "tokio-host")]
#[derive(Clone)]
pub struct ConnectionView {
    read: std::sync::Arc<dyn Fn() -> ConnectionState + Send + Sync>,
}

#[cfg(feature = "tokio-host")]
impl ConnectionView {
    pub fn of<S>(status: S) -> Self
    where
        S: InterfaceStatus + Send + Sync + 'static,
    {
        Self {
            read: std::sync::Arc::new(move || status.connection()),
        }
    }

    pub fn connection(&self) -> ConnectionState {
        (self.read)()
    }
}

/// What a host interface (or supervisor) hands the runtime for central status tracking: a
/// [`StatusView`] over its own handle, or `None` for a type that owns no live status. The
/// runtime stores one per attached interface, so a capability like the shared-instance control RPC reads the whole fleet.
#[cfg(feature = "tokio-host")]
pub trait ReportsStatus {
    fn status_view(&self) -> Option<StatusView> {
        None
    }

    fn connection_view(&self) -> Option<ConnectionView> {
        None
    }
}

/// Read a status through a shared reference, so a renderer can feed `&[&Status]` to the card
/// builder when the handle itself can't be cloned (the no_std `&'static` board handle), not only the std `Arc` case.
impl<T: InterfaceStatus + ?Sized> InterfaceStatus for &T {
    fn id(&self) -> InterfaceId {
        (**self).id()
    }

    fn connection(&self) -> ConnectionState {
        (**self).connection()
    }

    fn failure_reason(&self) -> Option<&'static str> {
        (**self).failure_reason()
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
