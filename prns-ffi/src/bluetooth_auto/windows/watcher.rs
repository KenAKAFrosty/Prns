use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use prns_core::interfaces::bluetooth_auto::{BleAddress, BLE_SERVICE_UUID};
use tokio::sync::mpsc as tokio_mpsc;
use windows::core::GUID;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEAdvertisementWatcherStatus, BluetoothLEAdvertisementWatcherStoppedEventArgs,
    BluetoothLEScanningMode,
};
use windows::Devices::Bluetooth::BluetoothAddressType;
use windows::Foundation::TypedEventHandler;

use super::{guid_of, Event, WindowsBleError};

pub(super) fn build_watcher(
    events_tx: tokio_mpsc::UnboundedSender<Event>,
    adverts: Arc<AtomicU64>,
) -> Result<BluetoothLEAdvertisementWatcher, WindowsBleError> {
    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;

    // No OS-level service-UUID filter: it only matches the *primary* advertisement, so a peer that
    // carries the 128-bit UUID in its scan response slips past it (a likely cause of missed
    // sightings). Instead, count every advert (radio-liveness signal) and match the service UUID in
    // the handler. Compute the target GUID once here, not per packet (this fires for every BLE
    // advertisement in range).
    let target = guid_of(BLE_SERVICE_UUID);
    watcher.Received(&TypedEventHandler::new(
        move |_sender, args: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
            if let Some(args) = args.as_ref() {
                adverts.fetch_add(1, Ordering::Relaxed);
                if let Some(sighting) = sighting_from(args, target) {
                    let _ = events_tx.send(sighting);
                }
            }
            Ok(())
        },
    ))?;

    // The OS can Stop/Abort the watcher on a radio hiccup; without this it dies silently and the node
    // looks dormant forever. Log the cause and restart it. (The handler holds a clone of the watcher,
    // a deliberate cycle — the watcher is process-lifetime anyway.)
    let restart = watcher.clone();
    watcher.Stopped(&TypedEventHandler::new(
        move |_sender: &Option<BluetoothLEAdvertisementWatcher>,
              args: &Option<BluetoothLEAdvertisementWatcherStoppedEventArgs>| {
            let error = args.as_ref().and_then(|args| args.Error().ok());
            crate::diagnostic_log::warn!(
                "bluetooth: advertisement watcher stopped (error {error:?}) — restarting"
            );
            if let Err(err) = restart.Start() {
                crate::diagnostic_log::error!("bluetooth: watcher restart failed ({err:?})");
            }
            Ok(())
        },
    ))?;
    Ok(watcher)
}

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const SCAN_STALL_TICKS: u32 = 3;

pub(super) fn spawn_watcher_heartbeat(
    watcher: BluetoothLEAdvertisementWatcher,
    adverts: Arc<AtomicU64>,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
        let mut last_seen = adverts.load(Ordering::Relaxed);
        let mut quiet_ticks = 0u32;
        loop {
            tick.tick().await;
            let seen = adverts.load(Ordering::Relaxed);
            let status = match watcher.Status() {
                Ok(status) => status,
                Err(error) => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: scanner status unreadable ({error:?})"
                    );
                    break;
                }
            };
            crate::diagnostic_log::debug!(
                "bluetooth: scanner status {status:?}, {seen} adverts seen so far"
            );

            let started = status == BluetoothLEAdvertisementWatcherStatus::Started;
            quiet_ticks = if started && seen == last_seen {
                quiet_ticks + 1
            } else {
                0
            };
            last_seen = seen;

            if quiet_ticks >= SCAN_STALL_TICKS {
                crate::diagnostic_log::warn!(
                    "bluetooth: scanner delivered no adverts for ~{}s while Started — kicking it",
                    SCAN_STALL_TICKS * HEARTBEAT_INTERVAL.as_secs() as u32
                );
                // Stop() drives the Stopped handler, which restarts the watcher cleanly (calling
                // Start() here directly would race the Stopping->Stopped transition).
                if let Err(error) = watcher.Stop() {
                    crate::diagnostic_log::error!(
                        "bluetooth: scanner kick (Stop) failed ({error:?})"
                    );
                }
                quiet_ticks = 0;
            }
        }
    });
}

fn sighting_from(args: &BluetoothLEAdvertisementReceivedEventArgs, target: GUID) -> Option<Event> {
    let advertised = args
        .Advertisement()
        .ok()?
        .ServiceUuids()
        .ok()?
        .into_iter()
        .any(|uuid| uuid == target);
    if !advertised {
        return None;
    }
    let address = args.BluetoothAddress().ok()?;
    let bytes = address.to_be_bytes();
    let octets = [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]];
    let rssi = args
        .RawSignalStrengthInDBm()
        .ok()
        .and_then(|dbm| i8::try_from(dbm).ok());
    let address_type = args
        .BluetoothAddressType()
        .ok()
        .unwrap_or(BluetoothAddressType::Unspecified);
    Some(Event::Sighting {
        address: BleAddress::new(octets),
        address_type,
        rssi,
    })
}
