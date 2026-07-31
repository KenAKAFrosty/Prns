use prns_core::interfaces::lora::{Modulation, RadioProfile};

use super::airtime_quantum::BackoffRate;

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
const CONTENTION_BAND_SLOTS: u64 = 15;
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
enum BackoffSelection {
    Primary { band: u8 },
    TieBreak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelAccessState {
    NeedBackoff(BackoffSelection),
    WaitingForClear { clear_ms: u64, remaining_ms: u64 },
    Backoff { remaining_ms: u64 },
    FinalCheck,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentionPriority {
    Fresh { short_airtime_per_mille: u16 },
    Continuation,
}

impl ContentionPriority {
    const fn band(self) -> u8 {
        match self {
            Self::Fresh {
                short_airtime_per_mille,
            } => utilization_band(short_airtime_per_mille),
            Self::Continuation => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChannelTiming {
    slot_ms: u64,
    difs_ms: u64,
    sample_ms: u64,
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
        }
    }

    pub(crate) const fn slot_ms(self) -> u64 {
        self.slot_ms
    }

    pub(crate) const fn sample_ms(self) -> u64 {
        self.sample_ms
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
    rate_remainder: u16,
    deferrals: u32,
}

impl ChannelAccess {
    #[cfg(test)]
    pub(crate) fn new(profile: RadioProfile, now_ms: u64, packet_airtime_us: u64) -> Self {
        Self::new_at(
            profile,
            now_ms,
            now_ms,
            packet_airtime_us,
            ContentionPriority::Fresh {
                short_airtime_per_mille: 0,
            },
        )
    }

    pub(crate) fn new_at(
        profile: RadioProfile,
        now_ms: u64,
        enqueued_at_ms: u64,
        packet_airtime_us: u64,
        priority: ContentionPriority,
    ) -> Self {
        Self {
            timing: ChannelTiming::for_profile(profile),
            state: ChannelAccessState::NeedBackoff(BackoffSelection::Primary {
                band: priority.band(),
            }),
            last_observation_ms: now_ms,
            expires_at_ms: enqueued_at_ms.saturating_add(pending_ttl_ms(packet_airtime_us)),
            rate_remainder: 0,
            deferrals: 0,
        }
    }

    pub(crate) fn next_poll_ms(&self, rate: BackoffRate) -> u64 {
        let state_limit_ms = match self.state {
            ChannelAccessState::WaitingForClear {
                clear_ms,
                remaining_ms,
            } => {
                if clear_ms < self.timing.difs_ms {
                    self.timing.difs_ms - clear_ms
                } else {
                    rate.time_to_progress_ms(remaining_ms, self.rate_remainder)
                }
            }
            ChannelAccessState::Backoff { remaining_ms } => {
                rate.time_to_progress_ms(remaining_ms, self.rate_remainder)
            }
            ChannelAccessState::FinalCheck => 1,
            ChannelAccessState::NeedBackoff(_) | ChannelAccessState::Complete => {
                self.timing.sample_ms
            }
        };
        state_limit_ms.min(self.timing.sample_ms).max(1)
    }

    pub(crate) const fn deferrals(&self) -> u32 {
        self.deferrals
    }

    pub(crate) const fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }

    pub(crate) fn restart_contention(&mut self, now_ms: u64) {
        self.state = match self.state {
            ChannelAccessState::Backoff { remaining_ms }
            | ChannelAccessState::WaitingForClear { remaining_ms, .. } => {
                ChannelAccessState::WaitingForClear {
                    clear_ms: 0,
                    remaining_ms,
                }
            }
            ChannelAccessState::FinalCheck => ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms: 0,
            },
            state @ (ChannelAccessState::NeedBackoff(_) | ChannelAccessState::Complete) => state,
        };
        self.last_observation_ms = now_ms;
    }

    pub(crate) fn observe(
        &mut self,
        now_ms: u64,
        observation: ChannelObservation,
        rate: BackoffRate,
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
            ChannelAccessState::NeedBackoff(_) => ChannelAccessAction::NeedBackoffEntropy,
            ChannelAccessState::WaitingForClear {
                clear_ms,
                remaining_ms,
            } => {
                let total_clear_ms = clear_ms.saturating_add(elapsed_ms);
                if total_clear_ms < self.timing.difs_ms {
                    self.state = ChannelAccessState::WaitingForClear {
                        clear_ms: total_clear_ms,
                        remaining_ms,
                    };
                    ChannelAccessAction::Wait
                } else {
                    self.advance_backoff(
                        remaining_ms,
                        total_clear_ms.saturating_sub(self.timing.difs_ms),
                        rate,
                    )
                }
            }
            ChannelAccessState::Backoff { remaining_ms } => {
                self.advance_backoff(remaining_ms, elapsed_ms, rate)
            }
            ChannelAccessState::FinalCheck => ChannelAccessAction::ReadyForFinalCheck,
            ChannelAccessState::Complete => ChannelAccessAction::Expired,
        }
    }

    pub(crate) fn choose_backoff(&mut self, entropy: u16) -> bool {
        let ChannelAccessState::NeedBackoff(selection) = self.state else {
            return false;
        };
        let (start_ms, width_ms) = match selection {
            BackoffSelection::Primary { band } => (
                u64::from(band)
                    .saturating_mul(CONTENTION_BAND_SLOTS)
                    .saturating_mul(self.timing.slot_ms),
                CONTENTION_BAND_SLOTS.saturating_mul(self.timing.slot_ms),
            ),
            BackoffSelection::TieBreak => (0, self.timing.slot_ms),
        };
        let offset_ms = u64::from(entropy).saturating_mul(width_ms) >> 16;
        self.state = ChannelAccessState::WaitingForClear {
            clear_ms: 0,
            remaining_ms: start_ms.saturating_add(offset_ms),
        };
        self.rate_remainder = 0;
        true
    }

    pub(crate) fn after_entropy(&self) -> ChannelAccessAction {
        match self.state {
            ChannelAccessState::NeedBackoff(_) => ChannelAccessAction::NeedBackoffEntropy,
            ChannelAccessState::WaitingForClear { .. }
            | ChannelAccessState::Backoff { .. }
            | ChannelAccessState::FinalCheck => ChannelAccessAction::Wait,
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

    fn advance_backoff(
        &mut self,
        remaining_ms: u64,
        elapsed_ms: u64,
        rate: BackoffRate,
    ) -> ChannelAccessAction {
        let progress_ms = rate.scale_elapsed_ms(elapsed_ms, &mut self.rate_remainder);
        let remaining_ms = remaining_ms.saturating_sub(progress_ms);
        if remaining_ms == 0 {
            self.state = ChannelAccessState::FinalCheck;
            ChannelAccessAction::ReadyForFinalCheck
        } else {
            self.state = ChannelAccessState::Backoff { remaining_ms };
            ChannelAccessAction::Wait
        }
    }

    fn defer(&mut self) -> ChannelAccessAction {
        let redraws_tail = matches!(self.state, ChannelAccessState::FinalCheck);
        self.state = match self.state {
            ChannelAccessState::NeedBackoff(selection) => {
                ChannelAccessState::NeedBackoff(selection)
            }
            ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms,
            } => ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms,
            },
            ChannelAccessState::WaitingForClear { remaining_ms, .. }
            | ChannelAccessState::Backoff { remaining_ms } => {
                self.deferrals = self.deferrals.saturating_add(1);
                ChannelAccessState::WaitingForClear {
                    clear_ms: 0,
                    remaining_ms,
                }
            }
            ChannelAccessState::FinalCheck => {
                self.deferrals = self.deferrals.saturating_add(1);
                ChannelAccessState::NeedBackoff(BackoffSelection::TieBreak)
            }
            ChannelAccessState::Complete => ChannelAccessState::Complete,
        };
        if redraws_tail {
            self.rate_remainder = 0;
        }
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

    pub(crate) const fn next_poll_ms(&self, now_ms: u64, ordinary_poll_ms: u64) -> u64 {
        if self.active {
            let remaining_ms = self.expires_at_ms.saturating_sub(now_ms);
            if remaining_ms == 0 {
                1
            } else {
                remaining_ms
            }
        } else {
            ordinary_poll_ms
        }
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
    use crate::lora::airtime_quantum::ServiceAge;
    use prns_core::interfaces::lora::{
        CodingRate, Frequency, LoraBandwidth, ModemPreset, PreambleSymbols, Region,
        SpreadingFactor, TxPower, DEFAULT_915_PROFILE,
    };

    fn begin(access: &mut ChannelAccess, entropy: u16) {
        assert_eq!(
            access.observe(0, ChannelObservation::Clear, BackoffRate::ONE),
            ChannelAccessAction::NeedBackoffEntropy
        );
        assert!(access.choose_backoff(entropy));
        assert_eq!(access.after_entropy(), ChannelAccessAction::Wait);
    }

    #[test]
    fn timing_tracks_symbols_and_preserves_profile_bounds() {
        let normal = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        assert_eq!(normal.slot_ms(), 99);
        assert_eq!(normal.sample_ms(), 12);

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
    fn a_transmit_requires_real_difs_ticket_and_a_final_clear_check() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        begin(&mut access, 0);
        let difs_ms = access.timing.difs_ms;
        assert_eq!(
            access.observe(difs_ms - 1, ChannelObservation::Clear, BackoffRate::ONE),
            ChannelAccessAction::Wait
        );
        assert_eq!(
            access.observe(difs_ms, ChannelObservation::Clear, BackoffRate::ONE),
            ChannelAccessAction::ReadyForFinalCheck
        );
        assert_eq!(
            access.final_check(difs_ms, ChannelObservation::Clear),
            ChannelAccessAction::Transmit
        );
    }

    #[test]
    fn full_entropy_maps_to_millisecond_tickets_across_self_airtime_bands() {
        let timing = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        let width_ms = 15 * timing.slot_ms;

        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        begin(&mut access, u16::MAX / 2 + 1);
        assert_eq!(
            access.state,
            ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms: width_ms / 2,
            }
        );

        let mut loaded = ChannelAccess::new_at(
            DEFAULT_915_PROFILE,
            0,
            0,
            1_000_000,
            ContentionPriority::Fresh {
                short_airtime_per_mille: 1_000,
            },
        );
        begin(&mut loaded, u16::MAX);
        assert_eq!(
            loaded.state,
            ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms: 3 * width_ms + width_ms - 1,
            }
        );

        let mut continuation = ChannelAccess::new_at(
            DEFAULT_915_PROFILE,
            0,
            0,
            1_000_000,
            ContentionPriority::Continuation,
        );
        begin(&mut continuation, 0);
        assert_eq!(
            continuation.state,
            ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms: 0,
            }
        );
    }

