use core::future::Future;

use embassy_futures::join::{join, join3};
use embassy_nrf::gpio::Input;
use embassy_time::{Duration, Timer};
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::wire::DestinationHash;

use crate::boards::selected as board;

use super::bluetooth;
use super::{PrnsNodeHandle, COMMANDS, COMPLETION};

pub(super) const INTERFACE_CAPACITY: usize = 2 + bluetooth::MEMBERS;
pub(super) const LANE_COUNT: usize = 3;

pub(super) const fn heartbeat_timing() -> &'static super::super::heartbeat::HeartbeatTiming {
    &super::super::heartbeat::NORMAL
}

pub(super) async fn maintain() {
    board::maintain().await;
}

pub(super) fn run<I, L, B>(
    io: I,
    lora: L,
    bluetooth: B,
    button: Input<'static>,
    node_page_destination: DestinationHash,
) -> impl Future
where
    I: Future,
    L: Future,
    B: Future,
{
    let announce_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let announce = async move {
        loop {
            board::BUTTON_PRESSES.receive().await;
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
        }
    };
    join(
        join3(io, bluetooth, lora),
        join(board::drive_button(button), announce),
    )
}
