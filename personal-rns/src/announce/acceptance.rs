//! The announce-acceptance predicate: a faithful port of the `should_add` derivation in
//! Python `Transport.inbound()`
//! <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1748-L1829>.
//!
//! It answers one question: "Should this heard announce be installed into the
//! path table?" It does this as a total function of the announce, the existing path (if
//! any), whether we own the destination, and the packet's arrival instant.
//! Applying the decision is the engine's job; the predicate mutates nothing.

use core::cmp::Ordering;

use crate::announce::{AnnounceId, MonotonicTimebase};
use crate::engine::InstantMillis;
use crate::path::{ExistingPath, PathResponsiveness, MAX_HOP_COUNT};

#[derive(Debug, Clone, Copy)]
pub struct AnnounceAcceptanceInput<'a> {
    pub packet_hops: u8,
    /// RNS's `random_blob`
    pub announce_id: AnnounceId,
    pub destination_is_local: bool,
    pub existing_path: Option<ExistingPath<'a>>,
    /// The arriving packet's instant, used as "now" for path-expiry — keeps the
    /// predicate clock-free: time arrives as data on the packet.
    pub arrived_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptReason {
    FirstSighting,
    KnownRouteFreshEvidence,
    ExpiredRouteSucceededByLongerAlternative,
    LongerAlternativeWithNewerEvidence,
    FailoverFromUnresponsiveIncumbent,
}

/// Why an announce is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Hop count exceeds max — beyond reachable range.
    BeyondReach,
    DestinationIsLocal,
    KnownRouteReplay,
    KnownRouteNoNewerEvidence,
    DeadRouteReplay,
    NewerEmissionStampButReplayedBlob,
    EqualEvidenceIncumbentStillWorking,
    /// Longer hops, fresh route, emission strictly older than stored, i.e., stale.
    /// Python's if/elif chain has no else arm here, so `should_add` keeps its
    /// initial `False`; we surface it explicitly rather than as fallthrough.
    StaleEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceAcceptanceDecision {
    Accept(AcceptReason),
    Reject(RejectReason),
}

