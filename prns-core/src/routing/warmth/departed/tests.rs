use super::{
    DepartedInterfaces, Departure, FixedDepartedInterfaceTable, DEPARTED_INTERFACE_GRACE_MS,
};
use crate::interfaces::InterfaceId;
use crate::routing::warmth::RouteWarmth;
use crate::units::InstantMillis;

#[cfg(feature = "alloc")]
use super::HeapDepartedInterfaceTable;

type Ledger = DepartedInterfaces<FixedDepartedInterfaceTable<4>>;

fn iface(byte: u8) -> InterfaceId {
    InterfaceId::new([byte; 8])
}

#[test]
fn a_may_return_departure_is_warm_for_the_grace_and_a_forgotten_one_is_not() {
    let mut ledger = Ledger::default();
    ledger.record(iface(1), Departure::MayReturn, InstantMillis(1_000));
    assert_eq!(
        ledger.warm_until(iface(1)),
        Some(InstantMillis(1_000 + DEPARTED_INTERFACE_GRACE_MS)),
    );

    ledger.record(iface(1), Departure::Forgotten, InstantMillis(2_000));
    assert_eq!(
        ledger.warm_until(iface(1)),
        None,
        "a deliberate forget revokes the earlier bounce's grace",
    );
}

#[test]
fn a_repeat_departure_re_arms_the_grace_instead_of_stacking_rows() {
    let mut ledger = Ledger::default();
    ledger.record(iface(1), Departure::MayReturn, InstantMillis(1_000));
    ledger.record(iface(1), Departure::MayReturn, InstantMillis(50_000));
    assert_eq!(
        ledger.warm_until(iface(1)),
        Some(InstantMillis(50_000 + DEPARTED_INTERFACE_GRACE_MS)),
    );
}

#[test]
fn a_full_ledger_evicts_the_soonest_expiring_row_for_the_newcomer() {
    let mut ledger = Ledger::default();
    for n in 0..4u8 {
        ledger.record(
            iface(n),
            Departure::MayReturn,
            InstantMillis(1_000 + u64::from(n)),
        );
    }
    ledger.record(iface(0xFF), Departure::MayReturn, InstantMillis(2_000));
    assert_eq!(
        ledger.warm_until(iface(0)),
        None,
        "the row closest to expiry made room",
    );
    assert!(ledger.warm_until(iface(0xFF)).is_some());
    assert!(ledger.warm_until(iface(1)).is_some());
}

#[test]
fn expired_rows_are_swept() {
    let mut ledger = Ledger::default();
    ledger.record(iface(1), Departure::MayReturn, InstantMillis(1_000));
    ledger.evict_expired(InstantMillis(1_000 + DEPARTED_INTERFACE_GRACE_MS));
    assert_eq!(ledger.warm_until(iface(1)), None);
}

#[cfg(feature = "alloc")]
#[test]
fn heap_columns_hold_a_mass_departure_no_fixed_ledger_could() {
    let mut ledger: DepartedInterfaces<HeapDepartedInterfaceTable> = DepartedInterfaces::default();
    for n in 0..64u8 {
        ledger.record(iface(n), Departure::MayReturn, InstantMillis(1_000));
    }
    assert!(ledger.warm_until(iface(0)).is_some());
    assert!(ledger.warm_until(iface(63)).is_some());
}
