#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::interfaces::subghz::regions::us915::frequency_hopping::{
    AntennaGainDeciDb, ConductedPowerDbm, MeasuredTwentyDbBandwidth, Us915PowerInputs,
};
use prns_core::interfaces::subghz::regions::us915::turbo::{
    decode_frame, AcquisitionBeacon, AcquisitionObservation, AcquisitionTracker, CapabilitySupport,
    AcquisitionOutcome, ChannelAccess, ChannelAccessAction, ChannelAccessEvent, ContentionClass,
    ContentionPolicy, DatagramId, MaximumTransmitUncertainty, MonotonicMicros, ReassemblyLifetime,
    ScheduleMicros, SupercycleCycle, TransmissionTimingBudget, TrustedScheduleClock,
    TrustedTimeSource, TurboGlobalSlot, TurboHardwareSupport, TurboRadioDwell, TurboReassembler,
    TurboTransmitterInstanceId, Us915TurboConfiguration, Us915TurboTransmitter, UtcTimescale,
    ValidatedAcquiredSchedule, TURBO_CHANNEL_COUNT, US915_TURBO_SPEC,
};
use prns_core::interfaces::subghz::{
    ChannelAssessmentPolicy, ChannelNoiseFloorBank, ChannelSample,
};

fn validated_acquired_schedule(
    maximum_drift_ppm: u32,
    maximum_uncertainty: MaximumTransmitUncertainty,
) -> ValidatedAcquiredSchedule {
    let mut tracker = AcquisitionTracker::new(maximum_drift_ppm, maximum_uncertainty).unwrap();
    for cycle_index in 0..US915_TURBO_SPEC.acquisition_observations() {
        let cycle = SupercycleCycle::new(cycle_index).unwrap();
        let global_slot = u64::from(cycle_index) * TURBO_CHANNEL_COUNT as u64;
        let slot = US915_TURBO_SPEC
            .slot_for_global_slot(TurboGlobalSlot::new(global_slot).unwrap());
        let beacon = AcquisitionBeacon::new(cycle, cycle_index).unwrap();
        let received_at = global_slot * US915_TURBO_SPEC.slot_us()
            + beacon.completes_at_slot_offset_us(US915_TURBO_SPEC.phy());
        let outcome = tracker
            .observe(AcquisitionObservation::from_beacon(
                MonotonicMicros::new(received_at),
                slot.channel_index().index(),
                beacon,
                US915_TURBO_SPEC.phy(),
                500,
            ))
            .unwrap();
        if let AcquisitionOutcome::Operational { schedule } = outcome {
            return schedule;
        }
    }
    unreachable!()
}

fuzz_target!(|bytes: &[u8]| {
    let mut reassembler = TurboReassembler::new(ReassemblyLifetime::new(1_000_000).unwrap());
    for (index, chunk) in bytes.chunks(255).enumerate() {
        if let Ok(frame) = decode_frame(chunk) {
            let _ = reassembler.ingest(MonotonicMicros::new(index as u64), frame);
        }
        let _ = AcquisitionBeacon::decode(chunk);
    }

    let drift_ppm = bytes.first().copied().unwrap_or(1) as u32 + 1;
    let maximum_uncertainty = MaximumTransmitUncertainty::new(5_000).unwrap();
    let mut tracker = AcquisitionTracker::new(drift_ppm, maximum_uncertainty).unwrap();
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
        let schedule_slot = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(received_at));
        let cycle = schedule_slot.cycle();
        let beacon =
            AcquisitionBeacon::from_entropy(cycle, u16::from_le_bytes([padded[0], padded[1]]));
        let channel = schedule_slot.channel_index().index();
        let outcome = tracker.observe(AcquisitionObservation::from_beacon(
            MonotonicMicros::new(received_at),
            channel,
            beacon,
            US915_TURBO_SPEC.phy(),
            u64::from(padded[2]) + 1,
        ));
        if let Ok(AcquisitionOutcome::Operational { schedule }) = outcome {
            let _ = schedule.window_at(MonotonicMicros::new(received_at));
        }
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
    for chunk in bytes.chunks(16) {
        let mut padded = [0u8; 16];
        padded[..chunk.len()].copy_from_slice(chunk);
        let begins_at = u64::from_le_bytes(padded[..8].try_into().unwrap());
        let duration_us = u64::from_le_bytes(padded[8..].try_into().unwrap());
        let ends_before = begins_at.saturating_add(duration_us);
        if let Ok(dwell) = TurboRadioDwell::new(
            MonotonicMicros::new(begins_at),
            MonotonicMicros::new(ends_before),
        ) {
            let _ = clock.authorize_receive(dwell);
        }
    }
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
    let configuration = Us915TurboConfiguration::new(
        MeasuredTwentyDbBandwidth::new(400_000).unwrap(),
        timing,
        maximum_uncertainty,
        power,
        hardware_support,
        TurboTransmitterInstanceId::new(1).unwrap(),
    );
    let mut transmitter = if bytes.first().copied().unwrap_or(0) & 4 == 0 {
        Us915TurboTransmitter::new(MonotonicMicros::new(0), clock, configuration).unwrap()
    } else {
        Us915TurboTransmitter::new_with_acquired_schedule(
            MonotonicMicros::new(0),
            validated_acquired_schedule(drift_ppm, maximum_uncertainty),
            configuration,
        )
        .unwrap()
    };

    for (index, chunk) in bytes.chunks(32).enumerate().take(64) {
        let now = 50_100_000u64.saturating_add(index as u64 * 400_000);
        let channel = US915_TURBO_SPEC
            .slot_at(ScheduleMicros::new(now))
            .channel_index()
            .index();
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
