use crate::interfaces::{InterfaceDescriptor, InterfaceId};
#[cfg(feature = "alloc")]
use crate::lemire_index::HeapLemireIndex;

#[derive(Clone, Copy)]
pub struct AttachedInterfaces<'a> {
    descriptors: &'a [InterfaceDescriptor],
    available: Option<&'a (dyn Fn(InterfaceId) -> bool + Sync)>,
    generations: Option<&'a (dyn Fn(InterfaceId) -> u32 + Sync)>,
    #[cfg(feature = "alloc")]
    index: Option<InterfaceIndex<'a>>,
}

impl core::fmt::Debug for AttachedInterfaces<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// A borrowed, allocation-free view of the interfaces currently available to the engine.
#[derive(Clone)]
pub struct AttachedInterfaceIter<'a> {
    descriptors: core::slice::Iter<'a, InterfaceDescriptor>,
    available: Option<&'a (dyn Fn(InterfaceId) -> bool + Sync)>,
}

impl<'a> Iterator for AttachedInterfaceIter<'a> {
    type Item = &'a InterfaceDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        self.descriptors.find(|descriptor| {
            self.available
                .is_none_or(|available| available(descriptor.id))
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let upper = self.descriptors.len();
        (
            if self.available.is_none() { upper } else { 0 },
            Some(upper),
        )
    }
}

impl DoubleEndedIterator for AttachedInterfaceIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.descriptors.rfind(|descriptor| {
            self.available
                .is_none_or(|available| available(descriptor.id))
        })
    }
}

impl core::iter::FusedIterator for AttachedInterfaceIter<'_> {}

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
            available: None,
            generations: None,
            #[cfg(feature = "alloc")]
            index: None,
        }
    }

    /// Apply administrative availability independently of configured interface policy.
    /// The predicate is borrowed and evaluated at lookup/iteration time; runtimes must also
    /// fence their egress queues against availability changes after the engine dispatches.
    pub fn with_availability(
        mut self,
        available: &'a (dyn Fn(InterfaceId) -> bool + Sync),
    ) -> Self {
        self.available = Some(available);
        self
    }

    /// Whether the optional runtime availability gate permits this ID. This deliberately
    /// does not require a descriptor: legacy callers may omit timing descriptors entirely.
    pub fn availability_allows(self, id: InterfaceId) -> bool {
        self.available.is_none_or(|available| available(id))
    }

    /// Attach local administrative epochs, incremented whenever an interface is disabled.
    /// Deferred work can compare its captured epoch before acquiring new queue space.
    pub fn with_generations(
        mut self,
        generations: &'a (dyn Fn(InterfaceId) -> u32 + Sync),
    ) -> Self {
        self.generations = Some(generations);
        self
    }

    pub fn generation_for(self, id: InterfaceId) -> u32 {
        self.generations.map_or(0, |generation| generation(id))
    }

    pub fn descriptor_for(self, id: InterfaceId) -> Option<&'a InterfaceDescriptor> {
        if !self.availability_allows(id) {
            return None;
        }
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

    pub fn len(self) -> usize {
        if self.available.is_none() {
            self.descriptors.len()
        } else {
            self.iter().count()
        }
    }

    pub fn is_empty(self) -> bool {
        self.iter().next().is_none()
    }

    pub fn iter(self) -> AttachedInterfaceIter<'a> {
        AttachedInterfaceIter {
            descriptors: self.descriptors.iter(),
            available: self.available,
        }
    }
}

impl<'a> IntoIterator for AttachedInterfaces<'a> {
    type Item = &'a InterfaceDescriptor;
    type IntoIter = AttachedInterfaceIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
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
            available: None,
            generations: None,
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
            gravity: crate::interfaces::InterfaceGravity::ZERO,
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

    #[test]
    fn administrative_availability_filters_linear_and_indexed_views_consistently() {
        let descriptors = [descriptor(1), descriptor(2), descriptor(3)];
        let indexed = IndexedAttachedInterfaces::from(descriptors.to_vec());
        let available = |id| id != descriptor(2).id;
        for view in [AttachedInterfaces::new(&descriptors), indexed.view()] {
            let view = view.with_availability(&available);
            assert_eq!(view.len(), 2);
            assert!(!view.is_empty());
            assert!(view.descriptor_for(descriptor(2).id).is_none());
            assert!(!view.is_egress_eligible(descriptor(2).id, Egress::Transmit));
            assert!(!view.is_egress_eligible(descriptor(2).id, Egress::Transport));
            assert_eq!(
                view.iter().map(|d| d.id).collect::<std::vec::Vec<_>>(),
                [descriptor(1).id, descriptor(3).id]
            );
            assert_eq!(
                view.into_iter()
                    .rev()
                    .map(|d| d.id)
                    .collect::<std::vec::Vec<_>>(),
                [descriptor(3).id, descriptor(1).id]
            );
            assert!(view.with_availability(&|_| false).is_empty());
        }
    }

    #[test]
    fn availability_is_live_and_separate_from_transmit_policy_and_epoch() {
        let mut disabled_policy = descriptor(1);
        disabled_policy.capabilities.egress = EgressCapability::Disabled;
        let descriptors = [disabled_policy];
        let enabled = core::sync::atomic::AtomicBool::new(true);
        let generation = core::sync::atomic::AtomicU32::new(0);
        let available = |_| enabled.load(core::sync::atomic::Ordering::Relaxed);
        let epoch = |_| generation.load(core::sync::atomic::Ordering::Relaxed);
        let view = AttachedInterfaces::new(&descriptors)
            .with_availability(&available)
            .with_generations(&epoch);
        assert!(view.descriptor_for(disabled_policy.id).is_some());
        assert!(!view.is_egress_eligible(disabled_policy.id, Egress::Transmit));
        enabled.store(false, core::sync::atomic::Ordering::Relaxed);
        generation.store(1, core::sync::atomic::Ordering::Relaxed);
        assert!(view.is_empty());
        assert_eq!(view.generation_for(disabled_policy.id), 1);
        enabled.store(true, core::sync::atomic::Ordering::Relaxed);
        assert_eq!(view.len(), 1);
        assert_eq!(view.generation_for(disabled_policy.id), 1);
    }
}
