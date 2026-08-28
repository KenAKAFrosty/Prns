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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum DialOutcome {
    Started,
    Busy,
    UnknownPeer,
    RadioOff,
    InvariantViolation,
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
pub trait BleBackend<const MAX_PEERS: usize> {
    type Error: core::fmt::Debug;
    type Link: BleLink<Error = Self::Error>;

    /// A blocked backend is reported as a failed interface without bringing up the radio.
    fn blocked(&self) -> Option<&'static str> {
        None
    }

    /// Live local discovery group tag when the backend owns a mutable group (e.g. Android).
    fn local_group_tag(&self) -> Option<[u8; 4]> {
        None
    }

    /// Ask the radio to drop every peer without cycling advertising/scanning.
    ///
    /// Used when the discovery group changes so existing links cannot linger across groups.
    fn drop_all_links(&mut self) {}

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
    async fn dial(&mut self, address: BleAddress) -> DialOutcome;
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
