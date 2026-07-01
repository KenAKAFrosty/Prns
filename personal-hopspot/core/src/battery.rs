use crate::screen::BatteryState;

/// A board's hardware battery probe. Each platform implements this so the shared [`BatteryGauge`]
/// can turn a raw reading into a [`BatteryState`] — the smoothing and percentage curve live in the
/// gauge, so a board only has to say how to read its cell. Boards with no battery return `None`.
pub trait BatterySource {
    /// The battery terminal voltage in millivolts, or `None` when no battery is present or no
    /// reading is available this tick.
    fn read_millivolts(&mut self) -> Option<u32>;

    /// Whether external power is present (plugged in / charging), which the gauge renders as
    /// [`BatteryState::Charging`]. Defaults to `false` for boards with no charge-status signal.
    fn is_charging(&mut self) -> bool {
        false
    }
}

/// Folds a [`BatterySource`]'s raw millivolts into a smoothed [`BatteryState`]. The empty/full span,
/// the absent-battery floor, and the running average are shared here so every board renders the same
/// curve; only [`BatterySource::read_millivolts`] is board-specific.
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
        Self::new(3300, 4100, 3000)
    }

    pub const fn new(empty_mv: u32, full_mv: u32, absent_mv: u32) -> Self {
        Self {
            ema_mv: 0,
            empty_mv,
            full_mv,
            absent_mv,
        }
    }

    /// Read the source and fold it into the running estimate. A `None` reading, or a voltage below
    /// `absent_mv`, yields [`BatteryState::Unknown`]; otherwise an 8-sample EMA maps onto 0..=100%.
    pub fn sample(&mut self, source: &mut impl BatterySource) -> BatteryState {
        let charging = source.is_charging();
        self.update(source.read_millivolts(), charging)
    }

    /// Fold a raw reading straight into the gauge — the lower-level entry for platforms whose probe
    /// is async (e.g. the nRF SAADC) and so cannot implement the sync [`BatterySource`] trait. A
    /// `None` reading, or a voltage below `absent_mv`, yields [`BatteryState::Unknown`]; otherwise an
    /// 8-sample EMA maps onto 0..=100%, rendered as `Charging` when `charging` is set.
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
        let pct = (self.ema_mv.saturating_sub(self.empty_mv) * 100 / span).min(100) as u8;
        if charging {
            BatteryState::Charging(pct)
        } else {
            BatteryState::Level(pct)
        }
    }
}

/// A [`BatterySource`] for boards with no battery (or none wired up yet): always `None`, so the
/// gauge reports [`BatteryState::Unknown`].
pub struct NoBattery;

impl BatterySource for NoBattery {
    fn read_millivolts(&mut self) -> Option<u32> {
        None
    }
}
