use heapless::Vec as HVec;
use personal_rns::routing::links::channel::{channel_mdu, ChannelWindow};
use personal_rns::routing::links::data::link_mdu;
use personal_rns::routing::links::resources::max_part_count;
use personal_rns::routing::links::resources::{
    PART_REQUEST_MAX_RETRIES, RATE_FAST_BYTES_PER_SECOND, WINDOW_MAX, WINDOW_START,
};
use personal_rns::routing::links::MAX_LINK_MTU;
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity};

pub(in crate::screen) const LIMITS_PER_PAGE: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum LimitValue {
    Count(u32),
    Bytes(u64),
    Range(u32, u32),
    RateBytesPerSec(u64),
    Text(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) struct LimitRow {
    pub(in crate::screen) label: &'static str,
    pub(in crate::screen) value: LimitValue,
}

impl LimitRow {
    const fn count(label: &'static str, value: u32) -> Self {
        Self {
            label,
            value: LimitValue::Count(value),
        }
    }

    const fn bytes(label: &'static str, value: u64) -> Self {
        Self {
            label,
            value: LimitValue::Bytes(value),
        }
    }

    const fn range(label: &'static str, low: u32, high: u32) -> Self {
        Self {
            label,
            value: LimitValue::Range(low, high),
        }
    }

    const fn rate(label: &'static str, value: u64) -> Self {
        Self {
            label,
            value: LimitValue::RateBytesPerSec(value),
        }
    }

    const fn text(label: &'static str, value: &'static str) -> Self {
        Self {
            label,
            value: LimitValue::Text(value),
        }
    }
}

const LIMIT_ROW_CAPACITY: usize = 24;

fn limit_count(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

fn capacity_row(label: &'static str, capacity: StorageCapacity) -> LimitRow {
    match capacity {
        StorageCapacity::Fixed(value) => LimitRow::count(label, limit_count(value)),
        StorageCapacity::Dynamic => LimitRow::text(label, "dyn"),
    }
}

fn push_limit_row(rows: &mut HVec<LimitRow, LIMIT_ROW_CAPACITY>, row: LimitRow) {
    let _ = rows.push(row);
}

pub(in crate::screen) fn build_limit_rows(
    limits: DisplayedStorageLimits,
) -> HVec<LimitRow, LIMIT_ROW_CAPACITY> {
    let mut rows = HVec::new();
    push_limit_row(&mut rows, capacity_row("Dst", limits.tracked_destinations));
    push_limit_row(&mut rows, capacity_row("Ann", limits.announce_records));
    push_limit_row(
        &mut rows,
        capacity_row("AppDst", limits.upstream_app_destinations),
    );
    push_limit_row(&mut rows, capacity_row("Links", limits.links));
    push_limit_row(&mut rows, capacity_row("Chans", limits.channels));
    if let Some(pool) = limits.channel_window_pool {
        push_limit_row(&mut rows, LimitRow::count("ChPool", limit_count(pool)));
    } else {
        push_limit_row(
            &mut rows,
            capacity_row("Reorder", limits.channel_reorder_depth),
        );
    }
    match limits.link_mtu {
        StorageCapacity::Fixed(mtu) => {
            push_limit_row(&mut rows, LimitRow::bytes("MTU", mtu as u64));
            push_limit_row(&mut rows, LimitRow::bytes("LinkMDU", link_mdu(mtu) as u64));
            push_limit_row(
                &mut rows,
                LimitRow::bytes("ChanMDU", channel_mdu(mtu) as u64),
            );
        }
        StorageCapacity::Dynamic => {
            push_limit_row(&mut rows, LimitRow::bytes("MaxMTU", MAX_LINK_MTU as u64));
        }
    }
    match limits.resource_transfer_bytes {
        StorageCapacity::Fixed(bytes) => {
            push_limit_row(&mut rows, LimitRow::bytes("ResBuf", bytes as u64));
            push_limit_row(
                &mut rows,
                LimitRow::count("ResPart", limit_count(max_part_count(bytes))),
            );
        }
        StorageCapacity::Dynamic => push_limit_row(&mut rows, LimitRow::text("ResBuf", "dyn")),
    }
    push_limit_row(
        &mut rows,
        LimitRow::range("ResWin", WINDOW_START as u32, WINDOW_MAX as u32),
    );
    push_limit_row(
        &mut rows,
        LimitRow::count("Retry", PART_REQUEST_MAX_RETRIES as u32),
    );
    push_limit_row(
        &mut rows,
        LimitRow::rate("Fast", RATE_FAST_BYTES_PER_SECOND),
    );
    push_limit_row(&mut rows, capacity_row("Rcpts", limits.receipts));
    push_limit_row(&mut rows, capacity_row("PktHash", limits.packet_hashes));
    push_limit_row(
        &mut rows,
        capacity_row("BlkHole", limits.blackholed_identities),
    );
    match limits.blackhole_reason_bytes {
        StorageCapacity::Fixed(bytes) => {
            push_limit_row(&mut rows, LimitRow::bytes("BlkWhy", bytes as u64));
        }
        StorageCapacity::Dynamic => push_limit_row(&mut rows, LimitRow::text("BlkWhy", "dyn")),
    }
    push_limit_row(&mut rows, capacity_row("RevRte", limits.reverse_routes));
    push_limit_row(
        &mut rows,
        capacity_row("PathReq", limits.pending_path_requests),
    );
    push_limit_row(&mut rows, capacity_row("HeldAnn", limits.held_announces));
    push_limit_row(&mut rows, capacity_row("HeldID", limits.held_identities));
    push_limit_row(
        &mut rows,
        capacity_row("Ratchet", limits.ratchets_per_destination),
    );
    push_limit_row(
        &mut rows,
        LimitRow::range(
            "ChanWin",
            ChannelWindow::MIN as u32,
            ChannelWindow::MAX_FAST as u32,
        ),
    );
    rows
}

pub(in crate::screen) fn limit_page_count(rows: &[LimitRow]) -> usize {
    rows.len().max(1).div_ceil(LIMITS_PER_PAGE)
}

pub(in crate::screen) fn storage_limit_page_count(limits: DisplayedStorageLimits) -> usize {
    let rows = build_limit_rows(limits);
    limit_page_count(&rows)
}
