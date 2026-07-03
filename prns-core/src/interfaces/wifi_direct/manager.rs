use super::core::{GoIntent, GroupRole, Initiative, GO_MAX_CLIENTS};
use super::seam::{Availability, DiscoveryMode};
use crate::interfaces::MacAddress;

pub const FORMATION_TIMEOUT_MS: u64 = 30_000;
pub const FORM_RETRY_TTL_MS: u64 = 20_000;
pub const SUPPRESS_TTL_MS: u64 = 12_000;

#[derive(Debug, Clone, Copy)]
pub enum ManagerInput {
    Sighting {
        peer: MacAddress,
        initiative: Initiative,
        now_ms: u64,
    },
    Invitation {
        peer: MacAddress,
        now_ms: u64,
    },
    GroupFormed {
        role: GroupRole,
        now_ms: u64,
    },
    FormationFailed {
        peer: MacAddress,
        now_ms: u64,
    },
    FormationProgress {
        now_ms: u64,
    },
    GroupLost {
        now_ms: u64,
    },
    MembersChanged {
        count: usize,
    },
    AvailabilityChanged {
        state: Availability,
        now_ms: u64,
    },
    Tick {
        now_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerAction {
    SetDiscovery(DiscoveryMode),
    Form { peer: MacAddress, intent: GoIntent },
    Accept { peer: MacAddress },
    RemoveGroup,
    OpenDataPlane { role: GroupRole },
    CloseDataPlane,
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Forming {
        peer: MacAddress,
        since_ms: u64,
    },
    Grouped {
        role: GroupRole,
        formed_with: Option<MacAddress>,
    },
    Parked {
        reason: &'static str,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackoffKind {
    Forming,
    Suppressed,
}

#[derive(Clone, Copy)]
struct Backoff {
    peer: MacAddress,
    kind: BackoffKind,
    since_ms: u64,
}

impl Backoff {
    fn ttl_ms(self) -> u64 {
        match self.kind {
            BackoffKind::Forming => FORM_RETRY_TTL_MS,
            BackoffKind::Suppressed => SUPPRESS_TTL_MS,
        }
    }

    fn elapsed(self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.since_ms) >= self.ttl_ms()
    }
}

pub struct GroupPolicy<const DIAL_TRACK: usize> {
    intent: GoIntent,
    phase: Phase,
    backoff: [Option<Backoff>; DIAL_TRACK],
    discovery: bool,
    members: usize,
}

impl<const DIAL_TRACK: usize> GroupPolicy<DIAL_TRACK> {
    #[must_use]
    pub const fn new(intent: GoIntent) -> Self {
        Self {
            intent,
            phase: Phase::Idle,
            backoff: [None; DIAL_TRACK],
            discovery: false,
            members: 0,
        }
    }

    pub fn start<F: FnMut(ManagerAction)>(&mut self, emit: &mut F) {
        self.reconcile_discovery(emit);
    }

    #[must_use]
    pub fn role(&self) -> Option<GroupRole> {
        match self.phase {
            Phase::Grouped { role, .. } => Some(role),
            Phase::Idle | Phase::Forming { .. } | Phase::Parked { .. } => None,
        }
    }

    #[must_use]
    pub fn phase_reason(&self) -> Option<&'static str> {
        match self.phase {
            Phase::Parked { reason } => Some(reason),
            Phase::Idle | Phase::Forming { .. } | Phase::Grouped { .. } => None,
        }
    }

    pub fn handle<F: FnMut(ManagerAction)>(&mut self, input: ManagerInput, emit: &mut F) {
        match input {
            ManagerInput::Sighting {
                peer,
                initiative,
                now_ms,
            } => self.on_sighting(peer, initiative, now_ms, emit),
            ManagerInput::Invitation { peer, now_ms } => self.on_invitation(peer, now_ms, emit),
            ManagerInput::GroupFormed { role, now_ms } => self.on_group_formed(role, now_ms, emit),
            ManagerInput::FormationFailed { peer, now_ms } => {
                self.on_formation_failed(peer, now_ms, emit);
            }
            ManagerInput::FormationProgress { now_ms } => self.on_formation_progress(now_ms),
            ManagerInput::GroupLost { now_ms } => self.on_group_lost(now_ms, emit),
            ManagerInput::MembersChanged { count } => self.on_members_changed(count, emit),
            ManagerInput::AvailabilityChanged { state, now_ms } => {
                self.on_availability(state, now_ms, emit);
            }
            ManagerInput::Tick { now_ms } => self.on_tick(now_ms, emit),
        }
    }

    fn on_sighting<F: FnMut(ManagerAction)>(
        &mut self,
        peer: MacAddress,
        initiative: Initiative,
        now_ms: u64,
        emit: &mut F,
    ) {
        let formable = matches!(initiative, Initiative::Ours)
            && matches!(self.phase, Phase::Idle)
            && self.backoff_ready(peer, now_ms);
        if !formable {
            return;
        }
        self.phase = Phase::Forming {
            peer,
            since_ms: now_ms,
        };
        self.upsert_backoff(peer, BackoffKind::Forming, now_ms);
        emit(ManagerAction::Form {
            peer,
            intent: self.intent,
        });
        self.reconcile_discovery(emit);
    }

    fn on_invitation<F: FnMut(ManagerAction)>(
        &mut self,
        peer: MacAddress,
        now_ms: u64,
        emit: &mut F,
    ) {
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Forming {
                    peer,
                    since_ms: now_ms,
                };
                self.upsert_backoff(peer, BackoffKind::Forming, now_ms);
                emit(ManagerAction::Accept { peer });
                self.reconcile_discovery(emit);
            }
            Phase::Forming { peer: current, .. } => {
                if current == peer {
                    emit(ManagerAction::Accept { peer });
                }
            }
            Phase::Grouped { role, .. } => {
                let joinable = matches!(role, GroupRole::Owner) && self.members < GO_MAX_CLIENTS;
                if joinable {
                    emit(ManagerAction::Accept { peer });
                }
            }
            Phase::Parked { .. } => {}
        }
    }

    fn on_group_formed<F: FnMut(ManagerAction)>(
        &mut self,
        role: GroupRole,
        _now_ms: u64,
        emit: &mut F,
    ) {
        let formed_with = match self.phase {
            Phase::Forming { peer, .. } => {
                self.clear_backoff(peer);
                Some(peer)
            }
            Phase::Idle => None,
            Phase::Grouped { .. } | Phase::Parked { .. } => return,
        };
        self.phase = Phase::Grouped { role, formed_with };
        self.members = 0;
        emit(ManagerAction::OpenDataPlane { role });
        self.reconcile_discovery(emit);
    }

    fn on_formation_progress(&mut self, now_ms: u64) {
        if let Phase::Forming { peer, .. } = self.phase {
            self.phase = Phase::Forming {
                peer,
                since_ms: now_ms,
            };
            self.upsert_backoff(peer, BackoffKind::Forming, now_ms);
        }
    }

    fn on_formation_failed<F: FnMut(ManagerAction)>(
        &mut self,
        peer: MacAddress,
        now_ms: u64,
        emit: &mut F,
    ) {
        self.upsert_backoff(peer, BackoffKind::Suppressed, now_ms);
        if matches!(self.phase, Phase::Forming { peer: current, .. } if current == peer) {
            self.phase = Phase::Idle;
            self.reconcile_discovery(emit);
        }
    }

    fn on_group_lost<F: FnMut(ManagerAction)>(&mut self, now_ms: u64, emit: &mut F) {
        match self.phase {
            Phase::Grouped { formed_with, .. } => {
                if let Some(peer) = formed_with {
                    self.upsert_backoff(peer, BackoffKind::Suppressed, now_ms);
                }
                self.phase = Phase::Idle;
                self.members = 0;
                emit(ManagerAction::CloseDataPlane);
                self.reconcile_discovery(emit);
            }
            Phase::Forming { peer, .. } => {
                self.upsert_backoff(peer, BackoffKind::Suppressed, now_ms);
                self.phase = Phase::Idle;
                self.reconcile_discovery(emit);
            }
            Phase::Idle | Phase::Parked { .. } => {}
        }
    }

    fn on_members_changed<F: FnMut(ManagerAction)>(&mut self, count: usize, emit: &mut F) {
        self.members = count;
        self.reconcile_discovery(emit);
    }

    fn on_availability<F: FnMut(ManagerAction)>(
        &mut self,
        state: Availability,
        _now_ms: u64,
        emit: &mut F,
    ) {
        match state {
            Availability::Unavailable(reason) => {
                match self.phase {
                    Phase::Grouped { .. } => {
                        emit(ManagerAction::RemoveGroup);
                        emit(ManagerAction::CloseDataPlane);
                    }
                    Phase::Forming { .. } => emit(ManagerAction::RemoveGroup),
                    Phase::Idle | Phase::Parked { .. } => {}
                }
                self.phase = Phase::Parked { reason };
                self.members = 0;
                self.reconcile_discovery(emit);
            }
            Availability::Available => {
                if matches!(self.phase, Phase::Parked { .. }) {
                    self.phase = Phase::Idle;
                    self.reconcile_discovery(emit);
                }
            }
        }
    }

    fn on_tick<F: FnMut(ManagerAction)>(&mut self, now_ms: u64, emit: &mut F) {
        let Phase::Forming { peer, since_ms } = self.phase else {
            return;
        };
        if now_ms.saturating_sub(since_ms) < FORMATION_TIMEOUT_MS {
            return;
        }
        emit(ManagerAction::RemoveGroup);
        self.upsert_backoff(peer, BackoffKind::Suppressed, now_ms);
        self.phase = Phase::Idle;
        self.reconcile_discovery(emit);
    }

    fn reconcile_discovery<F: FnMut(ManagerAction)>(&mut self, emit: &mut F) {
        let want = match self.phase {
            Phase::Idle => true,
            Phase::Forming { .. } | Phase::Parked { .. } => false,
            Phase::Grouped { role, .. } => match role {
                GroupRole::Owner => self.members > 0 && self.members < GO_MAX_CLIENTS,
                GroupRole::Client => false,
            },
        };
        if want != self.discovery {
            self.discovery = want;
            emit(ManagerAction::SetDiscovery(if want {
                DiscoveryMode::On
            } else {
                DiscoveryMode::Off
            }));
        }
    }

    fn backoff_ready(&self, peer: MacAddress, now_ms: u64) -> bool {
        match self.find_backoff(peer) {
            Some(index) => self.backoff[index].is_none_or(|backoff| backoff.elapsed(now_ms)),
            None => true,
        }
    }

    fn find_backoff(&self, peer: MacAddress) -> Option<usize> {
        self.backoff
            .iter()
            .position(|entry| entry.is_some_and(|b| b.peer == peer))
    }

    fn clear_backoff(&mut self, peer: MacAddress) {
        if let Some(index) = self.find_backoff(peer) {
            self.backoff[index] = None;
        }
    }

    fn upsert_backoff(&mut self, peer: MacAddress, kind: BackoffKind, now_ms: u64) {
        let entry = Backoff {
            peer,
            kind,
            since_ms: now_ms,
        };
        if let Some(index) = self.find_backoff(peer) {
            self.backoff[index] = Some(entry);
            return;
        }
        if let Some(index) = self.backoff.iter().position(Option::is_none) {
            self.backoff[index] = Some(entry);
            return;
        }
        self.prune_backoff(now_ms);
        let slot = self.backoff.iter().position(Option::is_none).or_else(|| {
            self.backoff
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.map_or(u64::MAX, |b| b.since_ms))
                .map(|(index, _)| index)
        });
        if let Some(index) = slot {
            self.backoff[index] = Some(entry);
        }
    }

    fn prune_backoff(&mut self, now_ms: u64) {
        for entry in &mut self.backoff {
            if entry.is_some_and(|b| b.elapsed(now_ms)) {
                *entry = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(byte: u8) -> MacAddress {
        MacAddress::new([byte; 6])
    }

    fn started() -> GroupPolicy<8> {
        let mut policy = GroupPolicy::new(GoIntent::BALANCED);
        policy.start(&mut |_| {});
        policy
    }

    fn collect<const D: usize>(
        policy: &mut GroupPolicy<D>,
        input: ManagerInput,
    ) -> std::vec::Vec<ManagerAction> {
        let mut actions = std::vec::Vec::new();
        policy.handle(input, &mut |action| actions.push(action));
        actions
    }

    fn formed(policy: &mut GroupPolicy<8>, peer: u8, role: GroupRole) {
        policy.handle(
            ManagerInput::Sighting {
                peer: addr(peer),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
            &mut |_| {},
        );
        policy.handle(ManagerInput::GroupFormed { role, now_ms: 100 }, &mut |_| {});
    }

    #[test]
    fn start_turns_discovery_on() {
        let mut policy = GroupPolicy::<8>::new(GoIntent::BALANCED);
        let mut actions = std::vec::Vec::new();
        policy.start(&mut |action| actions.push(action));
        assert_eq!(
            actions,
            std::vec![ManagerAction::SetDiscovery(DiscoveryMode::On)]
        );
    }

    #[test]
    fn a_sighting_forms_and_focuses_the_radio() {
        let mut policy = started();
        let actions = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );
        assert_eq!(
            actions,
            std::vec![
                ManagerAction::Form {
                    peer: addr(9),
                    intent: GoIntent::BALANCED
                },
                ManagerAction::SetDiscovery(DiscoveryMode::Off)
            ]
        );
    }

    #[test]
    fn a_sighting_without_the_initiative_never_forms() {
        let mut policy = started();
        let actions = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Theirs,
                now_ms: 0,
            },
        );
        assert!(actions.is_empty());

        let invited = collect(
            &mut policy,
            ManagerInput::Invitation {
                peer: addr(9),
                now_ms: 500,
            },
        );
        assert!(matches!(
            invited.first(),
            Some(ManagerAction::Accept { .. })
        ));
    }

    #[test]
    fn sightings_while_forming_are_ignored() {
        let mut policy = started();
        collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );
        for peer in [9, 5] {
            let actions = collect(
                &mut policy,
                ManagerInput::Sighting {
                    peer: addr(peer),
                    initiative: Initiative::Ours,
                    now_ms: 1_000,
                },
            );
            assert!(actions.is_empty());
        }
    }

    #[test]
    fn a_hung_formation_is_abandoned_and_suppressed() {
        let mut policy = started();
        collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );

        let early = collect(
            &mut policy,
            ManagerInput::Tick {
                now_ms: FORMATION_TIMEOUT_MS - 1,
            },
        );
        assert!(early.is_empty());

        let abandoned = collect(
            &mut policy,
            ManagerInput::Tick {
                now_ms: FORMATION_TIMEOUT_MS,
            },
        );
        assert_eq!(
            abandoned,
            std::vec![
                ManagerAction::RemoveGroup,
                ManagerAction::SetDiscovery(DiscoveryMode::On)
            ]
        );

        let suppressed = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: FORMATION_TIMEOUT_MS + 1_000,
            },
        );
        assert!(suppressed.is_empty());

        let after = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: FORMATION_TIMEOUT_MS + SUPPRESS_TTL_MS,
            },
        );
        assert_eq!(
            after,
            std::vec![
                ManagerAction::Form {
                    peer: addr(9),
                    intent: GoIntent::BALANCED
                },
                ManagerAction::SetDiscovery(DiscoveryMode::Off)
            ]
        );
    }

    #[test]
    fn a_failed_formation_backs_off_before_reforming() {
        let mut policy = started();
        collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );

        let failed = collect(
            &mut policy,
            ManagerInput::FormationFailed {
                peer: addr(9),
                now_ms: 2_000,
            },
        );
        assert_eq!(
            failed,
            std::vec![ManagerAction::SetDiscovery(DiscoveryMode::On)]
        );

        let suppressed = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 3_000,
            },
        );
        assert!(suppressed.is_empty());

        let after = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 2_000 + SUPPRESS_TTL_MS,
            },
        );
        assert!(matches!(after.first(), Some(ManagerAction::Form { .. })));
    }

    #[test]
    fn formation_progress_rearms_the_timeout() {
        let mut policy = started();
        collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );
        collect(
            &mut policy,
            ManagerInput::FormationProgress {
                now_ms: FORMATION_TIMEOUT_MS - 1_000,
            },
        );
        let after_original_deadline = collect(
            &mut policy,
            ManagerInput::Tick {
                now_ms: FORMATION_TIMEOUT_MS + 1_000,
            },
        );
        assert!(after_original_deadline.is_empty());

        let after_rearmed_deadline = collect(
            &mut policy,
            ManagerInput::Tick {
                now_ms: (FORMATION_TIMEOUT_MS - 1_000) + FORMATION_TIMEOUT_MS,
            },
        );
        assert!(matches!(
            after_rearmed_deadline.first(),
            Some(ManagerAction::RemoveGroup)
        ));
    }

    #[test]
    fn a_crossed_invitation_is_accepted_and_a_foreign_one_ignored() {
        let mut policy = started();
        collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );
        let crossed = collect(
            &mut policy,
            ManagerInput::Invitation {
                peer: addr(9),
                now_ms: 500,
            },
        );
        assert_eq!(crossed, std::vec![ManagerAction::Accept { peer: addr(9) }]);

        let foreign = collect(
            &mut policy,
            ManagerInput::Invitation {
                peer: addr(5),
                now_ms: 600,
            },
        );
        assert!(foreign.is_empty());
    }

    #[test]
    fn an_idle_invitation_is_accepted() {
        let mut policy = started();
        let actions = collect(
            &mut policy,
            ManagerInput::Invitation {
                peer: addr(9),
                now_ms: 0,
            },
        );
        assert_eq!(
            actions,
            std::vec![
                ManagerAction::Accept { peer: addr(9) },
                ManagerAction::SetDiscovery(DiscoveryMode::Off)
            ]
        );
    }

    #[test]
    fn an_owner_stays_quiet_until_its_first_member_then_listens_while_slots_are_free() {
        let mut policy = started();
        collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );

        let opened = collect(
            &mut policy,
            ManagerInput::GroupFormed {
                role: GroupRole::Owner,
                now_ms: 100,
            },
        );
        assert_eq!(
            opened,
            std::vec![ManagerAction::OpenDataPlane {
                role: GroupRole::Owner
            }]
        );
        assert_eq!(policy.role(), Some(GroupRole::Owner));

        let first_member = collect(&mut policy, ManagerInput::MembersChanged { count: 1 });
        assert_eq!(
            first_member,
            std::vec![ManagerAction::SetDiscovery(DiscoveryMode::On)]
        );

        let full = collect(
            &mut policy,
            ManagerInput::MembersChanged {
                count: GO_MAX_CLIENTS,
            },
        );
        assert_eq!(
            full,
            std::vec![ManagerAction::SetDiscovery(DiscoveryMode::Off)]
        );

        let freed = collect(
            &mut policy,
            ManagerInput::MembersChanged {
                count: GO_MAX_CLIENTS - 1,
            },
        );
        assert_eq!(
            freed,
            std::vec![ManagerAction::SetDiscovery(DiscoveryMode::On)]
        );
    }

    #[test]
    fn a_client_group_goes_quiet_and_a_loss_cools_the_owner_off() {
        let mut policy = started();
        collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 0,
            },
        );

        let opened = collect(
            &mut policy,
            ManagerInput::GroupFormed {
                role: GroupRole::Client,
                now_ms: 100,
            },
        );
        assert_eq!(
            opened,
            std::vec![ManagerAction::OpenDataPlane {
                role: GroupRole::Client
            }]
        );
        assert_eq!(policy.role(), Some(GroupRole::Client));

        let lost = collect(&mut policy, ManagerInput::GroupLost { now_ms: 5_000 });
        assert_eq!(
            lost,
            std::vec![
                ManagerAction::CloseDataPlane,
                ManagerAction::SetDiscovery(DiscoveryMode::On)
            ]
        );

        let cooling = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 6_000,
            },
        );
        assert!(cooling.is_empty());

        let after = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(9),
                initiative: Initiative::Ours,
                now_ms: 5_000 + SUPPRESS_TTL_MS,
            },
        );
        assert!(matches!(after.first(), Some(ManagerAction::Form { .. })));
    }

    #[test]
    fn revocation_parks_and_restoration_reconciles() {
        let mut policy = started();
        formed(&mut policy, 9, GroupRole::Owner);
        collect(&mut policy, ManagerInput::MembersChanged { count: 1 });

        let parked = collect(
            &mut policy,
            ManagerInput::AvailabilityChanged {
                state: Availability::Unavailable("Wi-Fi P2P disabled by the platform"),
                now_ms: 1_000,
            },
        );
        assert_eq!(
            parked,
            std::vec![
                ManagerAction::RemoveGroup,
                ManagerAction::CloseDataPlane,
                ManagerAction::SetDiscovery(DiscoveryMode::Off)
            ]
        );
        assert_eq!(
            policy.phase_reason(),
            Some("Wi-Fi P2P disabled by the platform")
        );

        let while_parked = collect(
            &mut policy,
            ManagerInput::Sighting {
                peer: addr(5),
                initiative: Initiative::Ours,
                now_ms: 2_000,
            },
        );
        assert!(while_parked.is_empty());

        let restored = collect(
            &mut policy,
            ManagerInput::AvailabilityChanged {
                state: Availability::Available,
                now_ms: 3_000,
            },
        );
        assert_eq!(
            restored,
            std::vec![ManagerAction::SetDiscovery(DiscoveryMode::On)]
        );
        assert_eq!(policy.phase_reason(), None);
    }
}
