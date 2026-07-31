use alloc::collections::BTreeSet;

use crate::{BackendKind, Capability, InterfaceKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendCapabilities {
    backend: BackendKind,
    available: BTreeSet<Capability>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendInfo {
    backend: BackendKind,
    capabilities: BTreeSet<Capability>,
    interface_kinds: BTreeSet<InterfaceKind>,
}

impl BackendInfo {
    #[must_use]
    pub fn new(
        backend: BackendKind,
        capabilities: impl IntoIterator<Item = Capability>,
        interface_kinds: impl IntoIterator<Item = InterfaceKind>,
    ) -> Self {
        Self {
            backend,
            capabilities: capabilities.into_iter().collect(),
            interface_kinds: interface_kinds.into_iter().collect(),
        }
    }

    #[must_use]
    pub const fn backend(&self) -> BackendKind {
        self.backend
    }

    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    #[must_use]
    pub fn supports_interface(&self, kind: InterfaceKind) -> bool {
        self.interface_kinds.contains(&kind)
    }

    pub fn capabilities(&self) -> impl ExactSizeIterator<Item = Capability> + '_ {
        self.capabilities.iter().copied()
    }

    pub fn interface_kinds(&self) -> impl ExactSizeIterator<Item = InterfaceKind> + '_ {
        self.interface_kinds.iter().copied()
    }
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
