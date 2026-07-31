use prns_core::interfaces::lora::RadioProfile;

use super::channel_access::ChannelTiming;

const TARGET_QUANTUM_SLOTS: u64 = 42;
const RATE_SCALE: u64 = 1 << 16;
const MAXIMUM_AGE_MULTIPLIER: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AirtimeQuantum {
    us: u64,
}

impl AirtimeQuantum {
    pub(super) const fn for_profile(profile: RadioProfile) -> Self {
        let maximum_logical_packet_us = profile.time_on_air_us(255).saturating_mul(2);
        let minimum_quantum_us = ChannelTiming::for_profile(profile)
            .slot_ms()
            .saturating_mul(TARGET_QUANTUM_SLOTS)
            .saturating_mul(1_000);
        let packet_multiples = minimum_quantum_us
            .saturating_add(maximum_logical_packet_us.saturating_sub(1))
            .saturating_div(maximum_logical_packet_us);
        let packet_multiples = if packet_multiples == 0 {
            1
        } else {
            packet_multiples
        };
        Self {
            us: maximum_logical_packet_us.saturating_mul(packet_multiples),
        }
    }

    pub(super) const fn us(self) -> u64 {
        self.us
    }

    pub(super) const fn permits(self, used_us: u64, packet_us: u64) -> bool {
        used_us.saturating_add(packet_us) <= self.us
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ServiceAge {
    earned_us: u64,
    quantum: AirtimeQuantum,
}

impl ServiceAge {
    pub(super) const fn new(profile: RadioProfile) -> Self {
        Self {
            earned_us: 0,
            quantum: AirtimeQuantum::for_profile(profile),
        }
    }

    pub(super) const fn quantum(self) -> AirtimeQuantum {
        self.quantum
    }

    #[cfg(test)]
    pub(super) const fn earned_us(self) -> u64 {
        self.earned_us
    }

    pub(super) fn record_peer_airtime(&mut self, airtime_us: u64) {
        self.earned_us = self
            .earned_us
            .saturating_add(airtime_us)
            .min(self.quantum.us());
    }

    pub(super) fn seed_continuation(&mut self) {
        self.earned_us = self.quantum.us();
    }

    pub(super) fn consume(&mut self) {
        self.earned_us = 0;
    }

    pub(super) fn reset(&mut self, profile: RadioProfile) {
        *self = Self::new(profile);
    }

    pub(super) const fn backoff_rate(self) -> BackoffRate {
        let bonus = self
            .earned_us
            .saturating_mul(MAXIMUM_AGE_MULTIPLIER.saturating_mul(RATE_SCALE))
            .saturating_div(self.quantum.us());
        BackoffRate((RATE_SCALE.saturating_add(bonus)) as u32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BackoffRate(u32);

impl BackoffRate {
    #[cfg(test)]
    pub(super) const ONE: Self = Self(RATE_SCALE as u32);

    pub(super) fn scale_elapsed_ms(self, elapsed_ms: u64, remainder: &mut u16) -> u64 {
        let scaled = elapsed_ms
            .saturating_mul(u64::from(self.0))
            .saturating_add(u64::from(*remainder));
        *remainder = (scaled & (RATE_SCALE - 1)) as u16;
        scaled / RATE_SCALE
    }

    pub(super) fn time_to_progress_ms(self, progress_ms: u64, remainder: u16) -> u64 {
        let required = progress_ms
            .saturating_mul(RATE_SCALE)
            .saturating_sub(u64::from(remainder));
        required
            .saturating_add(u64::from(self.0).saturating_sub(1))
            .saturating_div(u64::from(self.0))
            .max(1)
    }

    #[cfg(test)]
    const fn q16(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::lora::{
        ModemPreset, DEFAULT_915_PROFILE, LORA_MAX_PAYLOAD, LORA_SINGLE_FRAME_MAX,
    };

    fn profile(preset: ModemPreset) -> RadioProfile {
        RadioProfile {
            modulation: preset.modulation(),
            ..DEFAULT_915_PROFILE
        }
    }

    #[test]
    fn standard_preset_quanta_are_packet_aligned_and_amortize_contention() {
        let cases = [
            (ModemPreset::ShortFast, 1_208_502),
            (ModemPreset::MediumFast, 1_272_604),
            (ModemPreset::LongFast, 4_228_188),
            (ModemPreset::LongSlow, 28_406_578),
        ];
        for (preset, expected_us) in cases {
            let profile = profile(preset);
            let quantum = AirtimeQuantum::for_profile(profile);
            let maximum = profile.time_on_air_us(255) * 2;
            assert_eq!(quantum.us(), expected_us);
            assert_eq!(quantum.us() % maximum, 0);
            assert!(quantum.us() >= ChannelTiming::for_profile(profile).slot_ms() * 42 * 1_000);
        }
    }

    #[test]
    fn a_quantum_strictly_rejects_every_packet_that_would_cross_it() {
        let quantum = AirtimeQuantum::for_profile(DEFAULT_915_PROFILE);
        assert!(quantum.permits(0, quantum.us()));
        assert!(!quantum.permits(0, quantum.us() + 1));
        assert!(!quantum.permits(1, quantum.us()));
    }

    #[test]
    fn service_age_accelerates_continuously_and_caps_at_three_times() {
        let mut age = ServiceAge::new(DEFAULT_915_PROFILE);
        let quantum = age.quantum().us();
        assert_eq!(age.backoff_rate(), BackoffRate::ONE);

        age.record_peer_airtime(quantum / 2);
        assert_eq!(age.backoff_rate().q16(), (2 * RATE_SCALE) as u32);

        age.record_peer_airtime(quantum * 4);
        assert_eq!(age.earned_us(), quantum);
        assert_eq!(age.backoff_rate().q16(), (3 * RATE_SCALE) as u32);
    }

    #[test]
    fn fixed_point_countdown_preserves_fractional_progress() {
        let rate = BackoffRate((RATE_SCALE + RATE_SCALE / 2) as u32);
        let mut remainder = 0;
        assert_eq!(rate.scale_elapsed_ms(1, &mut remainder), 1);
        assert_eq!(remainder, (RATE_SCALE / 2) as u16);
        assert_eq!(rate.scale_elapsed_ms(1, &mut remainder), 2);
        assert_eq!(remainder, 0);
        assert_eq!(rate.time_to_progress_ms(3, 0), 2);
        assert_eq!(rate.time_to_progress_ms(3, (RATE_SCALE / 2) as u16), 2);
    }

    fn logical_packet_airtime(profile: RadioProfile, len: usize) -> u64 {
        let packet = [0u8; LORA_MAX_PAYLOAD];
        super::super::packet_airtime(&packet[..len], &profile)
    }

    #[test]
    fn strict_quantum_packing_exercises_all_presets_and_packet_shapes() {
        let packet_lengths = [1, 100, 220, LORA_SINGLE_FRAME_MAX - 1, LORA_MAX_PAYLOAD];
        for preset in [
            ModemPreset::ShortFast,
            ModemPreset::MediumFast,
            ModemPreset::LongFast,
            ModemPreset::LongSlow,
        ] {
            let profile = profile(preset);
            let quantum = AirtimeQuantum::for_profile(profile);
            for packet_len in packet_lengths {
                let packet_us = logical_packet_airtime(profile, packet_len);
                let mut used_us = 0;
                let mut packets = 0;
                while quantum.permits(used_us, packet_us) {
                    used_us += packet_us;
                    packets += 1;
                }
                assert!(
                    packets > 0,
                    "{preset:?} must admit one {packet_len}-byte packet"
                );
                assert!(used_us <= quantum.us());
                assert!(!quantum.permits(used_us, packet_us));
            }
        }
    }

    #[test]
    fn maximum_split_packets_are_atomic_and_bound_every_opportunity() {
        for preset in [
            ModemPreset::ShortFast,
            ModemPreset::MediumFast,
            ModemPreset::LongFast,
            ModemPreset::LongSlow,
        ] {
            let profile = profile(preset);
            let quantum = AirtimeQuantum::for_profile(profile);
            let split_us = logical_packet_airtime(profile, LORA_MAX_PAYLOAD);
            assert_eq!(split_us, profile.time_on_air_us(255) * 2);
            assert!(quantum.permits(0, split_us));
            assert_eq!(quantum.us() % split_us, 0);
            assert!(!quantum.permits(quantum.us(), split_us));
        }
    }

    #[test]
    fn peer_airtime_is_cumulative_capped_and_resettable() {
        let mut age = ServiceAge::new(DEFAULT_915_PROFILE);
        let quantum = age.quantum().us();
        age.record_peer_airtime(quantum / 4);
        age.record_peer_airtime(quantum / 3);
        assert_eq!(age.earned_us(), quantum / 4 + quantum / 3);
        age.record_peer_airtime(quantum);
        assert_eq!(age.earned_us(), quantum);
        age.consume();
        assert_eq!(age.earned_us(), 0);

        let slow = profile(ModemPreset::LongSlow);
        age.reset(slow);
        assert_eq!(age.earned_us(), 0);
        assert_eq!(age.quantum(), AirtimeQuantum::for_profile(slow));
    }
}
