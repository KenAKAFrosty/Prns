mod hardware;
mod identity;

use embassy_nrf::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use personal_rns::interfaces::InterfaceId;

pub(crate) use crate::storage::Nrf52840Storage as Storage;
pub(crate) use hardware::{
    MeshTowerV2Board as Board, MeshTowerV2Hardware as Hardware,
    MeshTowerV2LoraInterface as LoraInterface,
};
pub(crate) use identity::{bootstrap_ble_identity, bootstrap_node_identity};

pub(crate) const JOURNAL_LAYOUT: personal_rns::persistence::FlashJournalLayout =
    personal_hopspot_core::MESH_TOWER_V2_JOURNAL_LAYOUT;
pub(crate) const REMOTE_CONTROL_IDENTITY_FLASH: super::RemoteControlIdentityFlash =
    super::RemoteControlIdentityFlash::at(
        personal_hopspot_core::MESH_TOWER_V2_REMOTE_CONTROL_IDENTITY_FLASH_OFFSET,
    );
pub(crate) const USB_MANUFACTURER: &str = "Stay Personal";
pub(crate) const USB_PRODUCT: &str = "Personal Hopspot (Heltec MeshTower V2)";
pub(crate) const USB_SERIAL_NUMBER: &str = "PERSONAL-RNS-MTWR-HOP";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"mtowerv2");
pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x1dPersonal Hopspot MeshTower V2\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot MeshTower V2";

const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

pub(crate) static BUTTON_PRESSES: Channel<CriticalSectionRawMutex, (), 4> = Channel::new();

pub(crate) async fn maintain() {
    hardware::pet_watchdog();
    Timer::after(Duration::from_millis(1)).await;
    hardware::release_watchdog();
}

/// MeshTower user button is P1.10, active-low with pull-up. Any press announces.
pub(crate) async fn drive_button(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        Timer::after(BUTTON_DEBOUNCE).await;
        if !button.is_low() {
            continue;
        }
        BUTTON_PRESSES.send(()).await;
        button.wait_for_rising_edge().await;
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}
