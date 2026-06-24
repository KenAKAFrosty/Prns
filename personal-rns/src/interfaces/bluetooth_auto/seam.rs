use super::core::{BleAddress, Control, Dialect, L2capPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Dialed,
    Accepted,
}

pub enum BleEvent<L> {
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Inbound(L),
    LinkReady {
        link: L,
        origin: Origin,
        peer_rssi: Option<i8>,
    },
}

#[allow(async_fn_in_trait)]
pub trait BleBackend {
    const MAX_PEERS: usize;

    type Error: core::fmt::Debug;
    type Link: BleLink<Error = Self::Error>;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), Self::Error>;
    /// Scan for peers advertising our service so the supervisor can dial them. The supervisor gates
    /// this on capacity, exactly like advertising. A backend that scans autonomously (or cannot
    /// scan) keeps the default no-op.
    async fn set_scanning(&mut self, _enabled: bool) -> Result<(), Self::Error> {
        Ok(())
    }
    async fn next_event(&mut self) -> BleEvent<Self::Link>;
    async fn dial(&mut self, address: BleAddress);
    async fn on_link_closed(&mut self, _address: BleAddress) {}
}

#[allow(async_fn_in_trait)]
pub trait BleLink {
    type Error: core::fmt::Debug;
    type Source: BleSource<Error = Self::Error>;
    type Sink: BleSink<Error = Self::Error>;

    fn dialect(&self) -> Dialect;
    fn address(&self) -> BleAddress;

    async fn control_send(&mut self, msg: &Control) -> Result<(), Self::Error>;
    async fn control_recv(&mut self) -> Result<Control, Self::Error>;

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Self::Error>;

    fn into_data(self) -> (Self::Source, Self::Sink);
}

#[allow(async_fn_in_trait)]
pub trait BleSource {
    type Error: core::fmt::Debug;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error>;
}

#[allow(async_fn_in_trait)]
pub trait BleSink {
    type Error: core::fmt::Debug;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error>;
}
