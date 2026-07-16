use super::*;
use crate::identity::IdentityHash;
use crate::units::InstantMillis;

type FixedContractTable = FixedBlackholeTable<4, { blackhole_index_buckets(4) }, 16>;

fn identity(byte: u8) -> IdentityHash {
    IdentityHash::new([byte; 16])
}

fn numbered_identity(number: u32) -> IdentityHash {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&number.to_be_bytes());
    IdentityHash::new(bytes)
}

fn entry(
    identity_byte: u8,
    source_byte: u8,
    expiry: BlackholeExpiry,
    reason: Option<&str>,
) -> BlackholedIdentity<&str> {
    BlackholedIdentity {
        identity: identity(identity_byte),
        source: identity(source_byte),
        expiry,
        reason,
    }
}

fn assert_shared_contract<Table>()
where
    Table: BlackholeTable + Default,
{
    let mut blackholes = IdentityBlackholes::<Table>::default();
    let first = entry(
        1,
        9,
        BlackholeExpiry::At(InstantMillis(500)),
        Some("operator"),
    );
    assert!(matches!(
        blackholes.blackhole_identity(first.clone()),
        Ok(BlackholeIdentityOutcome::Added)
    ));
    assert!(matches!(
        blackholes.blackhole_identity(entry(1, 8, BlackholeExpiry::Indefinite, None)),
        Ok(BlackholeIdentityOutcome::AlreadyPresent)
    ));
    assert!(blackholes.is_blackholed(&identity(1)));
    assert_eq!(blackholes.entries().collect::<std::vec::Vec<_>>(), [first]);
    assert_eq!(blackholes.earliest_expiry_at(), Some(InstantMillis(501)));
    assert_eq!(
        blackholes.unblackhole_identity(&identity(1)),
        UnblackholeIdentityOutcome::Removed
    );
    assert_eq!(
        blackholes.unblackhole_identity(&identity(1)),
        UnblackholeIdentityOutcome::NotFound
    );
    assert!(blackholes.is_empty());

    for (byte, expiry) in [
        (1, BlackholeExpiry::At(InstantMillis(99))),
        (2, BlackholeExpiry::At(InstantMillis(100))),
        (3, BlackholeExpiry::Indefinite),
    ] {
        assert!(matches!(
            blackholes.blackhole_identity(entry(byte, 9, expiry, None)),
            Ok(BlackholeIdentityOutcome::Added)
        ));
    }
    assert_eq!(blackholes.cull_expired(InstantMillis(100)), 1);
    assert_eq!(blackholes.earliest_expiry_at(), Some(InstantMillis(101)));
    assert!(!blackholes.is_blackholed(&identity(1)));
    assert!(blackholes.is_blackholed(&identity(2)));
    assert!(blackholes.is_blackholed(&identity(3)));
    assert_eq!(blackholes.cull_expired(InstantMillis(101)), 1);
    assert!(blackholes.is_blackholed(&identity(3)));
    assert_eq!(blackholes.earliest_expiry_at(), None);
}

#[test]
fn the_last_representable_deadline_never_becomes_expired() {
    let expiry = BlackholeExpiry::At(InstantMillis(u64::MAX));
    assert_eq!(expiry.first_expired_at(), None);
    assert!(!expiry.is_expired_at(InstantMillis(u64::MAX)));
}

#[test]
fn fixed_storage_honors_the_shared_contract() {
    assert_shared_contract::<FixedContractTable>();
}

#[cfg(feature = "alloc")]
#[test]
fn heap_storage_honors_the_shared_contract() {
    assert_shared_contract::<HeapBlackholeTable>();
}

#[cfg(feature = "external-alloc")]
#[test]
fn fixed_heap_storage_honors_the_shared_contract() {
    type Table = FixedHeapBlackholeTable<4, { blackhole_index_buckets(4) }, 16>;

    assert_shared_contract::<Table>();
}

fn assert_fixed_bounds<Table>()
where
    Table: BlackholeTable<InsertError = FixedBlackholeInsertError> + Default,
{
    let mut blackholes = IdentityBlackholes::<Table>::default();
    assert_eq!(
        blackholes.blackhole_identity(entry(1, 9, BlackholeExpiry::Indefinite, Some("longer"),)),
        Err(FixedBlackholeInsertError::ReasonTooLong)
    );
    assert!(blackholes.is_empty());
    for byte in 1..=2 {
        assert_eq!(
            blackholes.blackhole_identity(entry(byte, 9, BlackholeExpiry::Indefinite, None)),
            Ok(BlackholeIdentityOutcome::Added)
        );
    }
    assert_eq!(
        blackholes.blackhole_identity(entry(3, 9, BlackholeExpiry::Indefinite, None)),
        Err(FixedBlackholeInsertError::TableFull)
    );
    assert!(!blackholes.is_blackholed(&identity(3)));
}

#[test]
fn fixed_storage_bounds_fail_without_partial_insertion() {
    type Table = FixedBlackholeTable<2, { blackhole_index_buckets(2) }, 4>;

    assert_fixed_bounds::<Table>();
}

#[cfg(feature = "external-alloc")]
#[test]
fn fixed_heap_storage_bounds_fail_without_partial_insertion() {
    type Table = FixedHeapBlackholeTable<2, { blackhole_index_buckets(2) }, 4>;

    assert_fixed_bounds::<Table>();
}

#[cfg(feature = "alloc")]
#[test]
fn heap_storage_grows_without_entry_or_reason_policy_limits() {
    let mut blackholes = IdentityBlackholes::<HeapBlackholeTable>::default();
    for number in 0..256 {
        let entry = BlackholedIdentity {
            identity: numbered_identity(number),
            source: identity(9),
            expiry: BlackholeExpiry::Indefinite,
            reason: None,
        };
        assert_eq!(
            blackholes.blackhole_identity(entry),
            Ok(BlackholeIdentityOutcome::Added)
        );
    }
    for number in (0..256).step_by(2) {
        assert_eq!(
            blackholes.unblackhole_identity(&numbered_identity(number)),
            UnblackholeIdentityOutcome::Removed
        );
    }
    for number in 0..256 {
        assert_eq!(
            blackholes.is_blackholed(&numbered_identity(number)),
            number % 2 == 1
        );
    }

    let reason = "r".repeat(8_192);
    let identity = numbered_identity(1_000);
    assert_eq!(
        blackholes.blackhole_identity(BlackholedIdentity {
            identity,
            source: numbered_identity(9),
            expiry: BlackholeExpiry::Indefinite,
            reason: Some(reason.as_str()),
        }),
        Ok(BlackholeIdentityOutcome::Added)
    );
    assert_eq!(
        blackholes
            .entries()
            .find(|entry| entry.identity == identity)
            .and_then(|entry| entry.reason),
        Some(reason.as_str())
    );
}
