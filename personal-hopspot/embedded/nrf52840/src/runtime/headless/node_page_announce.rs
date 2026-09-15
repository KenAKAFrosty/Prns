//! The self-announce of a board with no button and no screen. Display boards announce from a
//! menu action and the MeshTower from its button; without this the T1000-E and the Solar Node
//! relay traffic but no peer ever learns they exist.

use core::future::Future;

use embassy_time::{Duration, Timer};
use personal_hopspot_core::headless_announce::headless_announce_delay_ms;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::wire::DestinationHash;

use super::{PrnsNodeHandle, COMMANDS, COMPLETION};

/// Announces `node_page_destination` on every interface on the headless schedule, forever.
pub(super) fn announce_forever(node_page_destination: DestinationHash) -> impl Future<Output = ()> {
    let announce_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    async move {
        let mut announces_sent: u32 = 0;
        loop {
            Timer::after(Duration::from_millis(headless_announce_delay_ms(
                announces_sent,
            )))
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
    }
}
