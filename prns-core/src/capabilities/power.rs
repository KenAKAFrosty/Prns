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

/// The battery observation currently available to a host.
///
/// `Level` means the source is not reporting active charging; it does not by itself prove that
/// external power is disconnected. Sources without a charge-status signal report `Level` or
/// `Unknown`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryState {
    Level(BatteryPercent),
    Charging(BatteryPercent),
    Unknown,
}

/// A host's battery probe. Embedded implementations may read hardware while mobile and desktop
/// implementations may use operating-system power services. Smoothing and percentage mapping live
/// in the shared [`BatteryGauge`]. Sources with no battery return `None`.
pub trait BatterySource {
    /// Terminal voltage in millivolts; `None` when absent or no reading is available this tick.
    fn read_millivolts(&mut self) -> Option<u32>;

    /// Whether the source reports active charging. Defaults to `false` when no charge-status
    /// signal exists.
    fn is_charging(&mut self) -> bool {
        false
    }
}

/// Folds a [`BatterySource`]'s raw millivolts into a smoothed [`BatteryState`], keeping the
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
    /// charge ceiling, because a cell sits at 4.0–4.2 V for its whole charged life — mapping that to
    /// 100% so the gauge reads full when it looks full, rather than parking a bar short near the top.
    pub const fn lipo() -> Self {
        Self {
            ema_mv: 0,
            empty_mv: 3300,
            full_mv: 4100,
            absent_mv: 3000,
        }
    }

    /// Read the source and fold it into the running estimate. A `None` reading, or a voltage below
    /// `absent_mv`, yields [`BatteryState::Unknown`]; otherwise an 8-sample EMA maps onto 0..=100%.
    pub fn sample(&mut self, source: &mut impl BatterySource) -> BatteryState {
        let charging = source.is_charging();
        self.update(source.read_millivolts(), charging)
    }

    /// Fold a raw reading straight into the gauge: the lower-level entry for platforms whose
    /// probe is async (the nRF SAADC) and so cannot implement the sync [`BatterySource`] trait.
    /// Same `None`/EMA semantics as the trait path, rendered as `Charging` when `charging` is set.
    pub fn update(&mut self, millivolts: Option<u32>, charging: bool) -> BatteryState {
        let Some(mv) = millivolts else {
            return BatteryState::Unknown;
        };
        if mv < self.absent_mv {
            return BatteryState::Unknown;
        }
        self.ema_mv = if self.ema_mv == 0 {
            mv
        } else {
            (self.ema_mv * 7 + mv) / 8
        };
        let span = self.full_mv - self.empty_mv;
        let percent = BatteryPercent::saturating(
            (self.ema_mv.saturating_sub(self.empty_mv) * 100 / span).min(100) as u8,
        );
        if charging {
            BatteryState::Charging(percent)
        } else {
            BatteryState::Level(percent)
        }
    }
}

/// A [`BatterySource`] for hosts with no battery (or none wired up yet): always `None`, so the
/// gauge reports [`BatteryState::Unknown`].
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
}
