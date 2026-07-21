use crate::interfaces::{InterfaceDescriptor, InterfaceId};
#[cfg(feature = "alloc")]
use crate::lemire_index::HeapLemireIndex;

#[derive(Debug, Clone, Copy)]
pub struct AttachedInterfaces<'a> {
    descriptors: &'a [InterfaceDescriptor],
    #[cfg(feature = "alloc")]
    index: Option<InterfaceIndex<'a>>,
}

#[cfg(feature = "alloc")]
#[derive(Debug, Clone, Copy)]
struct InterfaceIndex<'a> {
    ids: &'a [InterfaceId],
    index: &'a HeapLemireIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    Transmit,
    Transport,
}

impl<'a> AttachedInterfaces<'a> {
    pub const fn new(descriptors: &'a [InterfaceDescriptor]) -> Self {
        Self {
            descriptors,
            #[cfg(feature = "alloc")]
            index: None,
        }
    }

    pub fn descriptor_for(self, id: InterfaceId) -> Option<&'a InterfaceDescriptor> {
        #[cfg(feature = "alloc")]
        if let Some(indexed) = self.index {
            return indexed
                .index
                .get(&id, indexed.ids)
                .map(|row| &self.descriptors[row]);
        }
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.id == id)
    }

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

#[cfg(feature = "alloc")]
#[derive(Debug, Default)]
pub struct IndexedAttachedInterfaces {
    descriptors: alloc::vec::Vec<InterfaceDescriptor>,
    ids: alloc::vec::Vec<InterfaceId>,
    index: HeapLemireIndex,
}

#[cfg(feature = "alloc")]
impl IndexedAttachedInterfaces {
    pub fn view(&self) -> AttachedInterfaces<'_> {
        AttachedInterfaces {
            descriptors: &self.descriptors,
            index: Some(InterfaceIndex {
                ids: &self.ids,
                index: &self.index,
            }),
        }
    }

    pub fn descriptors(&self) -> &[InterfaceDescriptor] {
        &self.descriptors
    }

    pub fn push(&mut self, descriptor: InterfaceDescriptor) {
        self.ids.push(descriptor.id);
        self.descriptors.push(descriptor);
        self.index.insert(self.ids.len() - 1, &self.ids);
    }

    pub fn remove(&mut self, id: InterfaceId) {
        let Some(row) = self.index.get(&id, &self.ids) else {
            return;
        };
        self.index.remove(&id, &self.ids);
        let last = self.ids.len() - 1;
        if row != last {
            self.index.repoint(&self.ids[last], row, &self.ids);
        }
        self.ids.swap_remove(row);
        self.descriptors.swap_remove(row);
    }
}

#[cfg(feature = "alloc")]
impl From<alloc::vec::Vec<InterfaceDescriptor>> for IndexedAttachedInterfaces {
    fn from(descriptors: alloc::vec::Vec<InterfaceDescriptor>) -> Self {
        let mut roster = Self::default();
        for descriptor in descriptors {
            roster.push(descriptor);
        }
        roster
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use crate::interfaces::{
        AnnounceBandwidthCap, BitrateBps, EgressCapability, IngressCapability,
        InterfaceCapabilities, InterfaceCommonPolicy, InterfaceMode, TransportCapability,
    };

    fn descriptor(n: u32) -> InterfaceDescriptor {
        let mut id = [0x07u8, 0, 0, 0, 0, 0, 0, 0];
        id[4..].copy_from_slice(&n.to_be_bytes());
        InterfaceDescriptor {
            id: InterfaceId::new(id),
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::Full,
            bitrate: BitrateBps::guess(1_000_000_000),
            hardware_mtu: None,
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }

    #[test]
    fn an_indexed_view_answers_like_the_linear_scan_through_churn() {
        let mut roster = IndexedAttachedInterfaces::default();
        let mut live: std::vec::Vec<u32> = std::vec::Vec::new();
        let mut rng = 0x0123_4567_89AB_CDEFu64;
        let mut next_id = 0u32;

        for _ in 0..1_000 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let attach = live.len() < 2 || !(rng >> 33).is_multiple_of(3);
            if attach {
                roster.push(descriptor(next_id));
                live.push(next_id);
                next_id += 1;
            } else {
                let victim = ((rng >> 17) as usize) % live.len();
                roster.remove(descriptor(live[victim]).id);
                live.swap_remove(victim);
            }

            let view = roster.view();
            for &n in &live {
                let id = descriptor(n).id;
                assert_eq!(
                    view.descriptor_for(id).map(|found| found.id),
                    Some(id),
                    "every live interface resolves through the indexed view",
                );
            }
            assert!(view.descriptor_for(descriptor(next_id + 7).id).is_none());
        }
        assert!(
            live.len() > 50,
            "the run must grow enough to force reindexing"
        );
    }

    #[test]
    fn removing_an_unknown_id_leaves_the_roster_intact() {
        let mut roster = IndexedAttachedInterfaces::from(std::vec![descriptor(1), descriptor(2)]);
        roster.remove(descriptor(9).id);
        assert_eq!(roster.descriptors().len(), 2);
        assert!(roster
            .view()
            .is_egress_eligible(descriptor(2).id, Egress::Transport));
    }
}
