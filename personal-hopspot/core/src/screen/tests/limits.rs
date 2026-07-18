use super::*;

#[test]
fn limit_rows_use_the_supplied_storage_limits() {
    let rows = build_limit_rows(DisplayedStorageLimits {
        upstream_app_destinations: StorageCapacity::Fixed(4),
        held_identities: StorageCapacity::Fixed(2),
        blackholed_identities: StorageCapacity::Fixed(8),
        blackhole_reason_bytes: StorageCapacity::Fixed(64),
        ..DisplayedStorageLimits::DYNAMIC
    });

    let app_dst = rows
        .iter()
        .find(|row| row.label == "AppDst")
        .map(|row| row.value);
    let held_id = rows
        .iter()
        .find(|row| row.label == "HeldID")
        .map(|row| row.value);
    let blackholes = rows
        .iter()
        .find(|row| row.label == "BlkHole")
        .map(|row| row.value);
    let blackhole_reason_bytes = rows
        .iter()
        .find(|row| row.label == "BlkWhy")
        .map(|row| row.value);

    assert_eq!(app_dst, Some(LimitValue::Count(4)));
    assert_eq!(held_id, Some(LimitValue::Count(2)));
    assert_eq!(blackholes, Some(LimitValue::Count(8)));
    assert_eq!(blackhole_reason_bytes, Some(LimitValue::Bytes(64)));
}
