/// A clamped battery state-of-charge percentage.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BatteryPercent(u8);

impl BatteryPercent {
    /// Clamp an untrusted percentage to the inclusive 0–100 range.
    #[must_use]
    pub const fn saturating(percent: u8) -> Self {
        Self(if percent > 100 { 100 } else { percent })
    }

    /// Return the clamped percentage.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// What a host knows about a battery while external power is present.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargingState {
    /// The battery is actively accepting charge.
    Charging,
    /// External power is present, but the battery is not actively charging.
    Idle,
    /// The host cannot distinguish charging from idle while externally powered.
    Unknown,
}

/// What a host knows about an external power source.
///
/// Charging status is nested under `Present`, so an impossible "charging while unplugged" state
/// cannot be represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalPowerState {
    /// The host has evidence that no external power source is present.
    Absent,
    /// An external power source is present, with the host's charging observation.
    Present { charging: ChargingState },
    /// The host cannot determine whether external power is present.
    Unknown,
}

impl ExternalPowerState {
    /// Convert a direct VBUS/external-power presence signal into a truthful observation. Presence
    /// alone does not establish whether the battery is actively charging.
    #[must_use]
    pub const fn from_presence(present: bool) -> Self {
        if present {
            Self::Present {
                charging: ChargingState::Unknown,
            }
        } else {
            Self::Absent
        }
    }
}

/// One coherent power observation shared by embedded, mobile, and desktop hosts.
///
/// Battery level and external power are independent observations: a USB-powered device with no
/// cell can still report external power, and a source without a VBUS signal can report a measured
/// battery while leaving external power unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerSnapshot {
    battery: Option<BatteryPercent>,
    external_power: ExternalPowerState,
}

impl PowerSnapshot {
    /// A snapshot with neither a battery reading nor external-power evidence.
    pub const UNKNOWN: Self = Self::new(None, ExternalPowerState::Unknown);

    #[must_use]
    pub const fn new(battery: Option<BatteryPercent>, external_power: ExternalPowerState) -> Self {
        Self {
            battery,
            external_power,
        }
    }

    /// Return the measured battery level, when a cell reading was available.
    #[must_use]
    pub const fn battery(self) -> Option<BatteryPercent> {
        self.battery
    }

    /// Return the host's independent external-power observation.
    #[must_use]
    pub const fn external_power(self) -> ExternalPowerState {
        self.external_power
    }
}

/// A host's battery probe. Embedded implementations may read hardware while mobile and desktop
/// implementations may use operating-system power services. Smoothing and percentage mapping live
/// in the shared [`BatteryGauge`].
pub trait BatterySource {
    /// Terminal voltage in millivolts; `None` when absent or no reading is available this tick.
    fn read_millivolts(&mut self) -> Option<u32>;

    /// The source's independent knowledge of external power.
    fn external_power(&mut self) -> ExternalPowerState {
        ExternalPowerState::Unknown
    }
}

/// Folds a [`BatterySource`]'s raw millivolts into a smoothed [`PowerSnapshot`], keeping the
/// empty/full span, absent-battery floor, and running average consistent across hosts.
pub struct BatteryGauge {
    ema_mv: u32,
    empty_mv: u32,
    full_mv: u32,
    absent_mv: u32,
}

impl BatteryGauge {
    /// A single-cell LiPo / 18650 curve: 3.30 V reads as empty and 4.10 V as full, with anything
    /// below 3.00 V treated as no battery (USB-only power). The full point is 4.10 V, not the 4.20 V
    /// charge ceiling, because a cell sits at 4.0–4.2 V for its whole charged life, so mapping that
    /// range to 100% keeps the gauge from parking a bar short near the top.
    #[must_use]
    pub const fn lipo() -> Self {
        Self {
            ema_mv: 0,
            empty_mv: 3300,
            full_mv: 4100,
            absent_mv: 3000,
        }
    }

    /// Read the source and fold it into the running estimate. Voltage is sampled before external
    /// power so probes that infer charging from a voltage trend observe the current sample.
    pub fn sample(&mut self, source: &mut impl BatterySource) -> PowerSnapshot {
        let millivolts = source.read_millivolts();
        let external_power = source.external_power();
        self.update(millivolts, external_power)
    }

    /// Fold a raw reading straight into the gauge: the lower-level entry for platforms whose
    /// probe is async (such as nRF SAADC) and therefore cannot implement the synchronous
    /// [`BatterySource`] trait.
    pub fn update(
        &mut self,
        millivolts: Option<u32>,
        external_power: ExternalPowerState,
    ) -> PowerSnapshot {
        let battery = millivolts
            .filter(|millivolts| *millivolts >= self.absent_mv)
            .map(|millivolts| {
                self.ema_mv = if self.ema_mv == 0 {
                    millivolts
                } else {
                    (self.ema_mv * 7 + millivolts) / 8
                };
                let span = self.full_mv - self.empty_mv;
                BatteryPercent::saturating(
                    (self.ema_mv.saturating_sub(self.empty_mv) * 100 / span).min(100) as u8,
                )
            });
        PowerSnapshot::new(battery, external_power)
    }
}

/// A [`BatterySource`] for hosts with no battery (or none wired up yet).
pub struct NoBattery;

impl BatterySource for NoBattery {
    fn read_millivolts(&mut self) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_percent_saturates_at_full() {
        assert_eq!(BatteryPercent::saturating(99).get(), 99);
        assert_eq!(BatteryPercent::saturating(101).get(), 100);
    }

    #[test]
    fn external_power_survives_an_absent_battery_reading() {
        let snapshot = BatteryGauge::lipo().update(
            None,
            ExternalPowerState::Present {
                charging: ChargingState::Unknown,
            },
        );

        assert_eq!(snapshot.battery(), None);
        assert_eq!(
            snapshot.external_power(),
            ExternalPowerState::Present {
                charging: ChargingState::Unknown,
            }
        );
    }

    struct TrendProbe {
        read: bool,
    }

    impl BatterySource for TrendProbe {
        fn read_millivolts(&mut self) -> Option<u32> {
            self.read = true;
            Some(3_700)
        }

        fn external_power(&mut self) -> ExternalPowerState {
            assert!(self.read, "power evidence must inspect the current sample");
            ExternalPowerState::Unknown
        }
    }

    #[test]
    fn source_power_evidence_observes_the_current_voltage_sample() {
        let mut source = TrendProbe { read: false };
        let snapshot = BatteryGauge::lipo().sample(&mut source);

        assert_eq!(snapshot.battery(), Some(BatteryPercent::saturating(50)));
        assert_eq!(snapshot.external_power(), ExternalPowerState::Unknown);
    }
}
