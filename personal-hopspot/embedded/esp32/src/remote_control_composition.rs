use personal_hopspot_core::{
    RemoteControlEventHandoff, RemoteControlTargetPairingFailure, RemoteControlTargetPairingState,
    RemoteControlTargetPairingUpdate, StableTargetAnnouncementAction, StableTargetAnnouncer,
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
        self.update_pairing(|state| state.persisted(attempt_id))
    }

    pub(crate) fn target_persistence_failed(
        &mut self,
        attempt_id: Attempt,
    ) -> RemoteControlCompositionOutput<RemoteControlTargetPairingUpdate> {
        self.update_pairing(|state| {
            if state.attempt_id().is_none() {
                return RemoteControlTargetPairingUpdate::StaleAttempt;
            }
            state.operation_failed(
                Some(attempt_id),
                RemoteControlTargetPairingFailure::Persistence,
            )
        })
    }

    #[must_use]
    pub(crate) fn take_current_pairing(&mut self) -> RemoteControlTargetPairingState<Attempt> {
        self.events.take_current()
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
    fn persistence_and_readiness_never_schedule_announcements() {
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
        assert!(!effects.wake_announcer());
        assert!(!effects.close_pairing());
        assert!(composition.pairing_wake_pending());

        let pairing = composition.take_current_pairing();
        assert_eq!(pairing.phase(), RemoteControlTargetPairingPhase::Persisted);
        assert_eq!(pairing.attempt_id(), Some(attempt_id));
        assert!(!composition.pairing_wake_pending());

        let mut clock = FakeClock::new(0);
        for now in [0, 2_000, 8_000, 60_000, u64::MAX] {
            clock.set(now);
            for ready in [true, false, true] {
                composition.set_transmit_ready(ready);
                assert_eq!(clock.poll(&mut composition).into_parts().0, None);
                assert_eq!(composition.next_announcement_deadline_millis(), None);
                assert!(composition.announcer_is_idle());
            }
        }
    }

    #[test]
    fn deliberate_dual_announcement_shares_the_serial_composition_owner() {
        let mut composition = RemoteControlComposition::<u8>::new();
        composition.set_transmit_ready(true);
        let clock = FakeClock::new(1_000);
        assert!(composition.request_manual_announcements().wake_announcer());

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
    fn explicit_success_does_not_schedule_a_followup() {
        let mut composition = RemoteControlComposition::<u8>::new();
        composition.set_transmit_ready(true);
        composition.request_manual_announcements();
        for expected in [
            StableTargetAnnouncementAction::ManualNodePage,
            StableTargetAnnouncementAction::ManualStableTarget,
        ] {
            let action = composition.poll_announcement(0).into_parts().0.unwrap();
            assert_eq!(action, expected);
            assert!(
                composition
                    .settle_announcement(action, StableTargetAnnouncementSettlement::Succeeded)
                    .into_parts()
                    .0
            );
        }
        assert!(composition.announcer_is_idle());
        assert_eq!(composition.poll_announcement(u64::MAX).into_parts().0, None);
        assert_eq!(composition.next_announcement_deadline_millis(), None);
    }

    #[test]
    fn persistence_failure_is_owned_by_the_same_visible_state_slot() {
        let attempt_id = 7_u8;
        let mut composition = RemoteControlComposition::new();

        let (unrelated, effects) = composition
            .target_persistence_failed(attempt_id)
            .into_parts();
        assert_eq!(unrelated, RemoteControlTargetPairingUpdate::StaleAttempt);
        assert_eq!(effects, Default::default());
        assert_eq!(
            composition.take_current_pairing().phase(),
            RemoteControlTargetPairingPhase::Idle
        );

        composition.update_pairing(|state| {
            state.confirmation_required(attempt_id, 654_321, InstantMillis(30_000))
        });

        let (unrelated, effects) = composition.target_persistence_failed(8_u8).into_parts();
        assert_eq!(unrelated, RemoteControlTargetPairingUpdate::StaleAttempt);
        assert_eq!(effects, Default::default());

        let (update, effects) = composition
            .target_persistence_failed(attempt_id)
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

    #[test]
    fn expiry_during_authorization_replaces_the_visible_state_and_wakes_the_ui() {
        let attempt_id = 0xA5_u8;
        let mut composition = RemoteControlComposition::new();
        composition.update_pairing(|state| {
            state.confirmation_required(attempt_id, 123_456, InstantMillis(30_000))
        });
        composition.update_pairing(|state| state.authorizing(attempt_id));
        assert_eq!(
            composition.take_current_pairing().phase(),
            RemoteControlTargetPairingPhase::Authorizing,
        );

        let (update, effects) = composition
            .update_pairing(|state| state.expired(Some(attempt_id)))
            .into_parts();

        assert_eq!(update, RemoteControlTargetPairingUpdate::Changed);
        assert!(effects.wake_ui());
        assert!(!effects.wake_announcer());
        assert!(!effects.close_pairing());
        assert!(composition.pairing_wake_pending());
        let pairing = composition.take_current_pairing();
        assert_eq!(pairing.phase(), RemoteControlTargetPairingPhase::Expired);
        assert_eq!(pairing.attempt_id(), Some(attempt_id));
        assert_eq!(pairing.failure(), None);
        assert!(!composition.pairing_wake_pending());
    }
}
