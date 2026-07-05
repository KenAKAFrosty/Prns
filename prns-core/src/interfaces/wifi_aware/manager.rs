use super::core::{is_keeper, NdpRole, RendezvousToken};
use super::seam::{Availability, DiscoveryMode};

pub const NDP_TIMEOUT_MS: u64 = 15_000;
pub const SUPPRESS_TTL_MS: u64 = 12_000;

#[derive(Debug, Clone, Copy)]
pub enum ManagerInput {
    PeerDiscovered {
        peer: RendezvousToken,
        now_ms: u64,
    },
    NdpRequested {
        peer: RendezvousToken,
        now_ms: u64,
    },
    DataPathUp {
        peer: RendezvousToken,
        role: NdpRole,
        now_ms: u64,
    },
    DataPathDown {
        peer: RendezvousToken,
        role: NdpRole,
        now_ms: u64,
    },
    NdpFailed {
        peer: RendezvousToken,
        role: NdpRole,
        now_ms: u64,
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
    RequestDataPath {
        peer: RendezvousToken,
        role: NdpRole,
    },
    AbandonDataPath {
        peer: RendezvousToken,
        role: NdpRole,
    },
    OpenDataPlane {
        peer: RendezvousToken,
        role: NdpRole,
    },
    CloseDataPlane {
        peer: RendezvousToken,
    },
}

/// One peer's rendezvous. A symmetric both-attempt fires up to two paths — the initiator path this
/// node opened on sighting and the responder path it opened for the peer's request — so the session
/// remembers which paths it has fired and which one (if any) it has admitted as the live member. At
/// most one path is ever the member; the keeper duel picks it when a duplicate settles.
#[derive(Clone, Copy)]
struct PeerSession {
    peer: RendezvousToken,
    fired_initiator: bool,
    fired_responder: bool,
    admitted: Option<NdpRole>,
    since_ms: u64,
}

#[derive(Clone, Copy)]
struct Suppress {
    peer: RendezvousToken,
    since_ms: u64,
}

impl Suppress {
    fn elapsed(self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.since_ms) >= SUPPRESS_TTL_MS
    }
}

/// The Wi-Fi Aware brain: unlike a Wi-Fi Direct group (one owner, one join), Aware sustains a data
/// path to each of several peers at once, so the policy tracks a bounded table of per-peer sessions
/// rather than a single phase. Every node publishes and subscribes continuously; a sighting fires an
/// attempt from both ends at once and the keeper duel dedups the survivor, and discovery stays live
/// until the table fills so the mesh keeps widening.
pub struct AwarePolicy<const PEER_TRACK: usize> {
    local: RendezvousToken,
    sessions: [Option<PeerSession>; PEER_TRACK],
    suppressed: [Option<Suppress>; PEER_TRACK],
    discovery: bool,
    parked: Option<&'static str>,
}

impl<const PEER_TRACK: usize> AwarePolicy<PEER_TRACK> {
    #[must_use]
    pub const fn new(local: RendezvousToken) -> Self {
        Self {
            local,
            sessions: [None; PEER_TRACK],
            suppressed: [None; PEER_TRACK],
            discovery: false,
            parked: None,
        }
    }

    pub fn start<F: FnMut(ManagerAction)>(&mut self, emit: &mut F) {
        self.reconcile_discovery(emit);
    }

    #[must_use]
    pub fn peer_count(&self) -> usize {
        self.sessions.iter().filter(|slot| slot.is_some()).count()
    }

