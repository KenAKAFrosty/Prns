use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleAddress, BleBackend, BleEvent, BleIdentity, BleLink, BleSink, Control,
    DialOutcome, L2capPlan, LinkCapabilities, PeerProtocol, RadioMode, ScanningMode,
};
use prns_simulation::ble::{
    VirtualBleBackend, VirtualBleError, VirtualBleLink, VirtualBleSink, VirtualBleSource,
};

use super::WireGate;

pub(crate) struct GatedBackend {
    inner: VirtualBleBackend,
    gate: WireGate,
}

impl GatedBackend {
    pub(crate) fn new(inner: VirtualBleBackend, gate: WireGate) -> Self {
        Self { inner, gate }
    }
}

impl<const MAX_PEERS: usize> BleBackend<MAX_PEERS> for GatedBackend {
    type Error = VirtualBleError;
    type Link = GatedLink;

    fn blocked(&self) -> Option<&'static str> {
        BleBackend::<MAX_PEERS>::blocked(&self.inner)
    }

    async fn local_capabilities(
        &mut self,
        configured: LinkCapabilities,
    ) -> Result<LinkCapabilities, Self::Error> {
        BleBackend::<MAX_PEERS>::local_capabilities(&mut self.inner, configured).await
    }

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Self::Error> {
        BleBackend::<MAX_PEERS>::set_advertising(&mut self.inner, mode).await
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Self::Error> {
        BleBackend::<MAX_PEERS>::set_scanning(&mut self.inner, mode).await
    }

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), Self::Error> {
        BleBackend::<MAX_PEERS>::set_radio_mode(&mut self.inner, mode).await
    }

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        BleBackend::<MAX_PEERS>::dial(&mut self.inner, address).await
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        BleBackend::<MAX_PEERS>::on_link_closed(&mut self.inner, address).await;
    }

    async fn next_event(&mut self) -> BleEvent<Self::Link> {
        let wrap = |inner| GatedLink {
            inner,
            gate: self.gate.clone(),
        };
        match BleBackend::<MAX_PEERS>::next_event(&mut self.inner).await {
            BleEvent::Sighting { address, rssi } => BleEvent::Sighting { address, rssi },
            BleEvent::DialFailed { address } => BleEvent::DialFailed { address },
            BleEvent::Inbound(inner) => BleEvent::Inbound(wrap(inner)),
            BleEvent::LinkReady {
                link,
                origin,
                peer_rssi,
            } => BleEvent::LinkReady {
                link: wrap(link),
                origin,
                peer_rssi,
            },
        }
    }
}

pub(crate) struct GatedLink {
    inner: VirtualBleLink,
    gate: WireGate,
}

impl BleLink for GatedLink {
    type Error = VirtualBleError;
    type Source = VirtualBleSource;
    type Sink = GatedSink;

    fn peer_protocol(&self) -> PeerProtocol {
        self.inner.peer_protocol()
    }

    fn address(&self) -> BleAddress {
        self.inner.address()
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, Self::Error> {
        self.inner.receive_columba_peer_identity().await
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), Self::Error> {
        self.inner.send_columba_identity(identity).await
    }

    async fn control_send(&mut self, message: &Control) -> Result<(), Self::Error> {
        self.inner.control_send(message).await
    }

    async fn control_recv(&mut self) -> Result<Control, Self::Error> {
        self.inner.control_recv().await
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Self::Error> {
        self.inner.upgrade(plan).await
    }

    fn into_data(self) -> (Self::Source, Self::Sink) {
        let (source, inner) = self.inner.into_data();
        (
            source,
            GatedSink {
                inner,
                gate: self.gate,
            },
        )
    }
}

pub(crate) struct GatedSink {
    inner: VirtualBleSink,
    gate: WireGate,
}

impl BleSink for GatedSink {
    type Error = VirtualBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        match self.gate.before_send(frame).await {
            super::Disposition::Forward => self.inner.send_frame(frame).await,
            super::Disposition::Drop => Ok(()),
        }
    }
}
