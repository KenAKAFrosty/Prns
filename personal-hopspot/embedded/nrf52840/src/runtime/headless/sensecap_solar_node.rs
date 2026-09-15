use core::future::Future;

use embassy_futures::join::join4;
use embassy_time::{Duration, Timer};
use personal_hopspot_core as hopspot;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::wire::DestinationHash;

use crate::boards::selected as board;

use super::super::heartbeat::{self, HeartbeatTiming};
use super::{PrnsNodeHandle, COMMANDS, COMPLETION};

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

pub(super) fn run<I, L>(
    io: I,
    lora: L,
    gnss: board::Gnss,
    node_page_destination: DestinationHash,
) -> impl Future
where
    I: Future,
    L: Future,
{
    board::control_gnss(hopspot::GnssReceiverCommand::Enable);
    // No button or screen here, so the node page is announced on the headless schedule instead;
    // without it the node relays traffic but no peer ever learns it exists.
    let announce_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let announce = async move {
        let mut announces_sent: u32 = 0;
        loop {
            Timer::after(Duration::from_millis(
                hopspot::headless_announce::headless_announce_delay_ms(announces_sent),
            ))
            .await;
            while announce_handle
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: node_page_destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                Timer::after(Duration::from_millis(50)).await;
            }
            announces_sent = announces_sent.saturating_add(1);
        }
    };
    join4(io, lora, board::drive_gnss(gnss), announce)
}
