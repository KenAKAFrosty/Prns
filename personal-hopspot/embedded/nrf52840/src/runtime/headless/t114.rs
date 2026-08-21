use core::future::Future;

use embassy_futures::join::join;

use crate::boards::selected as board;

pub(super) const INTERFACE_CAPACITY: usize = 2;
pub(super) const LANE_COUNT: usize = INTERFACE_CAPACITY;

pub(super) const fn heartbeat_illuminated_ms() -> u64 {
    100
}

pub(super) async fn maintain() {
    board::maintain().await;
}

pub(super) fn run<I, L>(io: I, lora: L) -> impl Future
where
    I: Future,
    L: Future,
{
    join(io, lora)
}