impl AnnounceAcceptanceInput<'_> {
    pub fn determine_acceptance(&self) -> AnnounceAcceptanceDecision {
        use AcceptReason::*;
        use AnnounceAcceptanceDecision::{Accept, Reject};
        use RejectReason::*;

        if self.packet_hops > MAX_HOP_COUNT {
            return Reject(BeyondReach);
        }
        if self.destination_is_local {
            return Reject(DestinationIsLocal);
        }
        let Some(existing) = self.existing_path else {
            return Accept(FirstSighting);
        };

        let is_longer_hops = self.packet_hops > existing.hops;
        let path_is_expired = self.arrived_at >= existing.expires;
        let announce_emitted_at = self.announce_id.timebase;

        let mut announce_id_was_already_seen = false;
        let mut path_max_emitted = MonotonicTimebase::ZERO;
        for stored in existing.seen_announce_ids.iter() {
            if !announce_id_was_already_seen && *stored == self.announce_id {
                announce_id_was_already_seen = true;
                if !is_longer_hops {
                    return Reject(KnownRouteReplay);
                }
                if path_is_expired {
                    return Reject(DeadRouteReplay);
                }
            }
            path_max_emitted = path_max_emitted.max(stored.timebase);
        }

        // EQUAL OR SHORTER HOPS — id was unheard (a hit would have early-exited).
        if !is_longer_hops {
            return if announce_emitted_at > path_max_emitted {
                Accept(KnownRouteFreshEvidence)
            } else {
                Reject(KnownRouteNoNewerEvidence)
            };
        }

        // LONGER HOPS + EXPIRED — id was unheard (a hit would have early-exited).
        if path_is_expired {
            return Accept(ExpiredRouteSucceededByLongerAlternative);
        }

        // LONGER HOPS + FRESH PATH. The seen-status matters only when the
        // emission is the more recent one; the equal and older arms decide
        // independently of it.
        match announce_emitted_at.cmp(&path_max_emitted) {
            Ordering::Less => Reject(StaleEvidence),
            Ordering::Equal => match existing.responsiveness {
                PathResponsiveness::Unresponsive => Accept(FailoverFromUnresponsiveIncumbent),
                PathResponsiveness::Responsive => Reject(EqualEvidenceIncumbentStillWorking),
            },
            Ordering::Greater => {
                if announce_id_was_already_seen {
                    Reject(NewerEmissionStampButReplayedBlob)
                } else {
                    Accept(LongerAlternativeWithNewerEvidence)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::SeenAnnounceIds;

    fn announce_id(nonce_byte: u8, timebase: u64) -> AnnounceId {
        let mut bytes = [0u8; 10];
        bytes[..5].copy_from_slice(&[nonce_byte; 5]);
        bytes[5..].copy_from_slice(&timebase.to_be_bytes()[3..]);
        AnnounceId::from_wire(bytes)
    }

    fn decide(input: AnnounceAcceptanceInput) -> AnnounceAcceptanceDecision {
        input.determine_acceptance()
    }

    #[test]
    fn hops_beyond_pathfinder_m_are_rejected() {
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: MAX_HOP_COUNT + 1,
            announce_id: announce_id(0x11, 5_000),
            destination_is_local: false,
            existing_path: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::BeyondReach)
        );
    }

    #[test]
    fn hops_exactly_at_pathfinder_m_are_accepted() {
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: MAX_HOP_COUNT,
            announce_id: announce_id(0x22, 5_000),
            destination_is_local: false,
            existing_path: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(AcceptReason::FirstSighting)
        );
    }

    #[test]
    fn local_destination_is_rejected() {
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 1,
            announce_id: announce_id(0x33, 5_000),
            destination_is_local: true,
            existing_path: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::DestinationIsLocal)
        );
    }

    #[test]
    fn no_existing_path_is_a_first_sighting() {
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 2,
            announce_id: announce_id(0x44, 5_000),
            destination_is_local: false,
            existing_path: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(AcceptReason::FirstSighting)
        );
    }

    #[test]
    fn same_hops_newer_emission_unseen_id_accepts() {
        let stored = announce_id(0x55, 100);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 3,
            announce_id: announce_id(0x56, 200),
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 3,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(AcceptReason::KnownRouteFreshEvidence)
        );
    }

    #[test]
    fn same_hops_replayed_id_rejects() {
        let stored = announce_id(0x55, 200);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 3,
            announce_id: stored,
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 3,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteReplay)
        );
    }

    #[test]
    fn same_hops_equal_emission_unseen_id_rejects() {
        let stored = announce_id(0x55, 200);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 3,
            announce_id: announce_id(0x56, 200),
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 3,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteNoNewerEvidence)
        );
    }

    #[test]
    fn longer_hops_expired_path_unseen_id_accepts() {
        let stored = announce_id(0x66, 200);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 5,
            announce_id: announce_id(0x67, 50),
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 2,
                expires: InstantMillis(1_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(2_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(
                AcceptReason::ExpiredRouteSucceededByLongerAlternative
            )
        );
    }

    #[test]
    fn longer_hops_expired_path_seen_id_rejects() {
        let stored = announce_id(0x66, 200);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 5,
            announce_id: stored,
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 2,
                expires: InstantMillis(1_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(2_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::DeadRouteReplay)
        );
    }

    #[test]
    fn longer_hops_fresh_newer_emission_unseen_id_accepts() {
        let stored = announce_id(0x77, 100);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 6,
            announce_id: announce_id(0x78, 500),
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 2,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(AcceptReason::LongerAlternativeWithNewerEvidence)
        );
    }

    #[test]
    fn longer_hops_fresh_equal_emission_unresponsive_is_a_failover() {
        let stored = announce_id(0x88, 300);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 6,
            announce_id: announce_id(0x89, 300),
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 2,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Unresponsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(AcceptReason::FailoverFromUnresponsiveIncumbent)
        );
    }

    #[test]
    fn longer_hops_fresh_equal_emission_responsive_rejects() {
        let stored = announce_id(0x99, 300);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 6,
            announce_id: announce_id(0x9a, 300),
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 2,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::EqualEvidenceIncumbentStillWorking)
        );
    }

    #[test]
    fn longer_hops_fresh_older_emission_is_stale() {
        let stored = announce_id(0xaa, 500);
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 6,
            announce_id: announce_id(0xab, 300),
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 2,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(
                    core::slice::from_ref(&stored),
                    &[],
                ),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::StaleEvidence)
        );
    }

    #[test]
    fn replay_of_an_id_only_in_overflow_is_recognized_as_known() {
        // The predicate iterates the view's floor and overflow back-to-back;
        // a replay of an id that lives exclusively in the overflow tier must
        // still hit the KnownRouteReplay branch. (Pre-fix, an iterator that
        // skipped overflow would have let the replay through.)
        let floor = [announce_id(0xA, 100), announce_id(0xB, 200)];
        let overflow = [announce_id(0xC, 300), announce_id(0xD, 400)];
        let replayed = overflow[0];
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 3,
            announce_id: replayed,
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 3,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(&floor, &overflow),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteReplay)
        );
    }

    #[test]
    fn max_emitted_calculation_includes_overflow_ids() {
        // path_max_emitted is the running max across every seen id — overflow
        // entries have to be included for the same-hops "newer evidence?" gate
        // to compare against the right ceiling. We put the newest stored
        // timebase in overflow and arrive with one that beats the floor but
        // not the overflow; the predicate should reject as no-newer-evidence.
        let floor = [announce_id(0xA, 100)];
        let overflow = [announce_id(0xB, 500)]; // newer than floor's 100
        let decision = decide(AnnounceAcceptanceInput {
            packet_hops: 3,
            announce_id: announce_id(0xC, 300), // newer than floor, older than overflow
            destination_is_local: false,
            existing_path: Some(ExistingPath {
                hops: 3,
                expires: InstantMillis(10_000),
                seen_announce_ids: SeenAnnounceIds::from_slices(&floor, &overflow),
                responsiveness: PathResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteNoNewerEvidence)
        );
    }
}
