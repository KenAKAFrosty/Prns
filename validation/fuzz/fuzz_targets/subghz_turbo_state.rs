#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::interfaces::subghz::frequency_hopping::{
    AntennaGainDeciDb, ConductedPowerDbm, MeasuredTwentyDbBandwidth, Us915PowerInputs,
};
use prns_core::interfaces::subghz::turbo::{
    decode_frame, AcquisitionBeacon, AcquisitionObservation, AcquisitionTracker, CapabilitySupport,
    ChannelAccess, ChannelAccessAction, ChannelAccessEvent, ContentionClass, ContentionPolicy,
    DatagramId, MaximumTransmitUncertainty, MonotonicMicros, ReassemblyLifetime, ScheduleMicros,
    TransmissionTimingBudget, TrustedScheduleClock, TrustedTimeSource, TurboHardwareSupport,
    TurboReassembler, TurboTransmitterInstanceId, Us915TurboConfiguration, Us915TurboTransmitter,
    UtcTimescale, TURBO_CHANNEL_COUNT, US915_TURBO_PHY,
};
use prns_core::interfaces::subghz::{
    ChannelAssessmentPolicy, ChannelNoiseFloorBank, ChannelSample,
};

fuzz_target!(|bytes: &[u8]| {
    let mut reassembler = TurboReassembler::new(ReassemblyLifetime::new(1_000_000).unwrap());
    for (index, chunk) in bytes.chunks(255).enumerate() {
        if let Ok(frame) = decode_frame(chunk) {
            let _ = reassembler.ingest(MonotonicMicros::new(index as u64), frame);
        }
        let _ = AcquisitionBeacon::decode(chunk);
    }

    let drift_ppm = bytes.first().copied().unwrap_or(1) as u32 + 1;
    let mut tracker = AcquisitionTracker::new(drift_ppm).unwrap();
    let mut noise =
        ChannelNoiseFloorBank::<TURBO_CHANNEL_COUNT, 8>::new(ChannelAssessmentPolicy::turbo())
            .unwrap();
    let mut monotonic_us = 0u64;
    for (index, byte) in bytes.iter().copied().enumerate().take(256) {
        monotonic_us = monotonic_us.saturating_add(u64::from(byte) + 1);
        let channel_index = index % TURBO_CHANNEL_COUNT;
        let sample = match byte % 3 {
            0 => ChannelSample::DemodulatorBusy,
            1 => ChannelSample::Rssi {
                dbm: -140 + i16::from(byte % 100),
            },
            _ => ChannelSample::Unavailable,
        };
        let _ = noise.observe(channel_index, monotonic_us, sample);
    }

    for chunk in bytes.chunks(8) {
        let mut padded = [0u8; 8];
        padded[..chunk.len()].copy_from_slice(chunk);
        let received_at = u64::from_le_bytes(padded);
        let cycle = prns_core::interfaces::subghz::turbo::supercycle_cycle_at(received_at);
        let beacon =
            AcquisitionBeacon::from_entropy(cycle, u16::from_le_bytes([padded[0], padded[1]]));
        let channel = prns_core::interfaces::subghz::turbo::channel_index_at(received_at);
        let _ = tracker.observe(AcquisitionObservation::from_beacon(
            MonotonicMicros::new(received_at),
            channel,
            beacon,
            US915_TURBO_PHY,
            u64::from(padded[2]) + 1,
        ));
    }

    let clock = TrustedScheduleClock::new(
        MonotonicMicros::new(0),
        ScheduleMicros::new(0),
        100,
        drift_ppm,
        TrustedTimeSource::AuthenticatedHost,
        UtcTimescale::PosixUnsmeared,
    )
    .unwrap();
    let timing = TransmissionTimingBudget::new(1_000, 500, 1_000, 250, 1_000).unwrap();
    let power = Us915PowerInputs::new(
        ConductedPowerDbm::new(22).unwrap(),
        AntennaGainDeciDb::new(0),
        ConductedPowerDbm::new(22).unwrap(),
    );
    let hardware_support = TurboHardwareSupport {
        bit_rate: CapabilitySupport::Supported,
        frequency_deviation: CapabilitySupport::Supported,
        receiver_bandwidth: CapabilitySupport::Supported,
        gaussian_filter: CapabilitySupport::Supported,
        modulation_index: CapabilitySupport::Supported,
        whitening: CapabilitySupport::Supported,
        variable_length_packets: CapabilitySupport::Supported,
        preamble: CapabilitySupport::Supported,
        sync_word: CapabilitySupport::Supported,
        crc: CapabilitySupport::Supported,
    };
    let mut transmitter = Us915TurboTransmitter::new(
        MonotonicMicros::new(0),
        clock,
        Us915TurboConfiguration::new(
            MeasuredTwentyDbBandwidth::new(400_000).unwrap(),
            timing,
            MaximumTransmitUncertainty::new(5_000).unwrap(),
            power,
            hardware_support,
            TurboTransmitterInstanceId::new(1).unwrap(),
        ),
    )
    .unwrap();

    for (index, chunk) in bytes.chunks(32).enumerate().take(64) {
        let now = 10_100_000u64.saturating_add(index as u64 * 400_000);
        let channel = prns_core::interfaces::subghz::turbo::channel_index_at(now);
        let mut access = ChannelAccess::begin(
            ContentionPolicy::turbo(),
            channel,
            ContentionClass::Fresh {
                recent_airtime_per_mille: u16::from(chunk.first().copied().unwrap_or(0)) * 4,
            },
            0,
            MonotonicMicros::new(now - 3_000),
        )
        .unwrap();
        let _ = access.observe(MonotonicMicros::new(now - 1_000), ChannelAccessEvent::Clear);
        let Ok(ChannelAccessAction::Granted(grant)) =
            access.observe(MonotonicMicros::new(now), ChannelAccessEvent::Clear)
        else {
            continue;
        };
        let payload_len = chunk.len().max(1);
        let Ok(prepared) = transmitter.prepare_after_final_clear(
            grant,
            MonotonicMicros::new(now),
            DatagramId::new([
                index as u8,
                chunk.first().copied().unwrap_or(0),
                chunk.len() as u8,
            ]),
            &chunk[..payload_len.min(chunk.len())],
        ) else {
            continue;
        };
        if chunk.first().copied().unwrap_or(0) & 1 == 0 {
            let _ = transmitter.abort_before_rf(prepared);
            continue;
        }
        let planned = prepared.keyed_airtime_us();
        let Ok(active) = transmitter.mark_rf_started(prepared, MonotonicMicros::new(now + 1))
        else {
            continue;
        };
        let actual = if chunk.first().copied().unwrap_or(0) & 2 == 0 {
            planned
        } else {
            planned.saturating_add(u64::from(chunk.get(1).copied().unwrap_or(0)))
        };
        let _ = transmitter.complete(
            active,
            MonotonicMicros::new(now.saturating_add(actual).saturating_add(1_000)),
            actual,
        );
    }
});
