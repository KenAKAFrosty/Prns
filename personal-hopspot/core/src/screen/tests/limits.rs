use super::*;

use crate::screen::render::layout::{FONT_5X8_CHAR_W, LIMITS_TEXT_X, WIDTH};
use crate::screen::render::menus::limits_row_text;

/// How many FONT_5X8 characters of a limits row the panel can actually draw.
/// The row buffer is deliberately wider than this so a long row is truncated
/// rather than lost, but nothing reconciled the two and a six digit capacity
/// ran off the right edge. Derived from the same three layout constants the
/// renderer draws with, so this cannot drift away from the screen.
const LIMITS_ROW_MAX_CHARS: usize = ((WIDTH - LIMITS_TEXT_X) / FONT_5X8_CHAR_W) as usize;

fn assert_every_row_fits(limits: DisplayedStorageLimits) {
    for row in build_limit_rows(limits) {
        let text = limits_row_text(row);
        let drawn = text.chars().count();
        assert!(
            drawn <= LIMITS_ROW_MAX_CHARS,
            "row {:?} renders {drawn} characters as {text:?}, panel draws {LIMITS_ROW_MAX_CHARS}",
            row.label,
        );
    }
}

/// The bug this pins: on every heap platform `packet_hashes` is the single
/// Fixed capacity and the largest number anywhere on the face, so the row came
/// out as `PktHash 500000` - fourteen FONT_5X8 characters against a panel that
/// draws twelve. The fourth zero was sliced down the middle on a real device,
/// and nothing here or in the renderer noticed, because the row buffer is
/// sized in bytes while the panel is sized in pixels.
///
/// Swept across every magnitude a `u32` capacity can hold rather than pinned to
/// the 500,000 that broke, so this cannot quietly stop covering `GrowableHeap`
/// if that constant moves.
///
/// One piece of headroom worth naming out loud: `Receipts` is the longest label
/// at eight characters, which leaves room for a three character value and no
/// more. Nothing declares a four figure receipt table today. If something does,
/// this is the test that will say so.
#[test]
fn every_limits_row_fits_the_panel() {
    assert_every_row_fits(DisplayedStorageLimits::DYNAMIC);

    for hashes in [8usize, 999, 1_000, 500_000, u32::MAX as usize] {
        assert_every_row_fits(DisplayedStorageLimits {
            packet_hashes: StorageCapacity::Fixed(hashes),
            ..DisplayedStorageLimits::DYNAMIC
        });
    }

    // Board sized: the embedded layouts declare small fixed capacities, and
    // those must keep rendering as exact numbers rather than being compacted.
    assert_every_row_fits(DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(36),
        announce_records: StorageCapacity::Fixed(36),
        upstream_app_destinations: StorageCapacity::Fixed(4),
        held_identities: StorageCapacity::Fixed(2),
        receipts: StorageCapacity::Fixed(8),
        packet_hashes: StorageCapacity::Fixed(64),
        blackholed_identities: StorageCapacity::Fixed(8),
        ..DisplayedStorageLimits::DYNAMIC
    });
}

/// A capacity a board can state exactly is still stated exactly. Compacting
/// numbers to fit the panel must not cost precision where there is room for it.
#[test]
fn small_capacities_are_not_compacted() {
    let rows = build_limit_rows(DisplayedStorageLimits {
        packet_hashes: StorageCapacity::Fixed(64),
        ..DisplayedStorageLimits::DYNAMIC
    });
    let row = rows
        .iter()
        .find(|row| row.label == "PktHash")
        .copied()
        .unwrap();
    assert_eq!(limits_row_text(row).as_str(), "PktHash 64");
}

/// And the capacity that overflowed now reads as the face's own count format.
#[test]
fn the_heap_packet_hash_capacity_fits_as_a_compact_count() {
    let rows = build_limit_rows(DisplayedStorageLimits {
        packet_hashes: StorageCapacity::Fixed(500_000),
        ..DisplayedStorageLimits::DYNAMIC
    });
    let row = rows
        .iter()
        .find(|row| row.label == "PktHash")
        .copied()
        .unwrap();
    assert_eq!(limits_row_text(row).as_str(), "PktHash 500K");
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
