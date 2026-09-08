use super::AddressSpaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservationId(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservationPoolId(pub &'static str);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationCharge {
    AdditionalToStatic,
    IncludedInStaticImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationAccounting {
    Dedicated {
        charge: ReservationCharge,
    },
    SharedPool {
        pool: ReservationPoolId,
        charge: ReservationCharge,
    },
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeReservation {
    pub id: ReservationId,
    pub address_space: AddressSpaceId,
    pub bytes: u64,
    pub accounting: ReservationAccounting,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReservationTotals {
    pub additional_bytes: u64,
    pub linker_counted_bytes: u64,
    pub external_bytes: u64,
}
