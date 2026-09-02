use personal_rns::remote_control::RemoteControlPairingAttemptId;
use personal_rns::units::InstantMillis;

pub const STABLE_TARGET_ANNOUNCE_OFFSETS_MILLIS: [u64; 3] = [0, 2_000, 8_000];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteControlPairingAvailability {
    Unavailable,
    Available,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteControlTargetPairingPhase {
    Idle,
    Opening,
    Invitation,
    Confirmation,
    AwaitingControllerCommit,
    Authorizing,
    Persisted,
    Rejected,
    Expired,
    Cancelled,
    Failed,
}

impl RemoteControlTargetPairingPhase {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Persisted | Self::Rejected | Self::Expired | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteControlTargetPairingFailure {
    Projection,
    Correlation,
    Open,
    Close,
    Approval,
    Rejection,
    Persistence,
    PairingExpiry,
    LinkClosed,
    CompletionExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StableTargetAnnouncementStatus {
    Idle,
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteControlTargetPairingState<Attempt = RemoteControlPairingAttemptId>
where
    Attempt: Copy + Eq,
{
    phase: RemoteControlTargetPairingPhase,
    attempt_id: Option<Attempt>,
    invitation_code: Option<u32>,
    confirmation_code: Option<u32>,
    controller_committed: bool,
    expires_at: Option<InstantMillis>,
    failure: Option<RemoteControlTargetPairingFailure>,
    stable_announcement: StableTargetAnnouncementStatus,
    revision: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteControlTargetPairingUpdate {
    Changed,
    PreservedTerminal,
    StaleAttempt,
    ClosePairingWindow,
}

impl RemoteControlTargetPairingUpdate {
    const fn wakes_ui(self) -> bool {
        matches!(self, Self::Changed | Self::ClosePairingWindow)
    }
}

impl<Attempt> RemoteControlTargetPairingState<Attempt>
where
    Attempt: Copy + Eq,
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            phase: RemoteControlTargetPairingPhase::Idle,
            attempt_id: None,
            invitation_code: None,
            confirmation_code: None,
            controller_committed: false,
            expires_at: None,
            failure: None,
            stable_announcement: StableTargetAnnouncementStatus::Idle,
            revision: 0,
        }
    }

    #[must_use]
    pub const fn phase(self) -> RemoteControlTargetPairingPhase {
        self.phase
    }

    #[must_use]
    pub const fn attempt_id(self) -> Option<Attempt> {
        self.attempt_id
    }

    #[must_use]
    pub const fn invitation_code(self) -> Option<u32> {
        self.invitation_code
    }

    #[must_use]
    pub const fn confirmation_code(self) -> Option<u32> {
        self.confirmation_code
    }

    #[must_use]
    pub const fn expires_at(self) -> Option<InstantMillis> {
        self.expires_at
    }

    #[must_use]
    pub const fn failure(self) -> Option<RemoteControlTargetPairingFailure> {
        self.failure
    }

    #[must_use]
    pub const fn stable_announcement(self) -> StableTargetAnnouncementStatus {
        self.stable_announcement
    }

    #[must_use]
    pub const fn revision(self) -> u32 {
        self.revision
    }

    pub fn begin_opening(&mut self) -> RemoteControlTargetPairingUpdate {
        self.phase = RemoteControlTargetPairingPhase::Opening;
        self.attempt_id = None;
        self.invitation_code = None;
        self.confirmation_code = None;
        self.controller_committed = false;
        self.expires_at = None;
        self.failure = None;
        self.stable_announcement = StableTargetAnnouncementStatus::Idle;
        self.changed()
    }

    pub fn opened(
        &mut self,
        invitation_code: u32,
        expires_at: InstantMillis,
    ) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        self.phase = RemoteControlTargetPairingPhase::Invitation;
        self.invitation_code = Some(invitation_code);
        self.controller_committed = false;
        self.expires_at = Some(expires_at);
        self.failure = None;
        self.changed()
    }

    pub fn confirmation_required(
        &mut self,
        attempt_id: Attempt,
        confirmation_code: u32,
        expires_at: InstantMillis,
    ) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        if self.attempt_id.is_some_and(|active| active != attempt_id) {
            return self.fail_and_close(RemoteControlTargetPairingFailure::Correlation);
        }
        self.phase = RemoteControlTargetPairingPhase::Confirmation;
        self.attempt_id = Some(attempt_id);
        self.confirmation_code = Some(confirmation_code);
        self.controller_committed = false;
        self.expires_at = Some(expires_at);
        self.failure = None;
        self.changed()
    }

    pub fn awaiting_controller_commit(
        &mut self,
        attempt_id: Attempt,
    ) -> RemoteControlTargetPairingUpdate {
        let phase = if self.controller_committed {
            RemoteControlTargetPairingPhase::Authorizing
        } else {
            RemoteControlTargetPairingPhase::AwaitingControllerCommit
        };
        self.transition_attempt(attempt_id, phase)
    }

    pub fn controller_committed(
        &mut self,
        attempt_id: Attempt,
    ) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        if !self.correlates(Some(attempt_id)) {
            return RemoteControlTargetPairingUpdate::StaleAttempt;
        }
        if self.attempt_id.is_none() {
            self.attempt_id = Some(attempt_id);
        }
        self.controller_committed = true;
        if self.phase == RemoteControlTargetPairingPhase::AwaitingControllerCommit {
            self.phase = RemoteControlTargetPairingPhase::Authorizing;
        }
        self.changed()
    }

    pub fn authorizing(&mut self, attempt_id: Attempt) -> RemoteControlTargetPairingUpdate {
        self.transition_attempt(attempt_id, RemoteControlTargetPairingPhase::Authorizing)
    }

    pub fn persisted(&mut self, attempt_id: Attempt) -> RemoteControlTargetPairingUpdate {
        self.transition_attempt(attempt_id, RemoteControlTargetPairingPhase::Persisted)
    }

    pub fn rejected(&mut self, attempt_id: Attempt) -> RemoteControlTargetPairingUpdate {
        self.transition_attempt(attempt_id, RemoteControlTargetPairingPhase::Rejected)
    }

    pub fn expired(&mut self, attempt_id: Option<Attempt>) -> RemoteControlTargetPairingUpdate {
        self.transition_optional_attempt(attempt_id, RemoteControlTargetPairingPhase::Expired)
    }

    pub fn cancelled(&mut self) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        self.phase = RemoteControlTargetPairingPhase::Cancelled;
        self.failure = None;
        self.changed()
    }

    pub fn operation_failed(
        &mut self,
        attempt_id: Option<Attempt>,
        failure: RemoteControlTargetPairingFailure,
    ) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        if !self.correlates(attempt_id) {
            return RemoteControlTargetPairingUpdate::StaleAttempt;
        }
        if self.attempt_id.is_none() {
            self.attempt_id = attempt_id;
        }
        self.phase = RemoteControlTargetPairingPhase::Failed;
        self.failure = Some(failure);
        self.changed()
    }

    pub fn terminal_link_closed(
        &mut self,
        attempt_id: Attempt,
    ) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        self.operation_failed(
            Some(attempt_id),
            RemoteControlTargetPairingFailure::LinkClosed,
        )
    }

    pub fn completion_expired(&mut self, attempt_id: Attempt) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        self.operation_failed(
            Some(attempt_id),
            RemoteControlTargetPairingFailure::CompletionExpired,
        )
    }

    pub fn stable_announcement_settled(
        &mut self,
        succeeded: bool,
    ) -> RemoteControlTargetPairingUpdate {
        if !succeeded {
            self.stable_announcement = StableTargetAnnouncementStatus::Failed;
        } else if self.stable_announcement != StableTargetAnnouncementStatus::Failed {
            self.stable_announcement = StableTargetAnnouncementStatus::Succeeded;
        }
        self.changed()
    }

    pub fn stable_announcement_started(&mut self) -> RemoteControlTargetPairingUpdate {
        self.stable_announcement = StableTargetAnnouncementStatus::Idle;
        self.changed()
    }

    fn transition_attempt(
        &mut self,
        attempt_id: Attempt,
        phase: RemoteControlTargetPairingPhase,
    ) -> RemoteControlTargetPairingUpdate {
        self.transition_optional_attempt(Some(attempt_id), phase)
    }

    fn transition_optional_attempt(
        &mut self,
        attempt_id: Option<Attempt>,
        phase: RemoteControlTargetPairingPhase,
    ) -> RemoteControlTargetPairingUpdate {
        if self.phase.is_terminal() {
            return RemoteControlTargetPairingUpdate::PreservedTerminal;
        }
        if !self.correlates(attempt_id) {
            return RemoteControlTargetPairingUpdate::StaleAttempt;
        }
        if self.attempt_id.is_none() {
            self.attempt_id = attempt_id;
        }
        self.phase = phase;
        self.failure = None;
        self.changed()
    }

    fn correlates(&self, attempt_id: Option<Attempt>) -> bool {
        match (self.attempt_id, attempt_id) {
            (Some(active), Some(observed)) => active == observed,
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    fn fail_and_close(
        &mut self,
        failure: RemoteControlTargetPairingFailure,
    ) -> RemoteControlTargetPairingUpdate {
        self.phase = RemoteControlTargetPairingPhase::Failed;
        self.failure = Some(failure);
        self.bump_revision();
        RemoteControlTargetPairingUpdate::ClosePairingWindow
    }

    fn changed(&mut self) -> RemoteControlTargetPairingUpdate {
        self.bump_revision();
        RemoteControlTargetPairingUpdate::Changed
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl<Attempt> Default for RemoteControlTargetPairingState<Attempt>
where
    Attempt: Copy + Eq,
{
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteControlEventHandoff<Attempt = RemoteControlPairingAttemptId>
where
    Attempt: Copy + Eq,
{
    state: RemoteControlTargetPairingState<Attempt>,
    wake_pending: bool,
}

impl<Attempt> RemoteControlEventHandoff<Attempt>
where
    Attempt: Copy + Eq,
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: RemoteControlTargetPairingState::new(),
            wake_pending: false,
        }
    }

    pub fn update(
        &mut self,
        transition: impl FnOnce(
            &mut RemoteControlTargetPairingState<Attempt>,
        ) -> RemoteControlTargetPairingUpdate,
    ) -> RemoteControlTargetPairingUpdate {
        let update = transition(&mut self.state);
        if update.wakes_ui() {
            self.wake_pending = true;
        }
        update
    }

    #[must_use]
    pub const fn current(&self) -> RemoteControlTargetPairingState<Attempt> {
        self.state
    }

    #[must_use]
    pub const fn wake_pending(&self) -> bool {
        self.wake_pending
    }

    pub fn take_current(&mut self) -> RemoteControlTargetPairingState<Attempt> {
        self.wake_pending = false;
        self.state
    }
}

impl<Attempt> Default for RemoteControlEventHandoff<Attempt>
where
    Attempt: Copy + Eq,
{
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StableTargetAnnouncementAction {
    AutomaticStableTarget { attempt: u8 },
    ManualNodePage,
    ManualStableTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AutomaticBurst {
    started_at_millis: u64,
    next_attempt: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManualAnnouncementStep {
    NodePage,
    StableTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableTargetAnnouncer {
    transmit_ready: bool,
    automatic_trigger_pending: bool,
    automatic_burst: Option<AutomaticBurst>,
    manual_request_pending: bool,
    manual_step: Option<ManualAnnouncementStep>,
    in_flight: Option<StableTargetAnnouncementAction>,
}

impl StableTargetAnnouncer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            transmit_ready: false,
            automatic_trigger_pending: false,
            automatic_burst: None,
            manual_request_pending: false,
            manual_step: None,
            in_flight: None,
        }
    }

    pub fn trigger_automatic(&mut self) {
        self.automatic_trigger_pending = true;
    }

    pub fn request_manual(&mut self) {
        self.manual_request_pending = true;
    }

    pub fn set_transmit_ready(&mut self, ready: bool) {
        if self.transmit_ready == ready {
            return;
        }
        self.transmit_ready = ready;
        if !ready && self.automatic_burst.take().is_some() {
            self.automatic_trigger_pending = true;
        }
    }

    #[must_use]
    pub fn poll(&mut self, now_millis: u64) -> Option<StableTargetAnnouncementAction> {
        if self.in_flight.is_some() {
            return None;
        }

        if self.manual_step.is_none() && self.manual_request_pending {
            self.manual_request_pending = false;
            self.manual_step = Some(ManualAnnouncementStep::NodePage);
        }
        if let Some(step) = self.manual_step {
            let action = match step {
                ManualAnnouncementStep::NodePage => StableTargetAnnouncementAction::ManualNodePage,
                ManualAnnouncementStep::StableTarget => {
                    StableTargetAnnouncementAction::ManualStableTarget
                }
            };
            self.in_flight = Some(action);
            return Some(action);
        }

        if !self.transmit_ready {
            return None;
        }

        if self.automatic_burst.is_none() && self.automatic_trigger_pending {
            self.automatic_trigger_pending = false;
            self.automatic_burst = Some(AutomaticBurst {
                started_at_millis: now_millis,
                next_attempt: 0,
            });
        }
        let burst = self.automatic_burst?;
        let offset = STABLE_TARGET_ANNOUNCE_OFFSETS_MILLIS[usize::from(burst.next_attempt)];
        if now_millis < burst.started_at_millis.saturating_add(offset) {
            return None;
        }
        let action = StableTargetAnnouncementAction::AutomaticStableTarget {
            attempt: burst.next_attempt + 1,
        };
        self.in_flight = Some(action);
        Some(action)
    }

    pub fn settle(&mut self, action: StableTargetAnnouncementAction) -> bool {
        if self.in_flight != Some(action) {
            return false;
        }
        self.in_flight = None;
        match action {
            StableTargetAnnouncementAction::ManualNodePage => {
                self.manual_step = Some(ManualAnnouncementStep::StableTarget);
            }
            StableTargetAnnouncementAction::ManualStableTarget => {
                self.manual_step = None;
            }
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt } => {
                if let Some(burst) = &mut self.automatic_burst {
                    if attempt == burst.next_attempt + 1 {
                        burst.next_attempt += 1;
                        if usize::from(burst.next_attempt)
                            == STABLE_TARGET_ANNOUNCE_OFFSETS_MILLIS.len()
                        {
                            self.automatic_burst = None;
                        }
                    }
                }
            }
        }
        true
    }

    #[must_use]
    pub fn next_deadline_millis(&self) -> Option<u64> {
        if !self.transmit_ready
            || self.in_flight.is_some()
            || self.manual_request_pending
            || self.manual_step.is_some()
        {
            return None;
        }
        let burst = self.automatic_burst?;
        Some(
            burst.started_at_millis.saturating_add(
                STABLE_TARGET_ANNOUNCE_OFFSETS_MILLIS[usize::from(burst.next_attempt)],
            ),
        )
    }

    #[must_use]
    pub const fn is_idle(&self) -> bool {
        !self.automatic_trigger_pending
            && self.automatic_burst.is_none()
            && !self.manual_request_pending
            && self.manual_step.is_none()
            && self.in_flight.is_none()
    }
}

impl Default for StableTargetAnnouncer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_burst_keeps_the_latest_authoritative_state_behind_one_pending_wake() {
        let id = 0xA5_u8;
        let mut handoff = RemoteControlEventHandoff::<u8>::new();
        handoff.update(RemoteControlTargetPairingState::begin_opening);
        handoff.update(|state| state.opened(0x1234_5678, InstantMillis(60_000)));
        handoff.update(|state| state.confirmation_required(id, 123_456, InstantMillis(30_000)));
        handoff.update(|state| state.awaiting_controller_commit(id));
        handoff.update(|state| state.authorizing(id));
        handoff.update(|state| state.persisted(id));

        assert!(handoff.wake_pending());
        let current = handoff.take_current();
        assert_eq!(current.phase(), RemoteControlTargetPairingPhase::Persisted);
        assert_eq!(current.attempt_id(), Some(id));
        assert!(!handoff.wake_pending());
    }

    #[test]
    fn controller_commit_before_the_local_decision_keeps_confirmation_actionable() {
        let id = 0xA5_u8;
        let mut state = RemoteControlTargetPairingState::<u8>::new();
        state.begin_opening();
        state.confirmation_required(id, 123_456, InstantMillis(30_000));

        assert_eq!(
            state.controller_committed(id),
            RemoteControlTargetPairingUpdate::Changed
        );
        assert_eq!(state.phase(), RemoteControlTargetPairingPhase::Confirmation);

        state.awaiting_controller_commit(id);
        assert_eq!(state.phase(), RemoteControlTargetPairingPhase::Authorizing);

        let mut target_first = RemoteControlTargetPairingState::<u8>::new();
        target_first.begin_opening();
        target_first.confirmation_required(id, 123_456, InstantMillis(30_000));
        target_first.awaiting_controller_commit(id);
        target_first.controller_committed(id);
        assert_eq!(
            target_first.phase(),
            RemoteControlTargetPairingPhase::Authorizing
        );
    }

    #[test]
    fn exact_attempt_correlation_fails_closed_and_terminal_state_is_preserved() {
        let first = 1_u8;
        let other = 2_u8;
        let mut state = RemoteControlTargetPairingState::<u8>::new();
        state.begin_opening();
        state.confirmation_required(first, 111_111, InstantMillis(30_000));

        assert_eq!(
            state.confirmation_required(other, 222_222, InstantMillis(30_000)),
            RemoteControlTargetPairingUpdate::ClosePairingWindow
        );
        assert_eq!(state.phase(), RemoteControlTargetPairingPhase::Failed);
        assert_eq!(
            state.failure(),
            Some(RemoteControlTargetPairingFailure::Correlation)
        );
        assert_eq!(
            state.persisted(first),
            RemoteControlTargetPairingUpdate::PreservedTerminal
        );
        assert_eq!(
            state.cancelled(),
            RemoteControlTargetPairingUpdate::PreservedTerminal
        );
        assert_eq!(
            state.operation_failed(Some(first), RemoteControlTargetPairingFailure::Persistence,),
            RemoteControlTargetPairingUpdate::PreservedTerminal
        );
        assert_eq!(state.phase(), RemoteControlTargetPairingPhase::Failed);
        assert_eq!(
            state.failure(),
            Some(RemoteControlTargetPairingFailure::Correlation)
        );
    }

    #[test]
    fn automatic_announces_use_exact_offsets_continue_after_failure_and_stop_at_three() {
        let mut announcer = StableTargetAnnouncer::new();
        announcer.set_transmit_ready(true);
        announcer.trigger_automatic();
        announcer.trigger_automatic();

        let first = announcer.poll(10_000).expect("first attempt is immediate");
        assert_eq!(
            first,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 1 }
        );
        assert!(announcer.settle(first));
        assert_eq!(announcer.next_deadline_millis(), Some(12_000));
        assert_eq!(announcer.poll(11_999), None);

        let second = announcer
            .poll(12_000)
            .expect("second attempt is at two seconds");
        assert_eq!(
            second,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 2 }
        );
        // The state machine advances after any typed settlement, including failure.
        assert!(announcer.settle(second));
        assert_eq!(announcer.next_deadline_millis(), Some(18_000));

        let third = announcer
            .poll(18_000)
            .expect("third attempt is at eight seconds");
        assert_eq!(
            third,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 3 }
        );
        assert!(announcer.settle(third));
        assert_eq!(announcer.poll(u64::MAX), None);
        assert!(announcer.is_idle());
    }

    #[test]
    fn loss_of_egress_pauses_and_ready_transition_rearms_a_fresh_bounded_burst() {
        let mut announcer = StableTargetAnnouncer::new();
        announcer.trigger_automatic();
        assert_eq!(announcer.poll(0), None);

        announcer.set_transmit_ready(true);
        let first = announcer
            .poll(5_000)
            .expect("ready transition starts burst");
        assert!(announcer.settle(first));
        announcer.set_transmit_ready(false);
        assert_eq!(announcer.poll(20_000), None);

        announcer.set_transmit_ready(true);
        let restarted = announcer.poll(25_000).expect("readiness rearms the burst");
        assert_eq!(
            restarted,
            StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 1 }
        );
        assert!(announcer.settle(restarted));
        assert_eq!(announcer.next_deadline_millis(), Some(27_000));
    }

    #[test]
    fn deliberate_dual_announce_serializes_before_an_automatic_burst() {
        let mut announcer = StableTargetAnnouncer::new();
        announcer.set_transmit_ready(true);
        announcer.trigger_automatic();
        announcer.request_manual();

        let node = announcer
            .poll(1_000)
            .expect("manual node announce starts first");
        assert_eq!(node, StableTargetAnnouncementAction::ManualNodePage);
        assert_eq!(announcer.poll(1_000), None);
        assert!(announcer.settle(node));

        let stable = announcer
            .poll(1_000)
            .expect("manual stable announce follows");
        assert_eq!(stable, StableTargetAnnouncementAction::ManualStableTarget);
        assert!(announcer.settle(stable));

        assert_eq!(
            announcer.poll(1_000),
            Some(StableTargetAnnouncementAction::AutomaticStableTarget { attempt: 1 })
        );
    }
}
