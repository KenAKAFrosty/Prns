//! The self-announce of a board with no button and no screen. Display boards announce from a
//! menu action and the MeshTower from its button; without this the T1000-E, the Solar Node and a
//! XIAO without a fitted button relay traffic but no peer ever learns they exist.

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

/// A peer must stay connected for two consecutive polls before it is announced to, so a link
/// that drops during setup does not cost an announce.
#[cfg(feature = "board-xiao-nrf52840")]
const BLUETOOTH_PEER_POLL: Duration = Duration::from_secs(2);

/// Announces `node_page_destination` to each Bluetooth peer once it has settled, so a board that
/// comes into range learns this node without waiting for the six-hour schedule. Only that peer's
/// interface is addressed, so peers coming and going do not cost LoRa airtime.
#[cfg(feature = "board-xiao-nrf52840")]
pub(super) fn announce_to_new_bluetooth_peers(
    node_page_destination: DestinationHash,
) -> impl Future<Output = ()> {
    use personal_rns::bluetooth_auto::BluetoothAutoStatus;
    use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceStatus};

    use super::bluetooth::{BLE_SHARED, MEMBERS};

    let announce_handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    async move {
        let status = BluetoothAutoStatus::new(&BLE_SHARED);
        let mut seen: heapless::Vec<InterfaceId, MEMBERS> = heapless::Vec::new();
        let mut announced: heapless::Vec<InterfaceId, MEMBERS> = heapless::Vec::new();
        loop {
            Timer::after(BLUETOOTH_PEER_POLL).await;
            let mut connected: heapless::Vec<InterfaceId, MEMBERS> = heapless::Vec::new();
            for member in status.members() {
                if member.connection() == ConnectionState::Connected {
                    let _ = connected.push(member.id());
                }
            }
            for &peer in &connected {
                if !seen.contains(&peer) || announced.contains(&peer) {
                    continue;
                }
                while announce_handle
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                        destination: node_page_destination,
                        target: AnnounceTarget::Interface(peer),
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    Timer::after(Duration::from_millis(50)).await;
                }
            }
            announced.retain(|peer| connected.contains(peer));
            for &peer in &connected {
                if seen.contains(&peer) && !announced.contains(&peer) {
                    let _ = announced.push(peer);
                }
            }
            seen = connected;
        }
    }
}
