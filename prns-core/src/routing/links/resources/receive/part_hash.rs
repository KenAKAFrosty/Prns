use crate::engine::InstantMillis;
use crate::routing::ingress::{IgnoreReason, IngestPacketOutcome};
use crate::routing::links::resources::{map_hash, ResourceHash, SaltNonce, MAP_HASH_LEN};
use crate::routing::links::LinkId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourcePartHashLane {
    #[default]
    Inline,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePartHashReservation {
    pub(crate) link_id: LinkId,
    pub(crate) hash: ResourceHash,
    pub(crate) salt_nonce: SaltNonce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourcePartHashReservations {
    One([ResourcePartHashReservation; 1]),
    Two([ResourcePartHashReservation; 2]),
    Three([ResourcePartHashReservation; 3]),
    Four([ResourcePartHashReservation; 4]),
}

impl ResourcePartHashReservations {
    pub(crate) fn one(reservation: ResourcePartHashReservation) -> Self {
        Self::One([reservation])
    }

    pub(crate) fn including(
        self,
        reservation: ResourcePartHashReservation,
    ) -> Result<Self, ResourcePartHashCandidateCapacityReached> {
        match self {
            Self::One([first]) => Ok(Self::Two([first, reservation])),
            Self::Two([first, second]) => Ok(Self::Three([first, second, reservation])),
            Self::Three([first, second, third]) => {
                Ok(Self::Four([first, second, third, reservation]))
            }
            Self::Four(_) => Err(ResourcePartHashCandidateCapacityReached),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ResourcePartHashCandidateCapacityReached;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePartHashPlan {
    pub(crate) reservations: ResourcePartHashReservations,
    pub(crate) arrived_at: InstantMillis,
}

impl ResourcePartHashPlan {
    #[must_use]
    pub fn calculate(self, part: &[u8]) -> ResourcePartHashResult {
        ResourcePartHashResult {
            matches: match self.reservations {
                ResourcePartHashReservations::One(reservations) => {
                    ResourcePartHashMatches::One(calculate_matches(reservations, part))
                }
                ResourcePartHashReservations::Two(reservations) => {
                    ResourcePartHashMatches::Two(calculate_matches(reservations, part))
                }
                ResourcePartHashReservations::Three(reservations) => {
                    ResourcePartHashMatches::Three(calculate_matches(reservations, part))
                }
                ResourcePartHashReservations::Four(reservations) => {
                    ResourcePartHashMatches::Four(calculate_matches(reservations, part))
                }
            },
            arrived_at: self.arrived_at,
        }
    }
}

fn calculate_matches<const N: usize>(
    reservations: [ResourcePartHashReservation; N],
    part: &[u8],
) -> [ResourcePartHashMatch; N] {
    reservations.map(|reservation| ResourcePartHashMatch {
        reservation,
        name: map_hash(part, &reservation.salt_nonce),
    })
}

pub(crate) struct ResourcePartHashMatch {
    pub(crate) reservation: ResourcePartHashReservation,
    pub(crate) name: [u8; MAP_HASH_LEN],
}

pub(crate) enum ResourcePartHashMatches {
    One([ResourcePartHashMatch; 1]),
    Two([ResourcePartHashMatch; 2]),
    Three([ResourcePartHashMatch; 3]),
    Four([ResourcePartHashMatch; 4]),
}

impl ResourcePartHashMatches {
    pub(crate) fn as_slice(&self) -> &[ResourcePartHashMatch] {
        match self {
            Self::One(matches) => matches,
            Self::Two(matches) => matches,
            Self::Three(matches) => matches,
            Self::Four(matches) => matches,
        }
    }
}

pub struct ResourcePartHashResult {
    matches: ResourcePartHashMatches,
    arrived_at: InstantMillis,
}

impl ResourcePartHashResult {
    #[must_use]
    pub fn completed<'a>(self, part: &'a [u8]) -> ResourcePartHashCompleted<'a> {
        ResourcePartHashCompleted {
            matches: self.matches,
            arrived_at: self.arrived_at,
            part,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePartHashOwed<'a> {
    pub(crate) plan: ResourcePartHashPlan,
    pub(crate) part: &'a [u8],
}

impl<'a> ResourcePartHashOwed<'a> {
    #[must_use]
    pub fn into_parts(self) -> (ResourcePartHashPlan, &'a [u8]) {
        (self.plan, self.part)
    }

    #[must_use]
    pub fn fulfill(self) -> ResourcePartHashCompleted<'a> {
        self.plan.calculate(self.part).completed(self.part)
    }
}

pub struct ResourcePartHashCompleted<'a> {
    pub(crate) matches: ResourcePartHashMatches,
    pub(crate) arrived_at: InstantMillis,
    pub(crate) part: &'a [u8],
}

pub(crate) enum ResourcePartHashLanding {
    Ignored(IgnoreReason),
    Pull { link_id: LinkId, hash: ResourceHash },
    Assembly { link_id: LinkId, hash: ResourceHash },
    DeadlineAdvanced { link_id: LinkId, hash: ResourceHash },
}

impl ResourcePartHashLanding {
    pub(crate) fn into_ingest_outcome(self) -> IngestPacketOutcome<'static> {
        match self {
            Self::Ignored(reason) => IngestPacketOutcome::Ignored(reason),
            Self::Pull { link_id, hash } => IngestPacketOutcome::OwesResourcePull { link_id, hash },
            Self::Assembly { link_id, hash } => {
                IngestPacketOutcome::OwesResourceAssembly { link_id, hash }
            }
            Self::DeadlineAdvanced { link_id, hash } => {
                IngestPacketOutcome::ResourceDeadlineAdvanced { link_id, hash }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reservation(salt: u8) -> ResourcePartHashReservation {
        ResourcePartHashReservation {
            link_id: LinkId::new([0x11; 16]),
            hash: ResourceHash::new([salt; 32]),
            salt_nonce: SaltNonce::new([salt; 4]),
        }
    }

    #[test]
    fn four_candidate_salts_are_calculated_in_reservation_order() {
        let reservations = ResourcePartHashReservations::one(reservation(1))
            .including(reservation(2))
            .unwrap()
            .including(reservation(3))
            .unwrap()
            .including(reservation(4))
            .unwrap();
        let part = b"one part can belong to one of four overlapping transfers";
        let result = ResourcePartHashPlan {
            reservations,
            arrived_at: InstantMillis(7),
        }
        .calculate(part);

        let actual: std::vec::Vec<_> = result
            .matches
            .as_slice()
            .iter()
            .map(|candidate| (candidate.reservation.hash, candidate.name))
            .collect();
        let expected: std::vec::Vec<_> = (1..=4)
            .map(|salt| {
                let reservation = reservation(salt);
                (reservation.hash, map_hash(part, &reservation.salt_nonce))
            })
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_fifth_candidate_exceeds_the_external_plan_capacity() {
        let reservations = ResourcePartHashReservations::one(reservation(1))
            .including(reservation(2))
            .unwrap()
            .including(reservation(3))
            .unwrap()
            .including(reservation(4))
            .unwrap();
        assert!(reservations.including(reservation(5)).is_err());
    }
}