    #[must_use]
    pub fn park_reason(&self) -> Option<&'static str> {
        self.parked
    }

    #[must_use]
    pub fn is_connected(&self, peer: RendezvousToken) -> bool {
        self.session_index(peer)
            .and_then(|index| self.sessions[index])
            .is_some_and(|session| session.admitted.is_some())
    }

    #[must_use]
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.sessions
            .iter()
            .filter_map(|slot| match slot {
                Some(session) if session.admitted.is_none() => {
                    Some(session.since_ms.saturating_add(NDP_TIMEOUT_MS))
                }
                _ => None,
            })
            .min()
    }

    pub fn handle<F: FnMut(ManagerAction)>(&mut self, input: ManagerInput, emit: &mut F) {
        match input {
            ManagerInput::PeerDiscovered { peer, now_ms } => {
                self.on_attempt(peer, NdpRole::Initiator, now_ms, emit);
            }
            ManagerInput::NdpRequested { peer, now_ms } => {
                self.on_attempt(peer, NdpRole::Responder, now_ms, emit);
            }
            ManagerInput::DataPathUp { peer, role, now_ms } => {
                self.on_data_path_up(peer, role, now_ms, emit);
            }
            ManagerInput::DataPathDown { peer, role, now_ms } => {
                self.on_path_lost(peer, role, now_ms, emit);
            }
            ManagerInput::NdpFailed { peer, role, now_ms } => {
                self.on_path_lost(peer, role, now_ms, emit);
            }
            ManagerInput::AvailabilityChanged { state, now_ms } => {
                self.on_availability(state, now_ms, emit);
            }
            ManagerInput::Tick { now_ms } => self.on_tick(now_ms, emit),
        }
    }

    fn on_attempt<F: FnMut(ManagerAction)>(
        &mut self,
        peer: RendezvousToken,
        role: NdpRole,
        now_ms: u64,
        emit: &mut F,
    ) {
        if !self.engageable(peer, now_ms) {
            return;
        }
        let Some(index) = self.ensure_session(peer, now_ms) else {
            return;
        };
        let Some(mut session) = self.sessions[index] else {
            return;
        };
        let already_fired = match role {
            NdpRole::Initiator => session.fired_initiator,
            NdpRole::Responder => session.fired_responder,
        };
        if already_fired {
            return;
        }
        match role {
            NdpRole::Initiator => session.fired_initiator = true,
            NdpRole::Responder => session.fired_responder = true,
        }
        self.sessions[index] = Some(session);
        emit(ManagerAction::RequestDataPath { peer, role });
        self.reconcile_discovery(emit);
    }

    fn on_data_path_up<F: FnMut(ManagerAction)>(
        &mut self,
        peer: RendezvousToken,
        role: NdpRole,
        _now_ms: u64,
        emit: &mut F,
    ) {
        let Some(index) = self.session_index(peer) else {
            return;
        };
        let Some(mut session) = self.sessions[index] else {
            return;
        };
        match session.admitted {
            None => {
                session.admitted = Some(role);
                self.sessions[index] = Some(session);
                self.clear_suppress(peer);
                emit(ManagerAction::OpenDataPlane { peer, role });
            }
            Some(incumbent) if incumbent == role => {}
            Some(incumbent) => {
                let challenger_keeps = is_keeper(role, self.local, peer);
                let incumbent_keeps = is_keeper(incumbent, self.local, peer);
                if challenger_keeps && !incumbent_keeps {
                    session.admitted = Some(role);
                    self.sessions[index] = Some(session);
                    emit(ManagerAction::CloseDataPlane { peer });
                    emit(ManagerAction::AbandonDataPath {
                        peer,
                        role: incumbent,
                    });
                    emit(ManagerAction::OpenDataPlane { peer, role });
                } else {
                    emit(ManagerAction::AbandonDataPath { peer, role });
                }
            }
        }
    }

    fn on_path_lost<F: FnMut(ManagerAction)>(
        &mut self,
        peer: RendezvousToken,
        role: NdpRole,
        now_ms: u64,
        emit: &mut F,
    ) {
        let Some(index) = self.session_index(peer) else {
            return;
        };
        let Some(session) = self.sessions[index] else {
            return;
        };
        if session.admitted != Some(role) {
            return;
        }
        let other = match role {
            NdpRole::Initiator => (NdpRole::Responder, session.fired_responder),
            NdpRole::Responder => (NdpRole::Initiator, session.fired_initiator),
        };
        self.sessions[index] = None;
        self.upsert_suppress(peer, now_ms);
        emit(ManagerAction::CloseDataPlane { peer });
        if other.1 {
            emit(ManagerAction::AbandonDataPath {
                peer,
                role: other.0,
            });
        }
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
                for index in 0..PEER_TRACK {
                    let Some(session) = self.sessions[index].take() else {
                        continue;
                    };
                    if session.admitted.is_some() {
                        emit(ManagerAction::CloseDataPlane { peer: session.peer });
                    }
                    if session.fired_initiator && session.admitted != Some(NdpRole::Initiator) {
                        emit(ManagerAction::AbandonDataPath {
                            peer: session.peer,
                            role: NdpRole::Initiator,
                        });
                    }
                    if session.fired_responder && session.admitted != Some(NdpRole::Responder) {
                        emit(ManagerAction::AbandonDataPath {
                            peer: session.peer,
                            role: NdpRole::Responder,
                        });
                    }
                }
                self.parked = Some(reason);
                self.reconcile_discovery(emit);
            }
            Availability::Available => {
                if self.parked.take().is_some() {
                    self.reconcile_discovery(emit);
                }
            }
        }
    }

    fn on_tick<F: FnMut(ManagerAction)>(&mut self, now_ms: u64, emit: &mut F) {
        let mut freed = false;
        for index in 0..PEER_TRACK {
            let Some(session) = self.sessions[index] else {
                continue;
            };
            if session.admitted.is_some() {
                continue;
            }
            if now_ms.saturating_sub(session.since_ms) < NDP_TIMEOUT_MS {
                continue;
            }
            self.sessions[index] = None;
            if session.fired_initiator {
                emit(ManagerAction::AbandonDataPath {
                    peer: session.peer,
                    role: NdpRole::Initiator,
                });
            }
            if session.fired_responder {
                emit(ManagerAction::AbandonDataPath {
                    peer: session.peer,
                    role: NdpRole::Responder,
                });
            }
            self.upsert_suppress(session.peer, now_ms);
            freed = true;
        }
        if freed {
            self.reconcile_discovery(emit);
        }
    }

    fn ensure_session(&mut self, peer: RendezvousToken, now_ms: u64) -> Option<usize> {
        if let Some(index) = self.session_index(peer) {
            return Some(index);
        }
        let slot = self.sessions.iter().position(Option::is_none)?;
        self.sessions[slot] = Some(PeerSession {
            peer,
            fired_initiator: false,
            fired_responder: false,
            admitted: None,
            since_ms: now_ms,
        });
        Some(slot)
    }

    fn engageable(&self, peer: RendezvousToken, now_ms: u64) -> bool {
        self.parked.is_none()
            && self.suppress_ready(peer, now_ms)
            && (self.session_index(peer).is_some() || self.has_free_slot())
    }

    fn has_free_slot(&self) -> bool {
        self.sessions.iter().any(Option::is_none)
    }

    fn session_index(&self, peer: RendezvousToken) -> Option<usize> {
        self.sessions
            .iter()
            .position(|slot| slot.is_some_and(|session| session.peer == peer))
    }

    fn reconcile_discovery<F: FnMut(ManagerAction)>(&mut self, emit: &mut F) {
        let want = self.parked.is_none() && self.has_free_slot();
        if want != self.discovery {
            self.discovery = want;
            emit(ManagerAction::SetDiscovery(if want {
                DiscoveryMode::On
            } else {
                DiscoveryMode::Off
            }));
        }
    }

    fn suppress_ready(&self, peer: RendezvousToken, now_ms: u64) -> bool {
        match self.find_suppress(peer) {
            Some(index) => self.suppressed[index].is_none_or(|entry| entry.elapsed(now_ms)),
            None => true,
        }
    }

    fn find_suppress(&self, peer: RendezvousToken) -> Option<usize> {
        self.suppressed
            .iter()
            .position(|entry| entry.is_some_and(|s| s.peer == peer))
    }

    fn clear_suppress(&mut self, peer: RendezvousToken) {
        if let Some(index) = self.find_suppress(peer) {
            self.suppressed[index] = None;
        }
    }

    fn upsert_suppress(&mut self, peer: RendezvousToken, now_ms: u64) {
        let entry = Suppress {
            peer,
            since_ms: now_ms,
        };
        if let Some(index) = self.find_suppress(peer) {
            self.suppressed[index] = Some(entry);
            return;
        }
        if let Some(index) = self.suppressed.iter().position(Option::is_none) {
            self.suppressed[index] = Some(entry);
            return;
        }
        self.prune_suppress(now_ms);
        let slot = self
            .suppressed
            .iter()
            .position(Option::is_none)
            .or_else(|| {
                self.suppressed
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, entry)| entry.map_or(u64::MAX, |s| s.since_ms))
                    .map(|(index, _)| index)
            });
        if let Some(index) = slot {
            self.suppressed[index] = Some(entry);
        }
    }

    fn prune_suppress(&mut self, now_ms: u64) {
        for entry in &mut self.suppressed {
            if entry.is_some_and(|s| s.elapsed(now_ms)) {
                *entry = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(value: u32) -> RendezvousToken {
        RendezvousToken::new(value)
    }

    fn started(local: u32) -> AwarePolicy<4> {
        let mut policy = AwarePolicy::new(token(local));
        policy.start(&mut |_| {});
        policy
    }

    fn collect<const P: usize>(
        policy: &mut AwarePolicy<P>,
        input: ManagerInput,
    ) -> std::vec::Vec<ManagerAction> {
        let mut actions = std::vec::Vec::new();
        policy.handle(input, &mut |action| actions.push(action));
        actions
    }

    // Drive a peer to a live member over its keeper path. `local` is lower than `peer`, so the keeper
    // is the initiator path this node opened on the sighting.
    fn connect_lower(policy: &mut AwarePolicy<4>, peer: u32, now_ms: u64) {
        policy.handle(
            ManagerInput::PeerDiscovered {
                peer: token(peer),
                now_ms,
            },
            &mut |_| {},
        );
        policy.handle(
            ManagerInput::DataPathUp {
                peer: token(peer),
                role: NdpRole::Initiator,
                now_ms,
            },
            &mut |_| {},
        );
    }

    #[test]
    fn start_turns_discovery_on() {
        let mut policy = AwarePolicy::<4>::new(token(1));
        let mut actions = std::vec::Vec::new();
        policy.start(&mut |action| actions.push(action));
        assert_eq!(
            actions,
            std::vec![ManagerAction::SetDiscovery(DiscoveryMode::On)]
        );
    }

    #[test]
    fn a_sighting_fires_an_initiator_attempt_and_an_inbound_request_a_responder_one() {
        let mut policy = started(5);
        let discovered = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 0,
            },
        );
        assert_eq!(
            discovered,
            std::vec![ManagerAction::RequestDataPath {
                peer: token(9),
                role: NdpRole::Initiator,
            }]
        );

        let requested = collect(
            &mut policy,
            ManagerInput::NdpRequested {
                peer: token(9),
                now_ms: 100,
            },
        );
        assert_eq!(
            requested,
            std::vec![ManagerAction::RequestDataPath {
                peer: token(9),
                role: NdpRole::Responder,
            }]
        );
    }

    #[test]
    fn a_repeated_sighting_does_not_re_fire_the_attempt() {
        let mut policy = started(5);
        collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 0,
            },
        );
        let again = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 500,
            },
        );
        assert!(again.is_empty());
    }

    #[test]
    fn the_first_path_to_settle_opens_the_plane() {
        let mut policy = started(5);
        collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 0,
            },
        );
        let opened = collect(
            &mut policy,
            ManagerInput::DataPathUp {
                peer: token(9),
                role: NdpRole::Initiator,
                now_ms: 200,
            },
        );
        assert_eq!(
            opened,
            std::vec![ManagerAction::OpenDataPlane {
                peer: token(9),
                role: NdpRole::Initiator,
            }]
        );
        assert!(policy.is_connected(token(9)));
    }

    #[test]
    fn the_keeper_evicts_a_non_keeper_incumbent() {
        // local 5 < peer 9, so the keeper is our initiator path. The non-keeper responder path
        // settles first and is admitted provisionally; the keeper then arrives and takes over.
        let mut policy = started(5);
        collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 0,
            },
        );
        collect(
            &mut policy,
            ManagerInput::NdpRequested {
                peer: token(9),
                now_ms: 0,
            },
        );

        let provisional = collect(
            &mut policy,
            ManagerInput::DataPathUp {
                peer: token(9),
                role: NdpRole::Responder,
                now_ms: 100,
            },
        );
        assert_eq!(
            provisional,
            std::vec![ManagerAction::OpenDataPlane {
                peer: token(9),
                role: NdpRole::Responder,
            }]
        );

        let swap = collect(
            &mut policy,
            ManagerInput::DataPathUp {
                peer: token(9),
                role: NdpRole::Initiator,
                now_ms: 150,
            },
        );
        assert_eq!(
            swap,
            std::vec![
                ManagerAction::CloseDataPlane { peer: token(9) },
                ManagerAction::AbandonDataPath {
                    peer: token(9),
                    role: NdpRole::Responder,
                },
                ManagerAction::OpenDataPlane {
                    peer: token(9),
                    role: NdpRole::Initiator,
                },
            ]
        );
    }

    #[test]
    fn a_keeper_incumbent_rejects_a_late_non_keeper() {
        // local 5 < peer 9: the initiator path is the keeper and settles first, so the later
        // responder path loses the duel and is abandoned.
        let mut policy = started(5);
        collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 0,
            },
        );
        collect(
            &mut policy,
            ManagerInput::NdpRequested {
                peer: token(9),
                now_ms: 0,
            },
        );
        collect(
            &mut policy,
            ManagerInput::DataPathUp {
                peer: token(9),
                role: NdpRole::Initiator,
                now_ms: 100,
            },
        );

        let rejected = collect(
            &mut policy,
            ManagerInput::DataPathUp {
                peer: token(9),
                role: NdpRole::Responder,
                now_ms: 150,
            },
        );
        assert_eq!(
            rejected,
            std::vec![ManagerAction::AbandonDataPath {
                peer: token(9),
                role: NdpRole::Responder,
            }]
        );
    }

    #[test]
    fn discovery_stays_on_across_a_connection_and_quiets_only_when_full() {
        let mut policy = started(1);
        for peer in [10, 11, 12] {
            let actions = collect(
                &mut policy,
                ManagerInput::PeerDiscovered {
                    peer: token(peer),
                    now_ms: 0,
                },
            );
            assert_eq!(
                actions,
                std::vec![ManagerAction::RequestDataPath {
                    peer: token(peer),
                    role: NdpRole::Initiator,
                }]
            );
        }
        assert_eq!(policy.peer_count(), 3);

        let fills = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(13),
                now_ms: 0,
            },
        );
        assert_eq!(
            fills,
            std::vec![
                ManagerAction::RequestDataPath {
                    peer: token(13),
                    role: NdpRole::Initiator,
                },
                ManagerAction::SetDiscovery(DiscoveryMode::Off),
            ]
        );

        let overflow = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(14),
                now_ms: 0,
            },
        );
        assert!(overflow.is_empty());
    }

    #[test]
    fn a_dropped_member_frees_the_slot_and_reopens_discovery() {
        let mut policy = started(1);
        for peer in [10, 11, 12, 13] {
            connect_lower(&mut policy, peer, 0);
        }
        assert_eq!(policy.peer_count(), 4);

        let dropped = collect(
            &mut policy,
            ManagerInput::DataPathDown {
                peer: token(11),
                role: NdpRole::Initiator,
                now_ms: 5_000,
            },
        );
        assert_eq!(
            dropped,
            std::vec![
                ManagerAction::CloseDataPlane { peer: token(11) },
                ManagerAction::SetDiscovery(DiscoveryMode::On),
            ]
        );
        assert_eq!(policy.peer_count(), 3);
    }

    #[test]
    fn a_dropped_member_cools_off_before_it_is_re_engaged() {
        let mut policy = started(1);
        connect_lower(&mut policy, 9, 0);
        let dropped = collect(
            &mut policy,
            ManagerInput::DataPathDown {
                peer: token(9),
                role: NdpRole::Initiator,
                now_ms: 1_000,
            },
        );
        assert_eq!(
            dropped,
            std::vec![ManagerAction::CloseDataPlane { peer: token(9) }]
        );

        let cooling = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 2_000,
            },
        );
        assert!(cooling.is_empty());

        let after = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 1_000 + SUPPRESS_TTL_MS,
            },
        );
        assert_eq!(
            after,
            std::vec![ManagerAction::RequestDataPath {
                peer: token(9),
                role: NdpRole::Initiator,
            }]
        );
    }

    #[test]
    fn a_losing_paths_teardown_leaves_the_live_member_untouched() {
        // A rejected responder path later drops; the admitted initiator member must be unaffected.
        let mut policy = started(5);
        collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 0,
            },
        );
        collect(
            &mut policy,
            ManagerInput::NdpRequested {
                peer: token(9),
                now_ms: 0,
            },
        );
        collect(
            &mut policy,
            ManagerInput::DataPathUp {
                peer: token(9),
                role: NdpRole::Initiator,
                now_ms: 100,
            },
        );
        collect(
            &mut policy,
            ManagerInput::DataPathUp {
                peer: token(9),
                role: NdpRole::Responder,
                now_ms: 150,
            },
        );

        let loser_dropped = collect(
            &mut policy,
            ManagerInput::DataPathDown {
                peer: token(9),
                role: NdpRole::Responder,
                now_ms: 200,
            },
        );
        assert!(loser_dropped.is_empty());
        assert!(policy.is_connected(token(9)));
    }

    #[test]
    fn a_hung_session_times_out_and_abandons_both_attempts() {
        let mut policy = started(5);
        collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: 0,
            },
        );
        collect(
            &mut policy,
            ManagerInput::NdpRequested {
                peer: token(9),
                now_ms: 0,
            },
        );
        assert_eq!(policy.next_deadline_ms(), Some(NDP_TIMEOUT_MS));

        let early = collect(
            &mut policy,
            ManagerInput::Tick {
                now_ms: NDP_TIMEOUT_MS - 1,
            },
        );
        assert!(early.is_empty());

        let abandoned = collect(
            &mut policy,
            ManagerInput::Tick {
                now_ms: NDP_TIMEOUT_MS,
            },
        );
        assert_eq!(
            abandoned,
            std::vec![
                ManagerAction::AbandonDataPath {
                    peer: token(9),
                    role: NdpRole::Initiator,
                },
                ManagerAction::AbandonDataPath {
                    peer: token(9),
                    role: NdpRole::Responder,
                },
            ]
        );
        assert_eq!(policy.peer_count(), 0);

        let suppressed = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
                now_ms: NDP_TIMEOUT_MS + 1_000,
            },
        );
        assert!(suppressed.is_empty());
    }

    #[test]
    fn concurrent_peers_each_hold_their_own_member() {
        let mut policy = started(1);
        for peer in [7, 8] {
            connect_lower(&mut policy, peer, 0);
        }
        assert!(policy.is_connected(token(7)));
        assert!(policy.is_connected(token(8)));
        assert_eq!(policy.peer_count(), 2);
    }

    #[test]
    fn unavailability_tears_every_path_down_and_availability_reopens() {
        let mut policy = started(1);
        connect_lower(&mut policy, 7, 0);
        collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(8),
                now_ms: 0,
            },
        );

        let parked = collect(
            &mut policy,
            ManagerInput::AvailabilityChanged {
                state: Availability::Unavailable("Wi-Fi Aware disabled by the platform"),
                now_ms: 1_000,
            },
        );
        assert_eq!(
            parked,
            std::vec![
                ManagerAction::CloseDataPlane { peer: token(7) },
                ManagerAction::AbandonDataPath {
                    peer: token(8),
                    role: NdpRole::Initiator,
                },
                ManagerAction::SetDiscovery(DiscoveryMode::Off),
            ]
        );
        assert_eq!(
            policy.park_reason(),
            Some("Wi-Fi Aware disabled by the platform")
        );

        let while_parked = collect(
            &mut policy,
            ManagerInput::PeerDiscovered {
                peer: token(9),
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
        assert_eq!(policy.park_reason(), None);
    }
}
