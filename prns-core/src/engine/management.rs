use core::num::NonZeroUsize;

use crate::interfaces::AttachedInterfaces;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::RemovedRoute;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::{DestinationHash, TransportId};

use super::{EngineState, WakeSchedules};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropRouteOutcome {
    Dropped,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropRoutesViaOutcome {
    pub dropped_routes: u32,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct DropRouteEffect(DropRouteEffectState);

#[derive(Debug, PartialEq, Eq)]
enum DropRouteEffectState {
    Dropped {
        removed: RemovedRoute,
        wake_schedules: WakeSchedules,
    },
    NotFound,
}

impl DropRouteEffect {
    pub fn outcome(&self) -> DropRouteOutcome {
        match self.0 {
            DropRouteEffectState::Dropped { .. } => DropRouteOutcome::Dropped,
            DropRouteEffectState::NotFound => DropRouteOutcome::NotFound,
        }
    }

    pub fn removed_route(&self) -> Option<RemovedRoute> {
        match self.0 {
            DropRouteEffectState::Dropped { removed, .. } => Some(removed),
            DropRouteEffectState::NotFound => None,
        }
    }

    pub fn wake_schedules(&self) -> WakeSchedules {
        match self.0 {
            DropRouteEffectState::Dropped { wake_schedules, .. } => wake_schedules,
            DropRouteEffectState::NotFound => WakeSchedules::UNCHANGED,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct DropRoutesViaEffect(DropRoutesViaEffectState);

#[derive(Debug, PartialEq, Eq)]
enum DropRoutesViaEffectState {
    Dropped {
        dropped_routes: NonZeroUsize,
        wake_schedules: WakeSchedules,
    },
    NoRoutes,
}

impl DropRoutesViaEffect {
    pub fn outcome(&self) -> DropRoutesViaOutcome {
        DropRoutesViaOutcome {
            dropped_routes: u32::try_from(self.dropped_route_count()).unwrap_or(u32::MAX),
        }
    }

    pub fn dropped_route_count(&self) -> usize {
        match self.0 {
            DropRoutesViaEffectState::Dropped { dropped_routes, .. } => dropped_routes.get(),
            DropRoutesViaEffectState::NoRoutes => 0,
        }
    }

    pub fn wake_schedules(&self) -> WakeSchedules {
        match self.0 {
            DropRoutesViaEffectState::Dropped { wake_schedules, .. } => wake_schedules,
            DropRoutesViaEffectState::NoRoutes => WakeSchedules::UNCHANGED,
        }
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub fn drop_route(
        &mut self,
        destination: &DestinationHash,
        interfaces: AttachedInterfaces<'_>,
    ) -> DropRouteEffect {
        let Some(removed) = self.routing_table.drop_route(destination) else {
            return DropRouteEffect(DropRouteEffectState::NotFound);
        };
        let _ = self.scheduled_announces.cancel(destination);
        self.dirty_interfaces.mark(removed.receiving_interface);
        DropRouteEffect(DropRouteEffectState::Dropped {
            removed,
            wake_schedules: self.route_removal_wake_schedules(interfaces),
        })
    }

    pub fn drop_routes_via(
        &mut self,
        transport: TransportId,
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> DropRoutesViaEffect {
        let dirty = &mut self.dirty_interfaces;
        let scheduled_announces = &mut self.scheduled_announces;
        let dropped_routes = self
            .routing_table
            .drop_routes_via(transport, &mut |removed| {
                let _ = scheduled_announces.cancel(&removed.destination);
                dirty.mark(removed.receiving_interface);
                on_removed(removed);
            });
        let Some(dropped_routes) = NonZeroUsize::new(dropped_routes) else {
            return DropRoutesViaEffect(DropRoutesViaEffectState::NoRoutes);
        };
        DropRoutesViaEffect(DropRoutesViaEffectState::Dropped {
            dropped_routes,
            wake_schedules: self.route_removal_wake_schedules(interfaces),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::engine::test_support::{routable_descriptor, TestStorageLayout};
    use crate::engine::WakeSchedule;
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::interfaces::{InterfaceDescriptor, InterfaceId};
    use crate::routing::announce::{Announce, AnnounceId, DottedNameHash};
    use crate::routing::{AnnounceArrival, NextHop, RouteRemovalCause};
    use crate::units::InstantMillis;

    const SOURCE: InterfaceId = InterfaceId::new([0xEE; 8]);

    fn interfaces() -> [InterfaceDescriptor; 1] {
        [routable_descriptor(SOURCE)]
    }

    fn destination(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn add_route(
        engine: &mut EngineState<TestStorageLayout>,
        destination: DestinationHash,
        next_hop: NextHop,
        learned_at: InstantMillis,
    ) {
        let announce = Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0x31; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0x41; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0x51; 10]),
            announce_id: AnnounceId::from_wire([destination.as_bytes()[0]; 10]),
            ratchet: None,
            signature: Ed25519Signature([0x61; 64]),
            app_data: b"",
        };
        let _ = engine.routing_table.upsert_route(
            &AnnounceArrival {
                announce,
                hops: 1,
                arrived_at: learned_at,
                receiving_interface: SOURCE,
                next_hop,
                is_path_response: false,
            },
            AttachedInterfaces::new(&interfaces()),
            &mut |_| {},
        );
    }

    #[test]
    fn dropping_one_route_returns_its_removal_and_complete_wake_delta() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let dropped_destination = destination(0x21);
        let unrelated = destination(0x22);
        add_route(
            &mut engine,
            dropped_destination,
            NextHop::Direct,
            InstantMillis(1_000),
        );
        let _ =
            engine
                .scheduled_announces
                .schedule(dropped_destination, InstantMillis(100), SOURCE, 1);
        let _ = engine
            .scheduled_announces
            .schedule(unrelated, InstantMillis(200), SOURCE, 1);

        let effect =
            engine.drop_route(&dropped_destination, AttachedInterfaces::new(&interfaces()));

        assert_eq!(effect.outcome(), DropRouteOutcome::Dropped);
        assert_eq!(
            effect.removed_route(),
            Some(RemovedRoute {
                destination: dropped_destination,
                receiving_interface: SOURCE,
                cause: RouteRemovalCause::Dropped,
            }),
        );
        assert_eq!(effect.wake_schedules().expired_routes, WakeSchedule::Idle);
        assert_eq!(
            effect.wake_schedules().scheduled_announces,
            WakeSchedule::At(InstantMillis(200)),
        );
        assert_eq!(engine.scheduled_announce_count(), 1);
        assert_eq!(
            engine
                .scheduled_announces
                .iter()
                .next()
                .map(|entry| entry.destination),
            Some(unrelated),
        );
        assert_eq!(
            effect.wake_schedules().expired_destination_identities,
            WakeSchedule::Idle,
        );

        let missing =
            engine.drop_route(&dropped_destination, AttachedInterfaces::new(&interfaces()));
        assert_eq!(missing.outcome(), DropRouteOutcome::NotFound);
        assert_eq!(missing.removed_route(), None);
        assert_eq!(missing.wake_schedules(), WakeSchedules::UNCHANGED);
    }

    #[test]
    fn dropping_routes_via_transport_returns_a_nonzero_count_and_exact_wake_delta() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let dropped_transport = TransportId::new([0xA1; 16]);
        let surviving_transport = TransportId::new([0xB1; 16]);
        for (byte, transport, learned_at) in [
            (0x21, dropped_transport, InstantMillis(1_000)),
            (0x22, dropped_transport, InstantMillis(2_000)),
            (0x23, surviving_transport, InstantMillis(3_000)),
        ] {
            add_route(
                &mut engine,
                destination(byte),
                NextHop::Via(transport),
                learned_at,
            );
        }
        for (byte, due_at) in [(0x21, 100), (0x22, 200), (0x23, 300)] {
            let _ = engine.scheduled_announces.schedule(
                destination(byte),
                InstantMillis(due_at),
                SOURCE,
                1,
            );
        }
        let mut removed = std::vec::Vec::new();

        let effect = engine.drop_routes_via(
            dropped_transport,
            AttachedInterfaces::new(&interfaces()),
            &mut |route| removed.push(route),
        );

        removed.sort_by_key(|route| *route.destination.as_bytes());
        assert_eq!(effect.dropped_route_count(), 2);
        assert_eq!(effect.outcome(), DropRoutesViaOutcome { dropped_routes: 2 });
        assert_eq!(
            removed
                .iter()
                .map(|route| route.destination)
                .collect::<std::vec::Vec<_>>(),
            std::vec![destination(0x21), destination(0x22)],
        );
        assert_eq!(
            effect.wake_schedules().expired_routes,
            engine.route_expiry_wake(AttachedInterfaces::new(&interfaces())),
        );
        assert_eq!(
            effect.wake_schedules().scheduled_announces,
            WakeSchedule::At(InstantMillis(300)),
        );
        assert_eq!(engine.scheduled_announce_count(), 1);
        assert_eq!(
            engine
                .scheduled_announces
                .iter()
                .next()
                .map(|entry| entry.destination),
            Some(destination(0x23)),
        );

        let unchanged = engine.drop_routes_via(
            dropped_transport,
            AttachedInterfaces::new(&interfaces()),
            &mut |_| {},
        );
        assert_eq!(unchanged.dropped_route_count(), 0);
        assert_eq!(
            unchanged.outcome(),
            DropRoutesViaOutcome { dropped_routes: 0 },
        );
        assert_eq!(unchanged.wake_schedules(), WakeSchedules::UNCHANGED);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn route_drop_outcome_saturates_counts_beyond_its_public_width() {
        let effect = DropRoutesViaEffect(DropRoutesViaEffectState::Dropped {
            dropped_routes: NonZeroUsize::new(usize::try_from(u32::MAX).unwrap() + 1).unwrap(),
            wake_schedules: WakeSchedules::UNCHANGED,
        });

        assert_eq!(
            effect.outcome(),
            DropRoutesViaOutcome {
                dropped_routes: u32::MAX,
            },
        );
    }
}
