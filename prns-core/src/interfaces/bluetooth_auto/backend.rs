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

/// Observed platform radio state, independent of the application's desired [`RadioMode`].
/// Backends without an authoritative observation report [`Self::Unknown`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BluetoothRadioState {
    #[default]
    Unknown,
    Resetting,
    Unsupported,
    Unauthorized,
    PoweredOff,
    PoweredOn,
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
    type Control: BleControl<Error = Self::Error>;

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

    /// Settles this exact physical session without discarding its control owner.
    /// Native links retain controls even with a legacy peer or an L2CAP data plane.
    /// Columba has no native control channel and returns `None`.
    fn into_parts(self) -> BleLinkParts<Self::Source, Self::Sink, Self::Control>;
}

/// Independently owned channels of one settled physical session.
#[must_use]
pub struct BleLinkParts<S, T, C> {
    pub source: S,
    pub sink: T,
    pub control: Option<C>,
}

/// Transport-only access to the existing native control channel.
///
/// Retaining this owner does not negotiate a liveness protocol. Send completion has
/// the backend's existing semantics; queue admission is not proof of remote receipt.
#[allow(async_fn_in_trait)]
pub trait BleControl {
    type Error: core::fmt::Debug;

    /// Keep a started send alive until it completes or the entire session is retired.
    /// Callers must not cancel and retry a possibly submitted control write.
    async fn send(&mut self, message: &Control) -> Result<(), Self::Error>;

    /// Cancellation-safe receive: dropping a pending future must not consume a
    /// message or lose partial parsing state. Embedded supervisors recreate this
    /// future when unrelated work wins their bounded event selection.
    async fn recv(&mut self) -> Result<Control, Self::Error>;
}

/// Uninhabited control type for data-only adapters. Always use `None`, never a
/// placeholder owner whose drop could close the data session.
pub struct NoBleControl<E = core::convert::Infallible> {
    impossible: core::convert::Infallible,
    error: core::marker::PhantomData<fn() -> E>,
}

impl<E: core::fmt::Debug> BleControl for NoBleControl<E> {
    type Error = E;

    async fn send(&mut self, _message: &Control) -> Result<(), E> {
        match self.impossible {}
    }

    async fn recv(&mut self) -> Result<Control, E> {
        match self.impossible {}
    }
}

#[allow(async_fn_in_trait)]
pub trait BleSource {
    type Error: core::fmt::Debug;

    /// Receives a whole wire frame, returning its exact length including interface authentication.
    /// Reject insufficient output capacity; never report a truncated prefix.
    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error>;
}

#[allow(async_fn_in_trait)]
pub trait BleSink {
    type Error: core::fmt::Debug;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error>;
}
