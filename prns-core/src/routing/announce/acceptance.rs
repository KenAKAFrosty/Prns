//! The announce-acceptance predicate: a faithful port of the `should_add` derivation in Python `Transport.inbound()`
//!
//! A total function of the announce, the existing path, ownership, and arrival instant; it mutates nothing.

use core::cmp::Ordering;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceGravity;
use crate::routing::announce::{AnnounceId, MonotonicTimebase};
use crate::routing::{ExistingRoute, RouteResponsiveness};
use crate::wire::MAX_HOP_COUNT;

#[derive(Debug, Clone, Copy)]
pub struct AnnounceAcceptanceInput<'a> {
    pub packet_hops: u8,
    pub announce_id: AnnounceId,
    pub destination_is_self_or_upstream: bool,
    pub existing_route: Option<ExistingRoute<'a>>,
    pub incoming_interface_gravity: Option<InterfaceGravity>,
    pub arrived_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptReason {
    FirstSighting,
    KnownRouteFreshEvidence,
    ExpiredRouteSucceededByLongerAlternative,
    LongerAlternativeWithNewerEvidence,
    FailoverFromUnresponsiveIncumbent,
    EqualEvidenceHigherGravity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    ExceedsMaxHops,
    /// A destination this node answers for (our own, or an upstream app's) is delivered locally, never routed, so no path may be learned for it.
    /// This must be a rejection because neighbors rebroadcast our own announces back at us; accepting the echo would store a route for our own address pointing out an interface.
    /// RNS folds this into `should_add`'s first condition alongside the hops cap.
    DestinationIsSelfOrUpstream,
    KnownRouteReplay,
    KnownRouteNoNewerEvidence,
    DeadRouteReplay,
    EqualEvidenceIncumbentStillWorking,
    /// Longer hops, fresh route, emission strictly older than stored, i.e., stale.
    /// Python's if/elif chain has no else arm here, so `should_add` keeps its initial `False`; we surface it explicitly.
    StaleEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceAcceptanceDecision {
    Accept(AcceptReason),
    Reject(RejectReason),
}

pub fn determine_acceptance(input: AnnounceAcceptanceInput<'_>) -> AnnounceAcceptanceDecision {
    use AcceptReason::*;
    use AnnounceAcceptanceDecision::{Accept, Reject};
    use RejectReason::*;

    if input.packet_hops > MAX_HOP_COUNT {
        return Reject(ExceedsMaxHops);
    }
    if input.destination_is_self_or_upstream {
        return Reject(DestinationIsSelfOrUpstream);
    }
    let Some(existing) = input.existing_route else {
        return Accept(FirstSighting);
    };

    let is_longer_hops = input.packet_hops > existing.hops.0;
    let route_is_expired = input.arrived_at >= existing.expires_at;
    let announce_emitted_at = input.announce_id.timebase;

    let mut route_max_emitted = MonotonicTimebase::ZERO;
    let mut known_announce = false;
    for stored in existing.announce_id_history.iter() {
        if *stored == input.announce_id {
            known_announce = true;
            if is_longer_hops && route_is_expired {
                return Reject(DeadRouteReplay);
            }
        }
        route_max_emitted = route_max_emitted.max(stored.timebase);
    }

    if !is_longer_hops {
        if announce_emitted_at == route_max_emitted
            && input
                .incoming_interface_gravity
                .zip(existing.interface_gravity)
                .is_some_and(|(incoming, incumbent)| incoming > incumbent)
        {
            return Accept(EqualEvidenceHigherGravity);
        }
        if known_announce {
            return Reject(KnownRouteReplay);
        }
        return if announce_emitted_at > route_max_emitted {
            Accept(KnownRouteFreshEvidence)
        } else {
            Reject(KnownRouteNoNewerEvidence)
        };
    }
    if route_is_expired {
        return Accept(ExpiredRouteSucceededByLongerAlternative);
    }

    // Acknowledged deviation: the reference's longer-hops arm folds this maximum inline with an early break, so its equal-evidence reading can vary with the order past announces happened to arrive.
    // Everywhere else the reference treats stored ids as an unordered set (membership checks, and its `timebase_from_random_blobs` is a plain max), so we follow that reading consistently and compare against the true max: the same decision from the same knowledge, however it was heard.
    match announce_emitted_at.cmp(&route_max_emitted) {
        Ordering::Less => Reject(StaleEvidence),
        Ordering::Equal => match existing.responsiveness {
            RouteResponsiveness::Unresponsive => Accept(FailoverFromUnresponsiveIncumbent),
            RouteResponsiveness::Responsive | RouteResponsiveness::Unknown => {
                Reject(EqualEvidenceIncumbentStillWorking)
            }
        },
        // A seen id's stamp is already folded into route_max_emitted, so Greater proves the blob is new.
        Ordering::Greater => Accept(LongerAlternativeWithNewerEvidence),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announce_id(nonce_byte: u8, timebase: u64) -> AnnounceId {
        let mut bytes = [0u8; 10];
        bytes[..5].copy_from_slice(&[nonce_byte; 5]);
        bytes[5..].copy_from_slice(&timebase.to_be_bytes()[3..]);
        AnnounceId::from_wire(bytes)
    }

    fn decide(input: AnnounceAcceptanceInput) -> AnnounceAcceptanceDecision {
        determine_acceptance(input)
    }

    #[test]
    fn hops_beyond_pathfinder_m_are_rejected() {
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: MAX_HOP_COUNT + 1,
            announce_id: announce_id(0x11, 5_000),
            destination_is_self_or_upstream: false,
            existing_route: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::ExceedsMaxHops)
        );
    }

    #[test]
    fn hops_exactly_at_pathfinder_m_are_accepted() {
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: MAX_HOP_COUNT,
            announce_id: announce_id(0x22, 5_000),
            destination_is_self_or_upstream: false,
            existing_route: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(AcceptReason::FirstSighting)
        );
    }

    #[test]
    fn an_upstream_app_destination_is_rejected() {
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: 1,
            announce_id: announce_id(0x33, 5_000),
            destination_is_self_or_upstream: true,
            existing_route: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::DestinationIsSelfOrUpstream)
        );
    }

    #[test]
    fn no_existing_route_is_a_first_sighting() {
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: 2,
            announce_id: announce_id(0x44, 5_000),
            destination_is_self_or_upstream: false,
            existing_route: None,
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Accept(AcceptReason::FirstSighting)
        );
    }

    #[test]
    fn an_older_stamp_reads_stale_regardless_of_history_arrival_order() {
        let equal_stamp = announce_id(0x77, 90);
        let newer_stamp = announce_id(0x78, 100);
        for history in [[equal_stamp, newer_stamp], [newer_stamp, equal_stamp]] {
            let decision = decide(AnnounceAcceptanceInput {
                incoming_interface_gravity: None,
                packet_hops: 5,
                announce_id: announce_id(0x79, 90),
                destination_is_self_or_upstream: false,
                existing_route: Some(ExistingRoute {
                    interface_gravity: None,
                    hops: crate::units::HopCount(3),
                    expires_at: InstantMillis(10_000),
                    announce_id_history: &history,
                    responsiveness: RouteResponsiveness::Unresponsive,
                }),
                arrived_at: InstantMillis(1_000),
            });
            assert_eq!(
                decision,
                AnnounceAcceptanceDecision::Reject(RejectReason::StaleEvidence),
                "the freshest stored stamp is 100, so a 90-stamped longer alternative is stale however history was heard",
            );
        }
    }

    #[test]
    fn same_hops_newer_emission_unseen_id_accepts() {
        let stored = announce_id(0x55, 100);
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: 3,
            announce_id: announce_id(0x56, 200),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(3),
                expires_at: InstantMillis(10_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
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
            incoming_interface_gravity: None,
            packet_hops: 3,
            announce_id: stored,
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(3),
                expires_at: InstantMillis(10_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
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
            incoming_interface_gravity: None,
            packet_hops: 3,
            announce_id: announce_id(0x56, 200),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(3),
                expires_at: InstantMillis(10_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteNoNewerEvidence)
        );
    }

    #[test]
    fn equal_evidence_moves_to_a_higher_gravity_interface() {
        let stored = announce_id(0x55, 200);
        for incoming in [stored, announce_id(0x56, 200)] {
            let decision = decide(AnnounceAcceptanceInput {
                incoming_interface_gravity: Some(InterfaceGravity::new(8)),
                packet_hops: 3,
                announce_id: incoming,
                destination_is_self_or_upstream: false,
                existing_route: Some(ExistingRoute {
                    interface_gravity: Some(InterfaceGravity::new(-3)),
                    hops: crate::units::HopCount(3),
                    expires_at: InstantMillis(10_000),
                    announce_id_history: core::slice::from_ref(&stored),
                    responsiveness: RouteResponsiveness::Responsive,
                }),
                arrived_at: InstantMillis(1_000),
            });
            assert_eq!(
                decision,
                AnnounceAcceptanceDecision::Accept(AcceptReason::EqualEvidenceHigherGravity)
            );
        }
    }

    #[test]
    fn equal_evidence_does_not_move_to_equal_or_lower_gravity() {
        let stored = announce_id(0x55, 200);
        for incoming in [-4, -5] {
            let decision = decide(AnnounceAcceptanceInput {
                incoming_interface_gravity: Some(InterfaceGravity::new(incoming)),
                packet_hops: 3,
                announce_id: stored,
                destination_is_self_or_upstream: false,
                existing_route: Some(ExistingRoute {
                    interface_gravity: Some(InterfaceGravity::new(-4)),
                    hops: crate::units::HopCount(3),
                    expires_at: InstantMillis(10_000),
                    announce_id_history: core::slice::from_ref(&stored),
                    responsiveness: RouteResponsiveness::Responsive,
                }),
                arrived_at: InstantMillis(1_000),
            });
            assert_eq!(
                decision,
                AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteReplay)
            );
        }
    }

    #[test]
    fn equal_evidence_requires_both_interface_gravities() {
        let stored = announce_id(0x55, 200);
        for (incoming, incumbent) in [
            (None, Some(InterfaceGravity::ZERO)),
            (Some(InterfaceGravity::new(1)), None),
        ] {
            let decision = decide(AnnounceAcceptanceInput {
                incoming_interface_gravity: incoming,
                packet_hops: 3,
                announce_id: stored,
                destination_is_self_or_upstream: false,
                existing_route: Some(ExistingRoute {
                    interface_gravity: incumbent,
                    hops: crate::units::HopCount(3),
                    expires_at: InstantMillis(10_000),
                    announce_id_history: core::slice::from_ref(&stored),
                    responsiveness: RouteResponsiveness::Responsive,
                }),
                arrived_at: InstantMillis(1_000),
            });
            assert_eq!(
                decision,
                AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteReplay)
            );
        }
    }

    #[test]
    fn gravity_does_not_promote_older_or_longer_evidence() {
        let stored = announce_id(0x55, 200);
        for (packet_hops, announce) in [(3, announce_id(0x56, 199)), (4, stored)] {
            let decision = decide(AnnounceAcceptanceInput {
                incoming_interface_gravity: Some(InterfaceGravity::new(100)),
                packet_hops,
                announce_id: announce,
                destination_is_self_or_upstream: false,
                existing_route: Some(ExistingRoute {
                    interface_gravity: Some(InterfaceGravity::ZERO),
                    hops: crate::units::HopCount(3),
                    expires_at: InstantMillis(10_000),
                    announce_id_history: core::slice::from_ref(&stored),
                    responsiveness: RouteResponsiveness::Responsive,
                }),
                arrived_at: InstantMillis(1_000),
            });
            assert!(matches!(decision, AnnounceAcceptanceDecision::Reject(_)));
        }
    }

    #[test]
    fn same_hops_fresh_nonce_but_older_emission_rejects() {
        let stored = announce_id(0x55, 200);
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: 3,
            announce_id: announce_id(0x56, 150),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(3),
                expires_at: InstantMillis(10_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
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
            incoming_interface_gravity: None,
            packet_hops: 5,
            announce_id: announce_id(0x67, 50),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(2),
                expires_at: InstantMillis(1_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
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
            incoming_interface_gravity: None,
            packet_hops: 5,
            announce_id: stored,
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(2),
                expires_at: InstantMillis(1_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
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
            incoming_interface_gravity: None,
            packet_hops: 6,
            announce_id: announce_id(0x78, 500),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(2),
                expires_at: InstantMillis(10_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
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
        for incoming in [stored, announce_id(0x89, 300)] {
            let decision = decide(AnnounceAcceptanceInput {
                incoming_interface_gravity: None,
                packet_hops: 6,
                announce_id: incoming,
                destination_is_self_or_upstream: false,
                existing_route: Some(ExistingRoute {
                    interface_gravity: None,
                    hops: crate::units::HopCount(2),
                    expires_at: InstantMillis(10_000),
                    announce_id_history: core::slice::from_ref(&stored),
                    responsiveness: RouteResponsiveness::Unresponsive,
                }),
                arrived_at: InstantMillis(1_000),
            });
            assert_eq!(
                decision,
                AnnounceAcceptanceDecision::Accept(AcceptReason::FailoverFromUnresponsiveIncumbent)
            );
        }
    }

    #[test]
    fn longer_hops_fresh_equal_emission_responsive_rejects() {
        let stored = announce_id(0x99, 300);
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: 6,
            announce_id: announce_id(0x9a, 300),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(2),
                expires_at: InstantMillis(10_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
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
            incoming_interface_gravity: None,
            packet_hops: 6,
            announce_id: announce_id(0xab, 300),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(2),
                expires_at: InstantMillis(10_000),
                announce_id_history: core::slice::from_ref(&stored),
                responsiveness: RouteResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::StaleEvidence)
        );
    }

    #[test]
    fn replay_is_recognized_wherever_it_sits_in_history() {
        let history = [
            announce_id(0xA, 100),
            announce_id(0xB, 200),
            announce_id(0xC, 300),
            announce_id(0xD, 400),
        ];
        let replayed = history[2];
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: 3,
            announce_id: replayed,
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(3),
                expires_at: InstantMillis(10_000),
                announce_id_history: &history,
                responsiveness: RouteResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteReplay)
        );
    }

    #[test]
    fn max_emitted_reads_the_full_history() {
        let history = [announce_id(0xA, 100), announce_id(0xB, 500)];
        let decision = decide(AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops: 3,
            announce_id: announce_id(0xC, 300),
            destination_is_self_or_upstream: false,
            existing_route: Some(ExistingRoute {
                interface_gravity: None,
                hops: crate::units::HopCount(3),
                expires_at: InstantMillis(10_000),
                announce_id_history: &history,
                responsiveness: RouteResponsiveness::Responsive,
            }),
            arrived_at: InstantMillis(1_000),
        });
        assert_eq!(
            decision,
            AnnounceAcceptanceDecision::Reject(RejectReason::KnownRouteNoNewerEvidence)
        );
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    fn arbitrary_announce_id() -> AnnounceId {
        AnnounceId::from_wire(kani::any())
    }

    #[kani::proof]
    fn hops_above_pathfinder_m_always_reject_before_any_other_gate() {
        let packet_hops: u8 = kani::any();
        kani::assume(packet_hops > MAX_HOP_COUNT);
        let input = AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops,
            announce_id: arbitrary_announce_id(),
            destination_is_self_or_upstream: kani::any(),
            existing_route: None,
            arrived_at: InstantMillis(kani::any()),
        };

        assert_eq!(
            determine_acceptance(input),
            AnnounceAcceptanceDecision::Reject(RejectReason::ExceedsMaxHops)
        );
    }

    #[kani::proof]
    fn an_upstream_app_destination_rejects_when_hops_are_in_range() {
        let packet_hops: u8 = kani::any();
        kani::assume(packet_hops <= MAX_HOP_COUNT);
        let input = AnnounceAcceptanceInput {
            incoming_interface_gravity: None,
            packet_hops,
            announce_id: arbitrary_announce_id(),
            destination_is_self_or_upstream: true,
            existing_route: None,
            arrived_at: InstantMillis(kani::any()),
        };

        assert_eq!(
            determine_acceptance(input),
            AnnounceAcceptanceDecision::Reject(RejectReason::DestinationIsSelfOrUpstream)
        );
    }
}
