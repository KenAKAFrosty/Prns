use super::handshake::{Control, L2capPlan, LinkCapabilities, PeerProtocol};
use super::identity::{BleAddress, BleIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertisingMode {
    On,
    Off,
}

impl AdvertisingMode {
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanningMode {
    On,
    Off,
}

impl ScanningMode {
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioMode {
    On,
    Off,
}

impl RadioMode {
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

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
    DialFailed {
        address: BleAddress,
    },
}

#[allow(async_fn_in_trait)]
pub trait BleBackend {
    const MAX_PEERS: usize;

    type Error: core::fmt::Debug;
    type Link: BleLink<Error = Self::Error>;

    /// Why this backend cannot safely run (e.g. a host policy that would prompt every nearby peer),
    /// or `None` to start normally. The supervisor surfaces a blocked backend as a `Failed` interface
    /// instead of bringing the radio up; most backends never block and keep the default.
    fn blocked(&self) -> Option<&'static str> {
        None
    }

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Self::Error>;
    async fn set_scanning(&mut self, _mode: ScanningMode) -> Result<(), Self::Error> {
        Ok(())
    }
    async fn set_radio_mode(&mut self, _mode: RadioMode) -> Result<(), Self::Error> {
        Ok(())
    }
    async fn local_capabilities(
        &mut self,
        configured: LinkCapabilities,
    ) -> Result<LinkCapabilities, Self::Error> {
        Ok(configured)
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

    fn peer_protocol(&self) -> PeerProtocol;
    fn address(&self) -> BleAddress;

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, Self::Error> {
        core::future::pending().await
    }

    async fn send_columba_identity(&mut self, _identity: BleIdentity) -> Result<(), Self::Error> {
        Ok(())
    }

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
