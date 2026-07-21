mod connection;

pub use connection::ConnectionState;

use crate::interfaces::InterfaceId;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Independent,
    FleetMember { supervisor_id: InterfaceId },
}

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

#[cfg(feature = "tokio-host")]
pub trait ReportsStatus {
    fn status_view(&self) -> Option<StatusView> {
        None
    }

    fn connection_view(&self) -> Option<ConnectionView> {
        None
    }
}

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
