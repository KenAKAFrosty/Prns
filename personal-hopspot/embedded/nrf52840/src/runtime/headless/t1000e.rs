use core::future::Future;

use embassy_futures::join::join3;
use personal_hopspot_core as hopspot;

use crate::boards::selected as board;

pub(super) const INTERFACE_CAPACITY: usize = 2;
pub(super) const LANE_COUNT: usize = INTERFACE_CAPACITY;

pub(super) fn heartbeat_illuminated_ms() -> u64 {
    if matches!(board::gnss_snapshot(), hopspot::GnssSnapshot::Fixed(_)) {
        900
    } else {
        100
    }
}

pub(super) async fn maintain() {}

pub(super) fn run<I, L>(io: I, lora: L, gnss: board::Gnss) -> impl Future
where
    I: Future,
    L: Future,
{
    board::control_gnss(hopspot::GnssReceiverCommand::Enable);
    join3(io, lora, board::drive_gnss(gnss))
}
