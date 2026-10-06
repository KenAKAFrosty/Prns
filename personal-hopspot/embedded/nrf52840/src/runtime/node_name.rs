//! The node's display name: a controller sets it with Remote Control, the embedded journal keeps
//! it, and both Hopspot announces carry it. Until a name is stored the board's built-in name is
//! used, so a board that is never renamed behaves exactly as before.

use core::cell::Cell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Timer};
use personal_hopspot_core as hopspot;
use personal_rns::engine::SetRegisteredAnnounceAppData;
use personal_rns::remote_control::{RemoteControlApplyOutcome, RemoteControlNodeName};
use personal_rns::runtime::{
    restored_node_name, restored_node_name_now, store_node_name, PrnsNodeHandle,
    RemoteControlHostCommandError,
};

#[cfg(not(any(feature = "board-t-echo", feature = "board-mesh-pocket")))]
use super::headless::{COMMANDS, COMPLETION};
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
use super::node::{COMMANDS, COMPLETION};
use crate::boards::selected as board;

const APPLY_ATTEMPTS: u8 = 20;
const APPLY_RETRY: Duration = Duration::from_millis(100);

static DESTINATIONS: Mutex<
    CriticalSectionRawMutex,
    Cell<Option<hopspot::HopspotDestinationHashes>>,
> = Mutex::new(Cell::new(None));

pub(super) fn set_destinations(destinations: hopspot::HopspotDestinationHashes) {
    DESTINATIONS.lock(|cell| cell.set(Some(destinations)));
}

fn default_name() -> RemoteControlNodeName {
    core::str::from_utf8(board::NODE_ANNOUNCE_APP_DATA)
        .ok()
        .and_then(RemoteControlNodeName::new)
        .expect("the board's built-in node name is a valid node name")
}

/// The stored name, or the board's built-in name before one is stored.
pub(super) fn current() -> RemoteControlNodeName {
    restored_node_name_now()
        .flatten()
        .unwrap_or_else(default_name)
}

/// Point both Hopspot announces at `name`.
async fn apply(name: &RemoteControlNodeName) -> Result<(), ()> {
    let destinations = DESTINATIONS.lock(Cell::get).ok_or(())?;
    let handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let delivery = hopspot::named_delivery_announce_app_data(name);
    for (destination, app_data) in [
        (destinations.delivery, delivery.as_slice()),
        (
            destinations.node_page,
            hopspot::named_node_announce_app_data(name),
        ),
    ] {
        let app_data = app_data.try_into().map_err(|_| ())?;
        handle
            .set_registered_announce_app_data(SetRegisteredAnnounceAppData {
                destination,
                app_data,
            })
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

/// A durable name still needs reapplying after a failed or interrupted announce update.
pub(super) async fn set(
    name: RemoteControlNodeName,
) -> Result<RemoteControlApplyOutcome, RemoteControlHostCommandError> {
    hopspot::apply_remote_node_name(
        restored_node_name().await,
        name,
        async |name| {
            store_node_name(name)
                .await
                .map_err(|_| RemoteControlHostCommandError::PersistenceFailed)
        },
        async |name| {
            apply(&name)
                .await
                .map_err(|()| RemoteControlHostCommandError::ApplyFailed)
        },
    )
    .await
}

/// Re-apply a stored name once the journal has been restored at boot. Awaited at the start of
/// the Remote Control loop rather than in its own task, so it shares that loop's future memory.
pub(super) async fn restore() {
    let Some(name) = restored_node_name().await else {
        return;
    };
    // The node may still be starting; its command channel accepts the change once it runs.
    for _ in 0..APPLY_ATTEMPTS {
        if apply(&name).await.is_ok() {
            return;
        }
        Timer::after(APPLY_RETRY).await;
    }
}