    #[test]
    fn busy_channel_restarts_difs_and_permanently_freezes_ticket_progress() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        begin(&mut access, u16::MAX / 2 + 1);
        let difs_ms = access.timing.difs_ms;
        assert_eq!(
            access.observe(difs_ms + 100, ChannelObservation::Clear, BackoffRate::ONE),
            ChannelAccessAction::Wait
        );
        let ChannelAccessState::Backoff {
            remaining_ms: frozen_ms,
        } = access.state
        else {
            panic!("ticket should be counting down");
        };
        assert_eq!(
            access.observe(difs_ms + 112, ChannelObservation::Busy, BackoffRate::ONE),
            ChannelAccessAction::Wait
        );
        assert_eq!(access.deferrals(), 1);
        assert_eq!(
            access.state,
            ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms: frozen_ms,
            }
        );

        let resumed_at = difs_ms + 112 + difs_ms;
        assert_eq!(
            access.observe(resumed_at, ChannelObservation::Clear, BackoffRate::ONE),
            ChannelAccessAction::Wait
        );
        assert_eq!(
            access.state,
            ChannelAccessState::Backoff {
                remaining_ms: frozen_ms,
            },
            "DIFS consumes clear time but not the frozen ticket"
        );
    }

    #[test]
    fn a_busy_final_check_draws_only_a_one_slot_tie_break_tail() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        begin(&mut access, 0);
        let difs_ms = access.timing.difs_ms;
        assert_eq!(
            access.observe(difs_ms, ChannelObservation::Clear, BackoffRate::ONE),
            ChannelAccessAction::ReadyForFinalCheck
        );
        assert_eq!(
            access.final_check(difs_ms, ChannelObservation::Busy),
            ChannelAccessAction::Wait
        );
        assert_eq!(
            access.observe(difs_ms, ChannelObservation::Clear, BackoffRate::ONE),
            ChannelAccessAction::NeedBackoffEntropy
        );
        assert!(access.choose_backoff(u16::MAX));
        assert_eq!(
            access.state,
            ChannelAccessState::WaitingForClear {
                clear_ms: 0,
                remaining_ms: access.timing.slot_ms - 1,
            }
        );
    }

    #[test]
    fn earned_age_accelerates_only_ticket_time_and_caps_at_three_times() {
        let mut age = ServiceAge::new(DEFAULT_915_PROFILE);
        age.record_peer_airtime(age.quantum().us());
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        begin(&mut access, u16::MAX / 2 + 1);
        let initial_ticket_ms = 15 * access.timing.slot_ms / 2;
        let difs_ms = access.timing.difs_ms;
        assert_eq!(
            access.observe(difs_ms + 100, ChannelObservation::Clear, age.backoff_rate()),
            ChannelAccessAction::Wait
        );
        assert_eq!(
            access.state,
            ChannelAccessState::Backoff {
                remaining_ms: initial_ticket_ms - 300,
            }
        );
    }

    #[test]
    fn fractional_ticket_progress_survives_channel_interruptions() {
        let mut age = ServiceAge::new(DEFAULT_915_PROFILE);
        age.record_peer_airtime(age.quantum().us() / 4);
        let rate = age.backoff_rate();
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 1_000_000);
        begin(&mut access, u16::MAX / 2 + 1);
        let initial_ticket_ms = 15 * access.timing.slot_ms / 2;
        let difs_ms = access.timing.difs_ms;

        assert_eq!(
            access.observe(difs_ms + 1, ChannelObservation::Clear, rate),
            ChannelAccessAction::Wait
        );
        assert_eq!(access.rate_remainder, 1 << 15);
        assert_eq!(
            access.observe(difs_ms + 2, ChannelObservation::Busy, rate),
            ChannelAccessAction::Wait
        );
        assert_eq!(access.rate_remainder, 1 << 15);

        let resumed_at = difs_ms + 2 + difs_ms;
        assert_eq!(
            access.observe(resumed_at, ChannelObservation::Clear, rate),
            ChannelAccessAction::Wait
        );
        assert_eq!(
            access.observe(resumed_at + 1, ChannelObservation::Clear, rate),
            ChannelAccessAction::Wait
        );
        assert_eq!(
            access.state,
            ChannelAccessState::Backoff {
                remaining_ms: initial_ticket_ms - 3,
            }
        );
        assert_eq!(access.rate_remainder, 0);
    }

    #[test]
    fn contention_expires_instead_of_forcing_a_transmit() {
        let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 100_000);
        assert_eq!(
            access.observe(30_000, ChannelObservation::Busy, BackoffRate::ONE),
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
        assert_eq!(activity.next_poll_ms(1_000, 12), watchdog);
        assert_eq!(activity.observe(1_000 + watchdog - 1), (true, false));
        assert_eq!(activity.observe(1_000 + watchdog), (false, true));

        activity.preamble_detected(2_000, DEFAULT_915_PROFILE);
        activity.header_valid(2_100, DEFAULT_915_PROFILE);
        assert!(activity.next_poll_ms(2_100, 12) > 12);
        assert_eq!(activity.observe(2_100 + watchdog), (true, false));
        activity.frame_finished();
        assert_eq!(activity.next_poll_ms(2_100, 12), 12);
        assert_eq!(activity.observe(20_000), (false, false));
    }

    #[test]
    fn low_load_latency_stays_within_one_sense_interval_of_the_rnode_shape() {
        let timing = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        for slots in [0u16, 1, 7, 14] {
            let mut access = ChannelAccess::new(DEFAULT_915_PROFILE, 0, 250_000);
            let entropy = ((u32::from(slots) << 16) / 15) as u16;
            let mut now_ms = 0;
            let adaptive_ms = loop {
                let mut action =
                    access.observe(now_ms, ChannelObservation::Clear, BackoffRate::ONE);
                if matches!(action, ChannelAccessAction::NeedBackoffEntropy) {
                    assert!(access.choose_backoff(entropy));
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

    #[test]
    fn rnode_baseline_freezes_a_selected_wait_but_restarts_difs() {
        let timing = ChannelTiming::for_profile(DEFAULT_915_PROFILE);
        let mut remaining_ms = Some(7 * timing.slot_ms);
        remaining_ms = remaining_ms.map(|remaining| remaining - timing.sample_ms());
        let frozen = remaining_ms;

        let mut clear_ms = 0;
        assert_eq!(remaining_ms, frozen, "busy does not redraw cw_wait");
        clear_ms += timing.sample_ms();
        assert!(clear_ms < timing.difs_ms, "busy does restart DIFS");
        assert_eq!(remaining_ms, frozen);
    }
}
