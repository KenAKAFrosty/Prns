use prns_core::interfaces::lora::{Modulation, RadioProfile};

const NOISE_SAMPLE_COUNT: usize = 32;
const NOISE_PERCENTILE: usize = 20;
const INTERFERENCE_MARGIN_DB: i16 = 11;
const CCA_THRESHOLD_CEILING_DBM: i16 = -83;
const PERSISTENT_SOFT_BUSY_MS: u64 = 2_500;
const SYMBOLS_PER_SLOT: u64 = 12;
const NORMAL_MIN_SLOT_MS: u64 = 24;
const FAST_MIN_SLOT_MS: u64 = 6;
const MAX_SLOT_MS: u64 = 100;
const FAST_BITRATE_BPS: u32 = 30_000;
const DIFS_SLOTS: u64 = 2;
const POST_TX_YIELD_SLOTS: u64 = 3;
const CONTENTION_BAND_SLOTS: u8 = 15;
const CONTENTION_BANDS: u8 = 4;
const MIN_PENDING_TTL_MS: u64 = 30_000;
const MAX_PENDING_TTL_MS: u64 = 120_000;

const fn clamp_u64(value: u64, minimum: u64, maximum: u64) -> u64 {
    if value < minimum {
        minimum
    } else if value > maximum {
        maximum
    } else {
        value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelObservation {
    Clear,
    Busy,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelAccessAction {
    Wait,
    NeedBackoffEntropy,
    ReadyForFinalCheck,
    Transmit,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelAccessState {
    WaitingForClear { clear_ms: u64 },
    NeedBackoff { band: u8 },
    Backoff { remaining_ms: u64 },
    FinalCheck,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChannelTiming {
    slot_ms: u64,
    difs_ms: u64,
    sample_ms: u64,
    post_tx_yield_ms: u64,
}

impl ChannelTiming {
    pub(crate) const fn for_profile(profile: RadioProfile) -> Self {
        let Modulation::Lora {
            spreading_factor,
            bandwidth,
            ..
        } = profile.modulation;
        let symbol_us = (1u64 << spreading_factor as u8) * 1_000_000 / bandwidth.hz() as u64;
        let slot_unclamped_ms = symbol_us
            .saturating_mul(SYMBOLS_PER_SLOT)
            .saturating_add(999)
            / 1_000;
        let minimum_ms = if profile.nominal_bitrate_bps() > FAST_BITRATE_BPS {
            FAST_MIN_SLOT_MS
        } else {
            NORMAL_MIN_SLOT_MS
        };
        let slot_ms = clamp_u64(slot_unclamped_ms, minimum_ms, MAX_SLOT_MS);
        Self {
            slot_ms,
            difs_ms: slot_ms * DIFS_SLOTS,
            sample_ms: clamp_u64(slot_ms / 4, 3, 12),
            post_tx_yield_ms: slot_ms * POST_TX_YIELD_SLOTS,
        }
    }

    #[cfg(test)]
    const fn slot_ms(self) -> u64 {
        self.slot_ms
    }

    pub(crate) const fn sample_ms(self) -> u64 {
        self.sample_ms
    }

    pub(crate) const fn post_tx_yield_ms(self) -> u64 {
        self.post_tx_yield_ms
    }
}

pub(crate) const fn pending_ttl_ms(airtime_us: u64) -> u64 {
    clamp_u64(
        airtime_us
            .saturating_mul(4)
            .saturating_add(999)
            .saturating_div(1_000),
        MIN_PENDING_TTL_MS,
        MAX_PENDING_TTL_MS,
    )
}

pub(crate) struct ChannelAccess {
    timing: ChannelTiming,
    state: ChannelAccessState,
    last_observation_ms: u64,
    expires_at_ms: u64,
    interruption_band: u8,
    deferrals: u32,
}

impl ChannelAccess {
    #[cfg(test)]
    pub(crate) fn new(profile: RadioProfile, now_ms: u64, packet_airtime_us: u64) -> Self {
        Self::new_at(profile, now_ms, now_ms, packet_airtime_us)
    }

    pub(crate) fn new_at(
        profile: RadioProfile,
        now_ms: u64,
        enqueued_at_ms: u64,
        packet_airtime_us: u64,
    ) -> Self {
        Self {
            timing: ChannelTiming::for_profile(profile),
            state: ChannelAccessState::WaitingForClear { clear_ms: 0 },
            last_observation_ms: now_ms,
            expires_at_ms: enqueued_at_ms.saturating_add(pending_ttl_ms(packet_airtime_us)),
            interruption_band: 0,
            deferrals: 0,
        }
    }

    pub(crate) const fn timing(&self) -> ChannelTiming {
        self.timing
    }

    pub(crate) const fn deferrals(&self) -> u32 {
        self.deferrals
    }

    pub(crate) const fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }

    pub(crate) fn restart_contention(&mut self, now_ms: u64) {
        self.state = ChannelAccessState::WaitingForClear { clear_ms: 0 };
        self.last_observation_ms = now_ms;
        self.interruption_band = 0;
    }

    pub(crate) fn observe(
        &mut self,
        now_ms: u64,
        observation: ChannelObservation,
        short_airtime_per_mille: u16,
    ) -> ChannelAccessAction {
        if now_ms >= self.expires_at_ms {
            self.state = ChannelAccessState::Complete;
            return ChannelAccessAction::Expired;
        }
        let elapsed_ms = now_ms.saturating_sub(self.last_observation_ms);
        self.last_observation_ms = now_ms;

        if !matches!(observation, ChannelObservation::Clear) {
            return self.defer();
        }

        match self.state {
            ChannelAccessState::WaitingForClear { clear_ms } => {
                let clear_ms = clear_ms.saturating_add(elapsed_ms);
                if clear_ms < self.timing.difs_ms {
                    self.state = ChannelAccessState::WaitingForClear { clear_ms };
                    ChannelAccessAction::Wait
                } else {
                    let band =
                        utilization_band(short_airtime_per_mille).max(self.interruption_band);
                    self.state = ChannelAccessState::NeedBackoff { band };
                    ChannelAccessAction::NeedBackoffEntropy
                }
            }
            ChannelAccessState::NeedBackoff { .. } => ChannelAccessAction::NeedBackoffEntropy,
            ChannelAccessState::Backoff { remaining_ms } => {
                let remaining_ms = remaining_ms.saturating_sub(elapsed_ms);
                if remaining_ms == 0 {
                    self.state = ChannelAccessState::FinalCheck;
                    ChannelAccessAction::ReadyForFinalCheck
                } else {
                    self.state = ChannelAccessState::Backoff { remaining_ms };
                    ChannelAccessAction::Wait
                }
            }
            ChannelAccessState::FinalCheck => ChannelAccessAction::ReadyForFinalCheck,
            ChannelAccessState::Complete => ChannelAccessAction::Expired,
        }
    }

    /// Supply fresh, uniformly distributed entropy. `u16::MAX` is rejected so
    /// all 15 slot choices have the same number of source values.
    pub(crate) fn choose_backoff(&mut self, entropy: u16) -> bool {
        let ChannelAccessState::NeedBackoff { band } = self.state else {
            return false;
        };
        if entropy == u16::MAX {
            return false;
        }
        let slot = band
            .saturating_mul(CONTENTION_BAND_SLOTS)
            .saturating_add((entropy % u16::from(CONTENTION_BAND_SLOTS)) as u8);
        let remaining_ms = u64::from(slot).saturating_mul(self.timing.slot_ms);
        self.state = if remaining_ms == 0 {
            ChannelAccessState::FinalCheck
        } else {
            ChannelAccessState::Backoff { remaining_ms }
        };
        true
    }

    pub(crate) fn after_entropy(&self) -> ChannelAccessAction {
        match self.state {
            ChannelAccessState::FinalCheck => ChannelAccessAction::ReadyForFinalCheck,
            ChannelAccessState::Backoff { .. } => ChannelAccessAction::Wait,
            ChannelAccessState::NeedBackoff { .. } => ChannelAccessAction::NeedBackoffEntropy,
            ChannelAccessState::WaitingForClear { .. } => ChannelAccessAction::Wait,
            ChannelAccessState::Complete => ChannelAccessAction::Expired,
        }
    }

    pub(crate) fn final_check(
        &mut self,
        now_ms: u64,
        observation: ChannelObservation,
    ) -> ChannelAccessAction {
        if now_ms >= self.expires_at_ms {
            self.state = ChannelAccessState::Complete;
            return ChannelAccessAction::Expired;
        }
        if !matches!(self.state, ChannelAccessState::FinalCheck) {
            return ChannelAccessAction::Wait;
        }
        self.last_observation_ms = now_ms;
        if matches!(observation, ChannelObservation::Clear) {
            self.state = ChannelAccessState::Complete;
            ChannelAccessAction::Transmit
        } else {
            self.defer()
        }
    }

    fn defer(&mut self) -> ChannelAccessAction {
        if matches!(
            self.state,
            ChannelAccessState::WaitingForClear { clear_ms: 0 }
        ) {
            return ChannelAccessAction::Wait;
        }
        self.deferrals = self.deferrals.saturating_add(1);
        self.interruption_band =
            (self.interruption_band + 1).min(CONTENTION_BANDS.saturating_sub(1));
        self.state = ChannelAccessState::WaitingForClear { clear_ms: 0 };
        ChannelAccessAction::Wait
    }
}

const fn utilization_band(short_airtime_per_mille: u16) -> u8 {
    match short_airtime_per_mille {
        0..=70 => 0,
        71..=450 => 1,
        451..=840 => 2,
        _ => 3,
    }
}

pub(crate) struct NoiseFloor {
    samples: [i16; NOISE_SAMPLE_COUNT],
    len: usize,
    cursor: usize,
    soft_busy_since_ms: Option<u64>,
}

impl NoiseFloor {
    pub(crate) const fn new() -> Self {
        Self {
            samples: [0; NOISE_SAMPLE_COUNT],
            len: 0,
            cursor: 0,
            soft_busy_since_ms: None,
        }
    }

    pub(crate) fn observe(
        &mut self,
        now_ms: u64,
        rssi_dbm: i16,
        demodulator_busy: bool,
    ) -> ChannelObservation {
        if demodulator_busy {
            self.soft_busy_since_ms = None;
            return ChannelObservation::Busy;
        }
        if self.len < NOISE_SAMPLE_COUNT {
            self.push(rssi_dbm);
            return if self.len == NOISE_SAMPLE_COUNT {
                self.classify(rssi_dbm)
            } else {
                ChannelObservation::Unknown
            };
        }

        let threshold = self
            .cca_threshold_dbm()
            .unwrap_or(CCA_THRESHOLD_CEILING_DBM);
        if rssi_dbm < threshold {
            self.soft_busy_since_ms = None;
            self.push(rssi_dbm);
            return ChannelObservation::Clear;
        }

        if rssi_dbm < CCA_THRESHOLD_CEILING_DBM {
            let since = *self.soft_busy_since_ms.get_or_insert(now_ms);
            if now_ms.saturating_sub(since) >= PERSISTENT_SOFT_BUSY_MS {
                self.reset();
                self.push(rssi_dbm);
                return ChannelObservation::Unknown;
            }
        } else {
            self.soft_busy_since_ms = None;
        }
        ChannelObservation::Busy
    }

    pub(crate) fn fail_closed(&self) -> ChannelObservation {
        ChannelObservation::Unknown
    }

    pub(crate) const fn is_calibrated(&self) -> bool {
        self.len == NOISE_SAMPLE_COUNT
    }

    pub(crate) fn noise_floor_dbm(&self) -> Option<i16> {
        if self.len < NOISE_SAMPLE_COUNT {
            return None;
        }
        let mut ordered = self.samples;
        ordered.sort_unstable();
        let index = (NOISE_SAMPLE_COUNT - 1) * NOISE_PERCENTILE / 100;
        Some(ordered[index])
    }

    pub(crate) fn cca_threshold_dbm(&self) -> Option<i16> {
        self.noise_floor_dbm()
            .map(|floor| floor.saturating_add(INTERFERENCE_MARGIN_DB))
            .map(|threshold| threshold.min(CCA_THRESHOLD_CEILING_DBM))
    }

    fn classify(&self, rssi_dbm: i16) -> ChannelObservation {
        match self.cca_threshold_dbm() {
            Some(threshold) if rssi_dbm < threshold => ChannelObservation::Clear,
            Some(_) => ChannelObservation::Busy,
            None => ChannelObservation::Unknown,
        }
    }

    fn push(&mut self, rssi_dbm: i16) {
        self.samples[self.cursor] = rssi_dbm;
        self.cursor = (self.cursor + 1) % NOISE_SAMPLE_COUNT;
        self.len = (self.len + 1).min(NOISE_SAMPLE_COUNT);
    }

    fn reset(&mut self) {
        self.len = 0;
        self.cursor = 0;
        self.soft_busy_since_ms = None;
    }
}

pub(crate) struct DemodulatorActivity {
    active: bool,
    header_valid: bool,
    expires_at_ms: u64,
}

impl DemodulatorActivity {
    pub(crate) const fn new() -> Self {
        Self {
            active: false,
            header_valid: false,
            expires_at_ms: 0,
        }
    }

    pub(crate) fn preamble_detected(&mut self, now_ms: u64, profile: RadioProfile) {
        if self.active {
            return;
        }
        self.active = true;
        self.header_valid = false;
        self.expires_at_ms = now_ms.saturating_add(false_preamble_watchdog_ms(profile));
    }

    pub(crate) fn header_valid(&mut self, now_ms: u64, profile: RadioProfile) {
        self.active = true;
        self.header_valid = true;
        let maximum_frame_ms = profile
            .time_on_air_us(255)
            .saturating_add(999)
            .saturating_div(1_000);
        self.expires_at_ms = now_ms
            .saturating_add(maximum_frame_ms)
            .saturating_add(1_000);
    }

    pub(crate) fn frame_finished(&mut self) {
        self.active = false;
        self.header_valid = false;
        self.expires_at_ms = 0;
    }

    /// Returns `(busy, false_preamble_expired)`.
    pub(crate) fn observe(&mut self, now_ms: u64) -> (bool, bool) {
        if !self.active {
            return (false, false);
        }
        if now_ms < self.expires_at_ms {
            return (true, false);
        }
        let false_preamble = !self.header_valid;
        self.frame_finished();
        (false, false_preamble)
    }
}

const fn false_preamble_watchdog_ms(profile: RadioProfile) -> u64 {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        ..
    } = profile.modulation;
    let symbol_us = (1u64 << spreading_factor as u8) * 1_000_000 / bandwidth.hz() as u64;
    let watchdog_symbols = (profile.preamble.count() as u64).saturating_add(20);
    symbol_us
        .saturating_mul(watchdog_symbols)
        .saturating_add(999)
        .saturating_div(1_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::lora::{
        CodingRate, Frequency, LoraBandwidth, ModemPreset, PreambleSymbols, Region,
        SpreadingFactor, TxPower, DEFAULT_915_PROFILE,
    };

    fn observe_clear_for(
        access: &mut ChannelAccess,
        start_ms: u64,
        duration_ms: u64,
        utilization: u16,
    ) -> ChannelAccessAction {
        let sample_ms = access.timing().sample_ms();
        let mut action = ChannelAccessAction::Wait;
        let mut now = start_ms;
        while now <= start_ms + duration_ms {
            action = access.observe(now, ChannelObservation::Clear, utilization);
            if matches!(action, ChannelAccessAction::NeedBackoffEntropy) {
                break;
            }
            now += sample_ms;
        }
        action
    }

    #[test]
    fn timing_tracks_symbols_and_preserves_rnode_bounds() {
        let normal = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        assert_eq!(normal.slot_ms(), 99);
        assert_eq!(normal.sample_ms(), 12);
        assert_eq!(normal.post_tx_yield_ms(), 297);

        let fastest = RadioProfile {
            frequency: Frequency::new(915_000_000),
            modulation: Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf5,
                bandwidth: LoraBandwidth::Bw500kHz,
                coding_rate: CodingRate::Cr45,
            },
            tx_power: TxPower::new(14),
            preamble: PreambleSymbols::new(18),
            region: Region::Us915,
        };
        assert_eq!(ChannelTiming::for_profile(fastest).slot_ms(), 6);

        let slowest = RadioProfile {
            modulation: ModemPreset::LongSlow.modulation(),
            ..DEFAULT_915_PROFILE
        };
        assert_eq!(ChannelTiming::for_profile(slowest).slot_ms(), 100);
    }

    #[test]
    fn a_transmit_requires_difs_backoff_and_a_final_clear_check() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        assert_eq!(
            observe_clear_for(&mut access, 0, 500, 0),
            ChannelAccessAction::NeedBackoffEntropy
        );
        assert!(access.choose_backoff(1));
        let after_backoff = access.observe(400, ChannelObservation::Clear, 0);
        assert_eq!(after_backoff, ChannelAccessAction::ReadyForFinalCheck);
        assert_eq!(
            access.final_check(400, ChannelObservation::Clear),
            ChannelAccessAction::Transmit
        );
    }

    #[test]
    fn busy_channel_freezes_backoff_and_escalates_the_next_window() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        assert_eq!(
            observe_clear_for(&mut access, 0, 500, 0),
            ChannelAccessAction::NeedBackoffEntropy
        );
        assert!(access.choose_backoff(14));
        assert_eq!(
            access.observe(400, ChannelObservation::Busy, 0),
            ChannelAccessAction::Wait
        );
        assert_eq!(access.deferrals(), 1);

        assert_eq!(
            observe_clear_for(&mut access, 401, 500, 0),
            ChannelAccessAction::NeedBackoffEntropy
        );
        assert!(access.choose_backoff(0));
        assert_eq!(
            access.state,
            ChannelAccessState::Backoff {
                remaining_ms: 15 * 99,
            },
            "one interruption raises an otherwise idle channel into the second window"
        );
    }

    #[test]
    fn entropy_rejection_is_unbiased_and_non_advancing() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        assert_eq!(
            observe_clear_for(&mut access, 0, 500, 0),
            ChannelAccessAction::NeedBackoffEntropy
        );
        assert!(!access.choose_backoff(u16::MAX));
        assert_eq!(
            access.after_entropy(),
            ChannelAccessAction::NeedBackoffEntropy
        );
        assert!(access.choose_backoff(u16::MAX - 1));
    }

    #[test]
    fn contention_expires_instead_of_forcing_a_transmit() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 100_000);
        assert_eq!(
            access.observe(30_000, ChannelObservation::Busy, 0),
            ChannelAccessAction::Expired
        );
    }

    #[test]
    fn packet_ttl_scales_but_stays_bounded() {
        assert_eq!(pending_ttl_ms(100_000), 30_000);
        assert_eq!(pending_ttl_ms(10_000_000), 40_000);
        assert_eq!(pending_ttl_ms(90_000_000), 120_000);
    }

    #[test]
    fn noise_floor_uses_a_lower_percentile_and_caps_the_threshold() {
        let mut floor = NoiseFloor::new();
        for index in 0..NOISE_SAMPLE_COUNT {
            let sample = if index < 7 { -121 } else { -112 };
            let _ = floor.observe(index as u64, sample, false);
        }
        assert_eq!(floor.noise_floor_dbm(), Some(-121));
        assert_eq!(floor.cca_threshold_dbm(), Some(-110));
        assert_eq!(floor.observe(100, -109, false), ChannelObservation::Busy);

        let mut loud_floor = NoiseFloor::new();
        for index in 0..NOISE_SAMPLE_COUNT {
            let _ = loud_floor.observe(index as u64, -80, false);
        }
        assert_eq!(loud_floor.cca_threshold_dbm(), Some(-83));
    }

    #[test]
    fn calibration_and_sensing_fail_closed() {
        let mut floor = NoiseFloor::new();
        assert_eq!(floor.observe(0, -120, false), ChannelObservation::Unknown);
        assert_eq!(floor.fail_closed(), ChannelObservation::Unknown);
        assert_eq!(floor.observe(1, -120, true), ChannelObservation::Busy);
    }

    #[test]
    fn persistent_soft_busy_restarts_calibration_without_learning_a_jammer() {
        let mut floor = NoiseFloor::new();
        for index in 0..NOISE_SAMPLE_COUNT {
            let _ = floor.observe(index as u64, -120, false);
        }
        assert_eq!(floor.observe(100, -100, false), ChannelObservation::Busy);
        assert_eq!(
            floor.observe(2_600, -100, false),
            ChannelObservation::Unknown
        );
        assert_eq!(floor.noise_floor_dbm(), None);

        let mut floor = NoiseFloor::new();
        for index in 0..NOISE_SAMPLE_COUNT {
            let _ = floor.observe(index as u64, -120, false);
        }
        assert_eq!(floor.observe(100, -70, false), ChannelObservation::Busy);
        assert_eq!(floor.observe(10_000, -70, false), ChannelObservation::Busy);
        assert_eq!(floor.noise_floor_dbm(), Some(-120));
    }

    #[test]
    fn preamble_activity_latches_until_header_or_the_false_preamble_watchdog() {
        let mut activity = DemodulatorActivity::new();
        activity.preamble_detected(1_000, DEFAULT_915_PROFILE);
        let watchdog = false_preamble_watchdog_ms(DEFAULT_915_PROFILE);
        assert_eq!(activity.observe(1_000 + watchdog - 1), (true, false));
        assert_eq!(activity.observe(1_000 + watchdog), (false, true));

        activity.preamble_detected(2_000, DEFAULT_915_PROFILE);
        activity.header_valid(2_100, DEFAULT_915_PROFILE);
        assert_eq!(activity.observe(2_100 + watchdog), (true, false));
        activity.frame_finished();
        assert_eq!(activity.observe(20_000), (false, false));
    }

    #[derive(Clone, Copy)]
    struct SimulationResult {
        collisions: u64,
        successes: [u64; 8],
    }

    fn next_entropy(state: &mut u32) -> u16 {
        let mut value = *state;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        *state = value;
        value as u16
    }

    fn simulate_adaptive(node_count: usize, target_successes: u64) -> SimulationResult {
        let timing = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        let sample_ms = timing.sample_ms();
        let frame_airtime_ms = 250u64;
        let mut access: [Option<ChannelAccess>; 8] = core::array::from_fn(|_| None);
        let mut entropy: [u32; 8] = core::array::from_fn(|index| 0x9e37_79b9 ^ (index as u32 + 1));
        let mut eligible_at_ms = [0u64; 8];
        let mut successes = [0u64; 8];
        let mut successful_tx_at_ms = [[0u64; 1_024]; 8];
        let mut collisions = 0u64;
        let mut busy_until_ms = 0u64;
        let mut now_ms = 0u64;

        while successes.iter().sum::<u64>() < target_successes {
            assert!(now_ms < 20_000_000, "adaptive simulation made no progress");
            let channel_busy = now_ms < busy_until_ms;
            let mut candidates = [false; 8];
            for node in 0..node_count {
                if access[node].is_none() && now_ms >= eligible_at_ms[node] {
                    access[node] = Some(ChannelAccess::new(
                        DEFAULT_915_PROFILE,
                        now_ms,
                        frame_airtime_ms * 1_000,
                    ));
                }
                let Some(contender) = access[node].as_mut() else {
                    continue;
                };
                let live_transmissions = successful_tx_at_ms[node][..successes[node] as usize]
                    .iter()
                    .filter(|sent_at| now_ms.saturating_sub(**sent_at) < 15_000)
                    .count() as u64;
                let short_airtime_per_mille = live_transmissions
                    .saturating_mul(frame_airtime_ms)
                    .saturating_mul(1_000)
                    .saturating_div(15_000)
                    .min(1_000) as u16;
                let mut action = contender.observe(
                    now_ms,
                    if channel_busy {
                        ChannelObservation::Busy
                    } else {
                        ChannelObservation::Clear
                    },
                    short_airtime_per_mille,
                );
                while matches!(action, ChannelAccessAction::NeedBackoffEntropy) {
                    if contender.choose_backoff(next_entropy(&mut entropy[node])) {
                        action = contender.after_entropy();
                    }
                }
                if matches!(action, ChannelAccessAction::ReadyForFinalCheck) {
                    action = contender.final_check(
                        now_ms,
                        if channel_busy {
                            ChannelObservation::Busy
                        } else {
                            ChannelObservation::Clear
                        },
                    );
                }
                candidates[node] = matches!(action, ChannelAccessAction::Transmit);
                if matches!(action, ChannelAccessAction::Expired) {
                    access[node] = None;
                    eligible_at_ms[node] = now_ms.saturating_add(timing.post_tx_yield_ms());
                }
            }

            let candidate_count = candidates[..node_count]
                .iter()
                .filter(|candidate| **candidate)
                .count();
            if candidate_count > 0 {
                busy_until_ms = now_ms.saturating_add(frame_airtime_ms);
                if candidate_count == 1 {
                    let winner = candidates[..node_count]
                        .iter()
                        .position(|candidate| *candidate)
                        .expect("one candidate has a position");
                    successful_tx_at_ms[winner][successes[winner] as usize] = now_ms;
                    successes[winner] += 1;
                } else {
                    collisions += 1;
                }
                for node in 0..node_count {
                    if candidates[node] {
                        access[node] = None;
                        eligible_at_ms[node] =
                            busy_until_ms.saturating_add(timing.post_tx_yield_ms());
                    }
                }
            }
            now_ms = now_ms.saturating_add(sample_ms);
        }
        SimulationResult {
            collisions,
            successes,
        }
    }

    fn simulate_rnode_baseline(node_count: usize, target_successes: u64) -> SimulationResult {
        let timing = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        let sample_ms = timing.sample_ms();
        let frame_airtime_ms = 250u64;
        let mut clear_ms = [0u64; 8];
        let mut backoff_ms = [None; 8];
        let mut entropy: [u32; 8] = core::array::from_fn(|index| 0x9e37_79b9 ^ (index as u32 + 1));
        let mut eligible_at_ms = [0u64; 8];
        let mut successes = [0u64; 8];
        let mut collisions = 0u64;
        let mut busy_until_ms = 0u64;
        let mut now_ms = 0u64;

        while successes.iter().sum::<u64>() < target_successes {
            assert!(now_ms < 20_000_000, "baseline simulation made no progress");
            let channel_busy = now_ms < busy_until_ms;
            let mut candidates = [false; 8];
            for node in 0..node_count {
                if now_ms < eligible_at_ms[node] {
                    continue;
                }
                if channel_busy {
                    clear_ms[node] = 0;
                    backoff_ms[node] = None;
                    continue;
                }
                if clear_ms[node] < timing.difs_ms {
                    clear_ms[node] = clear_ms[node].saturating_add(sample_ms);
                    continue;
                }
                match backoff_ms[node] {
                    None => {
                        let slots = u64::from(next_entropy(&mut entropy[node]) % 15);
                        let remaining = slots.saturating_mul(timing.slot_ms);
                        if remaining == 0 {
                            candidates[node] = true;
                        } else {
                            backoff_ms[node] = Some(remaining);
                        }
                    }
                    Some(remaining) => {
                        let remaining = remaining.saturating_sub(sample_ms);
                        if remaining == 0 {
                            candidates[node] = true;
                        } else {
                            backoff_ms[node] = Some(remaining);
                        }
                    }
                }
            }

            let candidate_count = candidates[..node_count]
                .iter()
                .filter(|candidate| **candidate)
                .count();
            if candidate_count > 0 {
                busy_until_ms = now_ms.saturating_add(frame_airtime_ms);
                if candidate_count == 1 {
                    let winner = candidates[..node_count]
                        .iter()
                        .position(|candidate| *candidate)
                        .expect("one candidate has a position");
                    successes[winner] += 1;
                } else {
                    collisions += 1;
                }
                for node in 0..node_count {
                    if candidates[node] {
                        clear_ms[node] = 0;
                        backoff_ms[node] = None;
                        eligible_at_ms[node] =
                            busy_until_ms.saturating_add(timing.post_tx_yield_ms());
                    }
                }
            }
            now_ms = now_ms.saturating_add(sample_ms);
        }
        SimulationResult {
            collisions,
            successes,
        }
    }

    fn fairness_meets_ninety_percent(successes: &[u64]) -> bool {
        let sum = successes.iter().sum::<u64>() as u128;
        let squares = successes
            .iter()
            .map(|successes| u128::from(*successes).pow(2))
            .sum::<u128>();
        10 * sum.pow(2) >= 9 * successes.len() as u128 * squares
    }

    #[test]
    fn adaptive_contention_is_fair_and_no_more_collision_prone_than_the_rnode_window() {
        for node_count in [2, 4, 8] {
            let target_successes = node_count as u64 * 300;
            let adaptive = simulate_adaptive(node_count, target_successes);
            let baseline = simulate_rnode_baseline(node_count, target_successes);
            assert!(
                fairness_meets_ninety_percent(&adaptive.successes[..node_count]),
                "{node_count} nodes: successes {:?}",
                &adaptive.successes[..node_count]
            );
            assert!(
                adaptive.collisions <= baseline.collisions,
                "{node_count} nodes: adaptive collisions {}, baseline {}",
                adaptive.collisions,
                baseline.collisions
            );
        }
    }

    #[test]
    fn low_load_latency_stays_within_one_sense_interval_of_the_rnode_shape() {
        let timing = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        for slots in [0u16, 1, 7, 14] {
            let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 250_000);
            let mut now_ms = 0;
            let adaptive_ms = loop {
                let mut action = access.observe(now_ms, ChannelObservation::Clear, 0);
                if matches!(action, ChannelAccessAction::NeedBackoffEntropy) {
                    assert!(access.choose_backoff(slots));
                    action = access.after_entropy();
                }
                if matches!(action, ChannelAccessAction::ReadyForFinalCheck)
                    && matches!(
                        access.final_check(now_ms, ChannelObservation::Clear),
                        ChannelAccessAction::Transmit
                    )
                {
                    break now_ms;
                }
                now_ms += timing.sample_ms();
            };
            let difs_samples =
                timing.difs_ms.saturating_add(timing.sample_ms() - 1) / timing.sample_ms();
            let backoff_ms = u64::from(slots).saturating_mul(timing.slot_ms);
            let backoff_samples =
                backoff_ms.saturating_add(timing.sample_ms() - 1) / timing.sample_ms();
            let baseline_ms = (difs_samples + backoff_samples) * timing.sample_ms();
            assert!(
                adaptive_ms <= baseline_ms.saturating_add(timing.sample_ms()),
                "{slots} slots: adaptive {adaptive_ms}ms, baseline {baseline_ms}ms"
            );
        }
    }
}
