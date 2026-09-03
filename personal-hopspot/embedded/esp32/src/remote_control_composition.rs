use personal_hopspot_core::{
    RemoteControlEventHandoff, RemoteControlTargetPairingState, RemoteControlTargetPairingUpdate,
    StableTargetAnnouncementAction, StableTargetAnnouncer,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RemoteControlCompositionEffects {
    wake_ui: bool,
    wake_announcer: bool,
    close_pairing: bool,
}

impl RemoteControlCompositionEffects {
    #[must_use]
    pub(crate) const fn wake_ui(self) -> bool {
        self.wake_ui
    }

    #[must_use]
    pub(crate) const fn wake_announcer(self) -> bool {
        self.wake_announcer
    }

    #[must_use]
    pub(crate) const fn close_pairing(self) -> bool {
        self.close_pairing
    }

    fn for_pairing_update(update: RemoteControlTargetPairingUpdate) -> Self {
        Self {
            wake_ui: matches!(
                update,
                RemoteControlTargetPairingUpdate::Changed
                    | RemoteControlTargetPairingUpdate::ClosePairingWindow
            ),
            wake_announcer: false,
            close_pairing: update == RemoteControlTargetPairingUpdate::ClosePairingWindow,
        }
    }

    fn with_announcer_wake(mut self) -> Self {
        self.wake_announcer = true;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RemoteControlCompositionOutput<T> {
    value: T,
    effects: RemoteControlCompositionEffects,
}

impl<T> RemoteControlCompositionOutput<T> {
    const fn new(value: T, effects: RemoteControlCompositionEffects) -> Self {
        Self { value, effects }
    }

    pub(crate) fn into_parts(self) -> (T, RemoteControlCompositionEffects) {
        (self.value, self.effects)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StableTargetAnnouncementSettlement {
    Succeeded,
    Failed,
}

impl StableTargetAnnouncementSettlement {
    const fn succeeded(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// The fixed-size state owner between synchronous node events, the render loop, and the
/// serialized stable-target announcer task.
///
/// Hardware wake primitives stay outside this type. Each method returns the capacity-one wakes
/// that the ESP32 composition must signal after releasing its critical-section lock.
pub(crate) struct RemoteControlComposition<Attempt>
where
    Attempt: Copy + Eq,
{
    events: RemoteControlEventHandoff<Attempt>,
    announcer: StableTargetAnnouncer,
    transmit_ready: bool,
}

impl<Attempt> RemoteControlComposition<Attempt>
where
    Attempt: Copy + Eq,
{
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            events: RemoteControlEventHandoff::new(),
            announcer: StableTargetAnnouncer::new(),
            transmit_ready: false,
        }
    }

    pub(crate) fn update_pairing(
        &mut self,
        transition: impl FnOnce(
            &mut RemoteControlTargetPairingState<Attempt>,
        ) -> RemoteControlTargetPairingUpdate,
    ) -> RemoteControlCompositionOutput<RemoteControlTargetPairingUpdate> {
        let update = self.events.update(transition);
        RemoteControlCompositionOutput::new(
            update,
            RemoteControlCompositionEffects::for_pairing_update(update),
        )
    }

    pub(crate) fn authorization_persisted(
        &mut self,
        attempt_id: Attempt,
    ) -> RemoteControlCompositionOutput<RemoteControlTargetPairingUpdate> {
        let mut output = self.update_pairing(|state| state.persisted(attempt_id));
        if output.value == RemoteControlTargetPairingUpdate::Changed {
            self.announcer.trigger_automatic();
            output.effects = output.effects.with_announcer_wake();
        }
        output
    }

    #[must_use]
    pub(crate) fn take_current_pairing(&mut self) -> RemoteControlTargetPairingState<Attempt> {
        self.events.take_current()
    }

    pub(crate) fn observe_restored_controller_grants(
        &mut self,
        restored_count: u32,
    ) -> RemoteControlCompositionEffects {
        if restored_count == 0 {
            return RemoteControlCompositionEffects::default();
        }
        self.announcer.trigger_automatic();
        RemoteControlCompositionEffects::default().with_announcer_wake()
    }

    pub(crate) fn set_transmit_ready(&mut self, ready: bool) -> RemoteControlCompositionEffects {
        if self.transmit_ready == ready {
            return RemoteControlCompositionEffects::default();
        }
        self.transmit_ready = ready;
        self.announcer.set_transmit_ready(ready);
        RemoteControlCompositionEffects::default().with_announcer_wake()
    }

    pub(crate) fn request_manual_announcements(&mut self) -> RemoteControlCompositionEffects {
        self.announcer.request_manual();
        RemoteControlCompositionEffects::default().with_announcer_wake()
    }

    pub(crate) fn poll_announcement(
        &mut self,
        now_millis: u64,
    ) -> RemoteControlCompositionOutput<Option<StableTargetAnnouncementAction>> {
        let action = self.announcer.poll(now_millis);
        let starts_stable_announcement = matches!(
            action,
            Some(StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 1 })
                | Some(StableTargetAnnouncementAction::ManualStableTarget)
        );
        let effects = if starts_stable_announcement {
            self.update_pairing(RemoteControlTargetPairingState::stable_announcement_started)
                .effects
        } else {
            RemoteControlCompositionEffects::default()
        };
        RemoteControlCompositionOutput::new(action, effects)
    }

    pub(crate) fn settle_announcement(
        &mut self,
        action: StableTargetAnnouncementAction,
        settlement: StableTargetAnnouncementSettlement,
    ) -> RemoteControlCompositionOutput<bool> {
        let accepted = self.announcer.settle(action);
        let effects = if accepted
            && matches!(
                action,
                StableTargetAnnouncementAction::AutomaticStableTarget { .. }
                    | StableTargetAnnouncementAction::ManualStableTarget
            ) {
            self.update_pairing(|state| state.stable_announcement_settled(settlement.succeeded()))
                .effects
        } else {
            RemoteControlCompositionEffects::default()
        };
        RemoteControlCompositionOutput::new(accepted, effects)
    }

    #[must_use]
    pub(crate) fn next_announcement_deadline_millis(&self) -> Option<u64> {
        self.announcer.next_deadline_millis()
    }

    #[cfg(test)]
    const fn pairing_wake_pending(&self) -> bool {
        self.events.wake_pending()
    }

    #[cfg(test)]
    const fn announcer_is_idle(&self) -> bool {
        self.announcer.is_idle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_hopspot_core::{
        RemoteControlTargetPairingFailure, RemoteControlTargetPairingPhase,
        StableTargetAnnouncementStatus,
    };
    use personal_rns::units::InstantMillis;

    #[derive(Clone, Copy)]
    struct FakeClock {
        now_millis: u64,
    }

    impl FakeClock {
        const fn new(now_millis: u64) -> Self {
            Self { now_millis }
        }

        fn set(&mut self, now_millis: u64) {
            self.now_millis = now_millis;
        }

        fn poll<Attempt>(
            self,
            composition: &mut RemoteControlComposition<Attempt>,
        ) -> RemoteControlCompositionOutput<Option<StableTargetAnnouncementAction>>
        where
            Attempt: Copy + Eq,
        {
            composition.poll_announcement(self.now_millis)
        }
    }

    #[test]
    fn embedded_fake_clock_composes_persistence_wakes_retries_and_readiness() {
        let attempt_id = 0xA5_u8;
        let mut composition = RemoteControlComposition::new();

        composition.update_pairing(RemoteControlTargetPairingState::begin_opening);
        composition.update_pairing(|state| state.opened(0x1234_5678, InstantMillis(60_000)));
        composition.update_pairing(|state| {
            state.confirmation_required(attempt_id, 123_456, InstantMillis(30_000))
        });
        composition.update_pairing(|state| state.authorizing(attempt_id));

        let (persisted, effects) = composition.authorization_persisted(attempt_id).into_parts();
        assert_eq!(persisted, RemoteControlTargetPairingUpdate::Changed);
        assert!(effects.wake_ui());
        assert!(effects.wake_announcer());
        assert!(!effects.close_pairing());
        assert!(composition.pairing_wake_pending());

        let pairing = composition.take_current_pairing();
        assert_eq!(pairing.phase(), RemoteControlTargetPairingPhase::Persisted);
        assert_eq!(pairing.attempt_id(), Some(attempt_id));
        assert!(!composition.pairing_wake_pending());

        // Boot/persistence triggers share one capacity-one owner and coalesce before polling.
        assert!(composition
            .observe_restored_controller_grants(1)
            .wake_announcer());
        assert!(composition
            .observe_restored_controller_grants(2)
            .wake_announcer());
        assert_eq!(
            composition.observe_restored_controller_grants(0),
            Default::default()
        );

        let mut clock = FakeClock::new(10_000);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        assert!(composition.set_transmit_ready(true).wake_announcer());

        let (first, effects) = clock.poll(&mut composition).into_parts();
        let first = first.expect("the persistence burst starts immediately when egress is ready");
        assert_eq!(
            first,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 1 }
        );
        assert!(effects.wake_ui());
        let (accepted, effects) = composition
            .settle_announcement(first, StableTargetAnnouncementSettlement::Failed)
            .into_parts();
        assert!(accepted);
        assert!(effects.wake_ui());
        assert_eq!(
            composition.take_current_pairing().stable_announcement(),
            StableTargetAnnouncementStatus::Failed
        );
        assert_eq!(
            composition.next_announcement_deadline_millis(),
            Some(12_000)
        );

        clock.set(11_999);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        clock.set(12_000);
        let second = clock
            .poll(&mut composition)
            .into_parts()
            .0
            .expect("the second attempt uses the exact two-second offset");
        assert_eq!(
            second,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 2 }
        );
        assert!(
            composition
                .settle_announcement(second, StableTargetAnnouncementSettlement::Succeeded)
                .into_parts()
                .0
        );
        assert_eq!(
            composition.take_current_pairing().stable_announcement(),
            StableTargetAnnouncementStatus::Failed,
            "a later success must not hide an earlier settlement failure"
        );
        assert_eq!(
            composition.next_announcement_deadline_millis(),
            Some(18_000)
        );

        assert!(composition.set_transmit_ready(false).wake_announcer());
        clock.set(20_000);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        assert_eq!(composition.next_announcement_deadline_millis(), None);

        assert!(composition.set_transmit_ready(true).wake_announcer());
        clock.set(25_000);
        let restarted = clock
            .poll(&mut composition)
            .into_parts()
            .0
            .expect("readiness re-arms a fresh burst immediately");
        assert_eq!(
            restarted,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 1 }
        );
        assert!(
            composition
                .settle_announcement(restarted, StableTargetAnnouncementSettlement::Succeeded)
                .into_parts()
                .0
        );

        clock.set(26_999);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        clock.set(27_000);
        let retry = clock
            .poll(&mut composition)
            .into_parts()
            .0
            .expect("the re-armed second attempt uses the two-second offset");
        assert_eq!(
            retry,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 2 }
        );
        assert!(
            composition
                .settle_announcement(retry, StableTargetAnnouncementSettlement::Succeeded)
                .into_parts()
                .0
        );

        clock.set(32_999);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        clock.set(33_000);
        let final_attempt = clock
            .poll(&mut composition)
            .into_parts()
            .0
            .expect("the re-armed third attempt uses the eight-second offset");
        assert_eq!(
            final_attempt,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 3 }
        );
        assert!(
            composition
                .settle_announcement(final_attempt, StableTargetAnnouncementSettlement::Succeeded,)
                .into_parts()
                .0
        );
        assert_eq!(composition.next_announcement_deadline_millis(), None);
        clock.set(u64::MAX);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        assert!(composition.announcer_is_idle());
    }

    #[test]
    fn deliberate_dual_announcement_shares_the_serial_composition_owner() {
        let mut composition = RemoteControlComposition::<u8>::new();
        composition.set_transmit_ready(true);
        composition.observe_restored_controller_grants(1);
        let clock = FakeClock::new(1_000);
        let automatic = clock
            .poll(&mut composition)
            .into_parts()
            .0
            .expect("the automatic announcement is in flight");

        assert!(composition.request_manual_announcements().wake_announcer());
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        assert!(
            composition
                .settle_announcement(automatic, StableTargetAnnouncementSettlement::Succeeded)
                .into_parts()
                .0
        );

        let node = clock
            .poll(&mut composition)
            .into_parts()
            .0
            .expect("the node-page half follows after the in-flight command settles");
        assert_eq!(node, StableTargetAnnouncementAction::ManualNodePage);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        assert!(
            composition
                .settle_announcement(node, StableTargetAnnouncementSettlement::Failed)
                .into_parts()
                .0
        );

        let stable = clock
            .poll(&mut composition)
            .into_parts()
            .0
            .expect("the stable-target half follows the node-page settlement");
        assert_eq!(stable, StableTargetAnnouncementAction::ManualStableTarget);
        assert_eq!(clock.poll(&mut composition).into_parts().0, None);
        let (accepted, effects) = composition
            .settle_announcement(stable, StableTargetAnnouncementSettlement::Failed)
            .into_parts();
        assert!(accepted);
        assert!(effects.wake_ui());
        assert_eq!(
            composition.take_current_pairing().stable_announcement(),
            StableTargetAnnouncementStatus::Failed
        );
    }

    #[test]
    fn persistence_failure_is_owned_by_the_same_visible_state_slot() {
        let attempt_id = 7_u8;
        let mut composition = RemoteControlComposition::new();
        composition.update_pairing(|state| {
            state.confirmation_required(attempt_id, 654_321, InstantMillis(30_000))
        });

        let (update, effects) = composition
            .update_pairing(|state| {
                state.operation_failed(
                    Some(attempt_id),
                    RemoteControlTargetPairingFailure::Persistence,
                )
            })
            .into_parts();
        assert_eq!(update, RemoteControlTargetPairingUpdate::Changed);
        assert!(effects.wake_ui());
        assert!(!effects.wake_announcer());
        assert!(!effects.close_pairing());
        let pairing = composition.take_current_pairing();
        assert_eq!(pairing.phase(), RemoteControlTargetPairingPhase::Failed);
        assert_eq!(
            pairing.failure(),
            Some(RemoteControlTargetPairingFailure::Persistence)
        );
    }
}
