use super::*;

fn limit_row(limits: DisplayedStorageLimits, label: &str) -> LimitRow {
    build_limit_rows(limits)
        .iter()
        .find(|row| row.label == label)
        .copied()
        .expect("limit row exists")
}

fn assert_every_row_fits(limits: DisplayedStorageLimits) {
    for row in build_limit_rows(limits) {
        let text = limits_row_text(row);
        let bounds = limits_row_drawable(&text, 0).bounding_box();
        let right = bounds
            .bottom_right()
            .expect("limit row has nonzero bounds")
            .x;
        assert!(
            right < WIDTH,
            "row {text:?} reaches x={right}, panel ends at x={}",
            WIDTH - 1
        );
    }
}

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

#[test]
fn limit_counts_use_compact_formatting() {
    for (value, expected) in [
        (64, "PktHash 64"),
        (500_000, "PktHash 500K"),
        (u32::MAX, "PktHash 4.2B"),
    ] {
        let limits = DisplayedStorageLimits {
            packet_hashes: StorageCapacity::Fixed(value as usize),
            ..DisplayedStorageLimits::DYNAMIC
        };
        assert_eq!(
            limits_row_text(limit_row(limits, "PktHash")).as_str(),
            expected
        );
    }

    let limits = DisplayedStorageLimits {
        receipts: StorageCapacity::Fixed(1_000),
        ..DisplayedStorageLimits::DYNAMIC
    };
    assert_eq!(
        limits_row_text(limit_row(limits, "Rcpts")).as_str(),
        "Rcpts 1.0K"
    );
}

#[test]
fn limits_rows_fit_the_panel() {
    assert_every_row_fits(DisplayedStorageLimits::DYNAMIC);

    let max_count = StorageCapacity::Fixed(u32::MAX as usize);
    let max_counts = DisplayedStorageLimits {
        tracked_destinations: max_count,
        destination_identities: max_count,
        announce_records: max_count,
        upstream_app_destinations: max_count,
        held_identities: max_count,
        links: max_count,
        channels: max_count,
        channel_window_pool: Some(u32::MAX as usize),
        channel_reorder_depth: max_count,
        receipts: max_count,
        packet_hashes: max_count,
        blackholed_identities: max_count,
        reverse_routes: max_count,
        pending_path_requests: max_count,
        held_announces: max_count,
        ratchets_per_destination: max_count,
        ..DisplayedStorageLimits::DYNAMIC
    };
    assert_every_row_fits(max_counts);
    assert_every_row_fits(DisplayedStorageLimits {
        channel_window_pool: None,
        ..max_counts
    });
    assert_every_row_fits(DisplayedStorageLimits {
        link_mtu: StorageCapacity::Fixed(1_024),
        resource_transfer_bytes: StorageCapacity::Fixed(1_504),
        blackhole_reason_bytes: StorageCapacity::Fixed(64),
        ..DisplayedStorageLimits::DYNAMIC
    });
}
