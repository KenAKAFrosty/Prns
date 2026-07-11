//! The engine's per-call view of the interfaces currently attached to the reactor.
//! Point lookups all flow through [`AttachedInterfaces::descriptor_for`], so the scan strategy can change behind this surface without touching a consumer.

use crate::interfaces::{InterfaceDescriptor, InterfaceId};

#[derive(Debug, Clone, Copy)]
pub struct AttachedInterfaces<'a> {
    descriptors: &'a [InterfaceDescriptor],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    Transmit,
    Transport,
}

impl<'a> AttachedInterfaces<'a> {
    pub const fn new(descriptors: &'a [InterfaceDescriptor]) -> Self {
        Self { descriptors }
    }

    pub fn descriptor_for(self, id: InterfaceId) -> Option<&'a InterfaceDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.id == id)
    }

    /// RNS would not push onto a receive-only or downed interface.
    pub fn is_egress_eligible(self, target: InterfaceId, egress_kind: Egress) -> bool {
        self.descriptor_for(target)
            .is_some_and(|descriptor| match egress_kind {
                Egress::Transmit => descriptor.capabilities.allows_transmit(),
                Egress::Transport => descriptor.capabilities.allows_transport(),
            })
    }

    pub const fn len(self) -> usize {
        self.descriptors.len()
    }

    pub const fn is_empty(self) -> bool {
        self.descriptors.is_empty()
    }

    pub fn iter(self) -> core::slice::Iter<'a, InterfaceDescriptor> {
        self.descriptors.iter()
    }
}

impl<'a> IntoIterator for AttachedInterfaces<'a> {
    type Item = &'a InterfaceDescriptor;
    type IntoIter = core::slice::Iter<'a, InterfaceDescriptor>;

    fn into_iter(self) -> Self::IntoIter {
        self.descriptors.iter()
    }
}
