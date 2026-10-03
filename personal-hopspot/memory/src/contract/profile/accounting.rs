use super::super::{AddressSpaceId, ReservationAccounting, ReservationCharge, ReservationTotals};
use super::{MemoryProfile, ValidationError};

impl MemoryProfile {
    pub fn reservation_totals(
        &self,
        address_space: AddressSpaceId,
    ) -> Result<ReservationTotals, ValidationError> {
        let mut totals = ReservationTotals::default();
        for (index, reservation) in self.runtime_reservations.iter().enumerate() {
            if reservation.address_space != address_space {
                continue;
            }
            if let ReservationAccounting::SharedPool { pool, .. } = reservation.accounting {
                if self.runtime_reservations[..index].iter().any(|prior| {
                    prior.address_space == address_space
                        && matches!(
                            prior.accounting,
                            ReservationAccounting::SharedPool { pool: prior_pool, .. }
                                if prior_pool == pool
                        )
                }) {
                    continue;
                }
            }
            match reservation.accounting {
                ReservationAccounting::Dedicated {
                    charge: ReservationCharge::AdditionalToStatic,
                }
                | ReservationAccounting::SharedPool {
                    charge: ReservationCharge::AdditionalToStatic,
                    ..
                } => {
                    totals.additional_bytes = totals
                        .additional_bytes
                        .checked_add(reservation.bytes)
                        .ok_or(ValidationError::ReservationTotalOverflow { address_space })?;
                }
                ReservationAccounting::Dedicated {
                    charge: ReservationCharge::IncludedInStaticImage,
                }
                | ReservationAccounting::SharedPool {
                    charge: ReservationCharge::IncludedInStaticImage,
                    ..
                } => {
                    totals.linker_counted_bytes = totals
                        .linker_counted_bytes
                        .checked_add(reservation.bytes)
                        .ok_or(ValidationError::ReservationTotalOverflow { address_space })?;
                }
                ReservationAccounting::External => {
                    totals.external_bytes = totals
                        .external_bytes
                        .checked_add(reservation.bytes)
                        .ok_or(ValidationError::ReservationTotalOverflow { address_space })?;
                }
            }
        }
        Ok(totals)
    }
}
