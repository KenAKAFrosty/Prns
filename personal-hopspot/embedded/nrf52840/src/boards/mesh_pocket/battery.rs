use personal_hopspot_core::{BatteryPercent, ExternalPowerState, PowerSnapshot};

const ABSENT_MILLIVOLTS: u32 = 3_000;
const PROFILE_POINTS: usize = 11;

#[cfg(feature = "mesh-pocket-battery-5000")]
const PROFILE_MILLIVOLTS: [u32; PROFILE_POINTS] = [
    4_300, 4_240, 4_120, 4_000, 3_888, 3_800, 3_740, 3_698, 3_655, 3_580, 3_400,
];
#[cfg(feature = "mesh-pocket-battery-10000")]
const PROFILE_MILLIVOLTS: [u32; PROFILE_POINTS] = [
    4_100, 4_060, 3_960, 3_840, 3_729, 3_625, 3_550, 3_500, 3_420, 3_345, 3_100,
];

pub(crate) struct MeshPocketBatteryGauge {
    ema_millivolts: u32,
}

pub(crate) const fn gauge() -> MeshPocketBatteryGauge {
    MeshPocketBatteryGauge { ema_millivolts: 0 }
}

impl MeshPocketBatteryGauge {
    pub(crate) fn update(
        &mut self,
        millivolts: Option<u32>,
        external_power: ExternalPowerState,
    ) -> PowerSnapshot {
        let battery = millivolts
            .filter(|millivolts| *millivolts >= ABSENT_MILLIVOLTS)
            .map(|millivolts| {
                self.ema_millivolts = if self.ema_millivolts == 0 {
                    millivolts
                } else {
                    (self.ema_millivolts * 7 + millivolts) / 8
                };
                BatteryPercent::saturating(profile_percent(self.ema_millivolts))
            });
        PowerSnapshot::new(battery, external_power)
    }
}

fn profile_percent(millivolts: u32) -> u8 {
    if millivolts >= PROFILE_MILLIVOLTS[0] {
        return 100;
    }
    for index in 0..PROFILE_POINTS - 1 {
        let high = PROFILE_MILLIVOLTS[index];
        let low = PROFILE_MILLIVOLTS[index + 1];
        if millivolts < low {
            continue;
        }
        let low_percent = ((PROFILE_POINTS - index - 2) * 10) as u32;
        let interpolated = low_percent + (millivolts - low) * 10 / (high - low);
        return interpolated as u8;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_points_map_to_exact_deciles() {
        for (index, millivolts) in PROFILE_MILLIVOLTS.into_iter().enumerate() {
            assert_eq!(profile_percent(millivolts), (100 - index * 10) as u8);
        }
    }

    #[test]
    fn profile_interpolates_and_clamps() {
        let high = PROFILE_MILLIVOLTS[4];
        let low = PROFILE_MILLIVOLTS[5];
        assert_eq!(profile_percent(high + 1), 60);
        assert_eq!(profile_percent((high + low) / 2), 55);
        assert_eq!(profile_percent(PROFILE_MILLIVOLTS[10] - 1), 0);
    }

    #[test]
    fn absent_reading_preserves_external_power() {
        let external_power = ExternalPowerState::Present {
            charging: personal_hopspot_core::ChargingState::Unknown,
        };
        let snapshot = gauge().update(Some(ABSENT_MILLIVOLTS - 1), external_power);
        assert_eq!(snapshot, PowerSnapshot::new(None, external_power));
    }
}
