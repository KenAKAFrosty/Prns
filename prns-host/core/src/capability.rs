use alloc::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BackendKind {
    Native,
    Browser,
    Cooperative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    Loopback,
    TcpClient,
    TcpServer,
    Udp,
    Serial,
    Usb,
    Bluetooth,
    Wifi,
    WebSocket,
    BrowserRendezvous,
    I2p,
    Weave,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendCapabilities {
    backend: BackendKind,
    available: BTreeSet<Capability>,
}

impl BackendCapabilities {
    #[must_use]
    pub fn new(backend: BackendKind, available: impl IntoIterator<Item = Capability>) -> Self {
        Self {
            backend,
            available: available.into_iter().collect(),
        }
    }

    #[must_use]
    pub const fn backend(&self) -> BackendKind {
        self.backend
    }

    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.available.contains(&capability)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = Capability> + '_ {
        self.available.iter().copied()
    }

    #[must_use]
    pub fn missing(&self, required: impl IntoIterator<Item = Capability>) -> BTreeSet<Capability> {
        required
            .into_iter()
            .filter(|capability| !self.supports(*capability))
            .collect()
    }
}
