use core::future::poll_fn;
use core::task::Poll;

use crate::engine::{EngineState, InstantMillis, Journaled};
use crate::interfaces::{AttachedInterfaces, InterfaceId, InterfaceIfac, InterfaceStatus};
use crate::storage::StorageLayout;

use super::egress::{lane_serves, route_reaction, InterfacePacer, ManifoldEgress};
use super::EmbassyInterfaceStatus;

/// A live lane association, independent of a status object's diagnostic identity.
/// Dedicated lane publication retags this association with the descriptor.
#[derive(Clone, Copy)]
pub(super) struct LaneStatus<'a> {
    pub lane: InterfaceId,
    pub status: &'a EmbassyInterfaceStatus,
}

#[derive(Clone, Copy)]
pub(super) struct Availability<'a> {
    bound: &'a [LaneStatus<'a>],
    fixed: &'a [&'a EmbassyInterfaceStatus],
}

impl<'a> Availability<'a> {
    pub fn bound(bound: &'a [LaneStatus<'a>]) -> Self {
        Self { bound, fixed: &[] }
    }

    pub fn fixed(fixed: &'a [&'a EmbassyInterfaceStatus]) -> Self {
        Self { bound: &[], fixed }
    }

    fn lanes(self) -> impl Iterator<Item = LaneStatus<'a>> {
        self.bound
            .iter()
            .copied()
            .chain(self.fixed.iter().map(|status| LaneStatus {
                lane: status.id(),
                status,
            }))
    }

    pub fn allows(self, target: InterfaceId) -> bool {
        self.lanes()
            .filter(|binding| lane_serves(binding.lane, target))
            .all(|binding| binding.status.is_available())
    }

    pub fn generation(self, target: InterfaceId) -> u32 {
        self.lanes()
            .filter(|binding| lane_serves(binding.lane, target))
            .fold(0u32, |generation, binding| {
                generation.wrapping_add(binding.status.disable_generation())
            })
    }

    pub async fn changed(self) {
        poll_fn(|cx| {
            let mut changed = false;
            for binding in self.lanes() {
                binding.status.register_manifold_waker(cx.waker());
                changed |= binding.status.availability_state()
                    != binding.status.applied_availability_state();
            }
            if changed {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }

    /// Runs before input dispatch. Live admission guards continue to reject the
    /// unretired epoch if another core switches power during a synchronous step.
    #[allow(clippy::too_many_arguments)]
    pub fn reconcile<S: StorageLayout>(
        self,
        engine: &mut EngineState<S>,
        attached: AttachedInterfaces<'_>,
        egress: &mut impl ManifoldEgress,
        ifacs: &[InterfaceIfac],
        pacers: &mut [InterfacePacer],
        now: InstantMillis,
        journaled: &mut impl FnMut(Journaled<'_>),
    ) -> bool {
        let mut changed = false;
        let allows = |id| self.allows(id);
        let generation = |id| self.generation(id);
        let live = attached
            .with_availability(&allows)
            .with_generations(&generation);
        for binding in self.lanes() {
            let (enabled, generation) = binding.status.availability_state();
            let (applied_enabled, retired) = binding.status.applied_availability_state();
            if (enabled, generation) == (applied_enabled, retired) {
                continue;
            }
            changed = true;
            if generation != retired {
                let _ = engine.interface_unavailable(binding.lane, now, live, &mut |reaction| {
                    route_reaction(reaction, egress, ifacs, pacers, now, journaled)
                });
                for pacer in pacers
                    .iter_mut()
                    .filter(|pacer| lane_serves(binding.lane, pacer.id))
                {
                    pacer.discard_pending();
                }
            }
            if enabled && (!applied_enabled || generation != retired) {
                for descriptor in attached {
                    if lane_serves(binding.lane, descriptor.id) {
                        engine.interface_attached(descriptor.id, now);
                    }
                }
            }
            // A newer concurrent transition stays pending and cannot be admitted.
            binding.status.acknowledge_availability(enabled, generation);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{ConnectionState, InterfaceKind};
    use crate::manifold::driver::test_support::descriptor;
    use crate::manifold::driver::PooledEgress;
    use crate::storage::GrowableHeap;

    #[test]
    fn fast_resume_retires_before_exposing_members() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"fleet");
        let child = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"member");
        let sibling = InterfaceId::new([0x19; 8]);
        let status =
            EmbassyInterfaceStatus::new_unaccounted(supervisor, ConnectionState::Connected);
        let bindings = [LaneStatus {
            lane: supervisor,
            status: &status,
        }];
        let availability = Availability::bound(&bindings);
        let descriptors = [descriptor(child), descriptor(sibling)];
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut egress = PooledEgress::<1>::new();
        status.disable();
        status.enable();
        assert!(!availability.allows(child));
        assert!(availability.allows(sibling));
        assert_eq!(availability.generation(child), 1);
        assert!(availability.reconcile(
            &mut engine,
            AttachedInterfaces::new(&descriptors),
            &mut egress,
            &[],
            &mut [],
            InstantMillis(10),
            &mut |_| {},
        ));
        assert!(availability.allows(child));
        assert!(!availability.reconcile(
            &mut engine,
            AttachedInterfaces::new(&descriptors),
            &mut egress,
            &[],
            &mut [],
            InstantMillis(11),
            &mut |_| {},
        ));
    }

    #[test]
    fn member_generation_tracks_both_its_own_and_supervisor_power_cycles() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"parent");
        let child = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"child");
        let parent =
            EmbassyInterfaceStatus::new_unaccounted(supervisor, ConnectionState::Connected);
        let member = EmbassyInterfaceStatus::new_unaccounted(child, ConnectionState::Connected);
        let statuses = [&parent, &member];
        let availability = Availability::fixed(&statuses);
        assert_eq!(availability.generation(child), 0);
        member.disable();
        member.enable();
        member.acknowledge_availability(true, member.disable_generation());
        assert_eq!(availability.generation(child), 1);
        parent.disable();
        parent.enable();
        parent.acknowledge_availability(true, parent.disable_generation());
        assert_eq!(availability.generation(child), 2);
        assert!(availability.allows(child));
    }

    #[test]
    fn manifold_and_queue_waiters_have_independent_wake_registrations() {
        use core::task::{Context, Waker};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        use std::task::Wake;
        struct Count(AtomicUsize);
        impl Wake for Count {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let status = EmbassyInterfaceStatus::new_accounted(
            InterfaceId::new([0x31; 8]),
            ConnectionState::Connected,
        );
        let bindings = [LaneStatus {
            lane: InterfaceId::new([0x31; 8]),
            status: &status,
        }];
        let availability = Availability::bound(&bindings);
        let manifold_wakes = Arc::new(Count(AtomicUsize::new(0)));
        let queue_wakes = Arc::new(Count(AtomicUsize::new(0)));
        let manifold_waker = Waker::from(manifold_wakes.clone());
        let queue_waker = Waker::from(queue_wakes.clone());
        let mut changed = core::pin::pin!(availability.changed());
        use core::future::Future;
        assert!(changed
            .as_mut()
            .poll(&mut Context::from_waker(&manifold_waker))
            .is_pending());
        status.register_queue_waker(
            super::super::InterfaceQueueWakeSlot::EgressConsumer,
            &queue_waker,
        );
        status.disable();
        status.enable();
        assert!(manifold_wakes.0.load(Ordering::SeqCst) >= 2);
        assert!(queue_wakes.0.load(Ordering::SeqCst) >= 2);
        assert!(changed
            .as_mut()
            .poll(&mut Context::from_waker(&manifold_waker))
            .is_ready());
    }
}
