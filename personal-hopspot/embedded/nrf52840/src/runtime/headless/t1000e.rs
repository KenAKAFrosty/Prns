use core::future::Future;

use embassy_futures::join::join3;
use personal_hopspot_core as hopspot;

use crate::boards::selected as board;

use super::super::heartbeat::{self, HeartbeatTiming};

pub(super) const INTERFACE_CAPACITY: usize = 2;
pub(super) const LANE_COUNT: usize = INTERFACE_CAPACITY;

const GNSS_FIXED_HEARTBEAT: HeartbeatTiming = HeartbeatTiming::with_illuminated_millis(900);

pub(super) fn heartbeat_timing() -> &'static HeartbeatTiming {
    if matches!(board::gnss_snapshot(), hopspot::GnssSnapshot::Fixed(_)) {
        &GNSS_FIXED_HEARTBEAT
    } else {
        &heartbeat::NORMAL
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
