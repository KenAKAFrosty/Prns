//! Latest board power observation for remote-control `DescribePower`.
//!
//! Display / ADC loops publish here; `HopspotRemoteControlState::power_snapshot` reads it.

use core::sync::atomic::{AtomicU16, Ordering};

use crate::PowerSnapshot;

static LATEST: AtomicU16 = AtomicU16::new(pack(PowerSnapshot::UNKNOWN));

const fn pack(snapshot: PowerSnapshot) -> u16 {
    let external = snapshot.external_power().wire_tag() as u16;
    let percent = match snapshot.battery() {
        Some(percent) => percent.get() as u16,
        None => 0xFF,
    };
    (external << 8) | percent
}

fn unpack(packed: u16) -> PowerSnapshot {
    let bytes = [(packed >> 8) as u8, packed as u8];
    PowerSnapshot::parse(&bytes)
        .map(|(snapshot, _)| snapshot)
        .unwrap_or(PowerSnapshot::UNKNOWN)
}

/// Publish the newest OLED/ADC power reading for remote-control queries.
pub fn publish_power_snapshot(snapshot: PowerSnapshot) {
    LATEST.store(pack(snapshot), Ordering::Relaxed);
}

/// Latest published power reading, or [`PowerSnapshot::UNKNOWN`] before the first sample.
#[must_use]
pub fn latest_power_snapshot() -> PowerSnapshot {
    unpack(LATEST.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BatteryPercent, ChargingState, ExternalPowerState};

    #[test]
    fn published_snapshot_round_trips() {
        let snapshot = PowerSnapshot::new(
            Some(BatteryPercent::saturating(73)),
            ExternalPowerState::Present {
                charging: ChargingState::Charging,
            },
        );
        publish_power_snapshot(snapshot);
        assert_eq!(latest_power_snapshot(), snapshot);
        publish_power_snapshot(PowerSnapshot::UNKNOWN);
        assert_eq!(latest_power_snapshot(), PowerSnapshot::UNKNOWN);
    }
}
