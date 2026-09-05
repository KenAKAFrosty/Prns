use super::*;
use crate::interfaces::subghz::regions::us915::frequency_hopping::{
    AntennaGainDeciDb, ConductedPowerDbm, MeasuredTwentyDbBandwidth, Us915PowerInputs,
};
use crate::interfaces::subghz::MonotonicMicros;
use proptest::prelude::*;

fn trusted_clock() -> TrustedScheduleClock {
    TrustedScheduleClock::new(
        MonotonicMicros::new(0),
        ScheduleMicros::new(0),
        100,
        1,
        TrustedTimeSource::Gnss,
        UtcTimescale::PosixUnsmeared,
    )
    .unwrap()
}

fn timing() -> TransmissionTimingBudget {
    TransmissionTimingBudget::new(1_000, 500, 1_000, 250, 1_000).unwrap()
}

fn scheduled_channel(schedule_us: u64) -> usize {
    US915_TURBO_SPEC
        .slot_at(ScheduleMicros::new(schedule_us))
        .channel_index()
        .index()
}

fn global_slot_channel(global_slot: u64) -> usize {
    US915_TURBO_SPEC
        .slot_for_global_slot(TurboGlobalSlot::new(global_slot).unwrap())
        .channel_index()
        .index()
}

fn hardware_support() -> TurboHardwareSupport {
    TurboHardwareSupport {
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
    }
}

fn configuration(
    instance_id: u64,
    hardware_support: TurboHardwareSupport,
) -> Us915TurboConfiguration {
    let power = Us915PowerInputs::new(
        ConductedPowerDbm::new(28).unwrap(),
        AntennaGainDeciDb::new(0),
        ConductedPowerDbm::new(28).unwrap(),
    );
    Us915TurboConfiguration::new(
        MeasuredTwentyDbBandwidth::new(400_000).unwrap(),
        timing(),
        MaximumTransmitUncertainty::new(1_000).unwrap(),
        power,
        hardware_support,
        TurboTransmitterInstanceId::new(instance_id).unwrap(),
    )
}

fn transmitter() -> Us915TurboTransmitter {
    Us915TurboTransmitter::new(
        MonotonicMicros::new(0),
        trusted_clock(),
        configuration(1, hardware_support()),
    )
    .unwrap()
}

fn final_clear_grant(channel_index: usize, issued_at: u64) -> FinalClearGrant {
    let mut access = ChannelAccess::begin(
        ContentionPolicy::turbo(),
        channel_index,
        ContentionClass::Fresh {
            recent_airtime_per_mille: 0,
        },
        0,
        MonotonicMicros::new(issued_at - 3_000),
    )
    .unwrap();
    assert_eq!(
        access
            .observe(
                MonotonicMicros::new(issued_at - 1_000),
                ChannelAccessEvent::Clear
            )
            .unwrap(),
        ChannelAccessAction::PerformFinalClear
    );
    match access
        .observe(MonotonicMicros::new(issued_at), ChannelAccessEvent::Clear)
        .unwrap()
    {
        ChannelAccessAction::Granted(grant) => grant,
        outcome => panic!("expected final-clear grant, got {outcome:?}"),
    }
}

fn acquisition_observation(cycle_index: u8, timing_uncertainty_us: u64) -> AcquisitionObservation {
    acquisition_observation_with_offset(cycle_index, timing_uncertainty_us, 0)
}

fn acquisition_observation_with_offset(
    cycle_index: u8,
    timing_uncertainty_us: u64,
    monotonic_offset_us: u64,
) -> AcquisitionObservation {
    let cycle = SupercycleCycle::new(cycle_index).unwrap();
    let beacon = AcquisitionBeacon::new(cycle, 0).unwrap();
    let global_slot = u64::from(cycle_index) * TURBO_CHANNEL_COUNT as u64;
    let received_at = global_slot * US915_TURBO_SPEC.slot_us()
        + beacon.completes_at_slot_offset_us(US915_TURBO_SPEC.phy())
        + monotonic_offset_us;
    AcquisitionObservation::from_beacon(
        MonotonicMicros::new(received_at),
        global_slot_channel(global_slot),
        beacon,
        US915_TURBO_SPEC.phy(),
        timing_uncertainty_us,
    )
}

fn validated_acquired_schedule() -> ValidatedAcquiredSchedule {
    let maximum_uncertainty = MaximumTransmitUncertainty::new(1_000).unwrap();
    let mut tracker = AcquisitionTracker::new(40, maximum_uncertainty).unwrap();
    assert!(matches!(
        tracker.observe(acquisition_observation(0, 500)).unwrap(),
        AcquisitionOutcome::Candidate { .. }
    ));
    assert!(matches!(
        tracker.observe(acquisition_observation(1, 500)).unwrap(),
        AcquisitionOutcome::Candidate { .. }
    ));
    match tracker.observe(acquisition_observation(2, 500)).unwrap() {
        AcquisitionOutcome::Operational { schedule } => schedule,
        outcome => panic!("expected operational acquisition, got {outcome:?}"),
    }
}

fn acquisition_simulation(environment: AcquisitionEnvironment) -> AcquisitionSimulation {
    AcquisitionSimulation {
        seed: 0x5052_4e53_4143_5155,
        trials: 256,
        scanner_dwell_us: US915_TURBO_SPEC.scan_dwell_us(),
        beacon_opportunity_per_mille: 1_000,
        packet_loss_per_mille: 0,
        maximum_search_us: 1_800_000_000,
        maximum_clock_drift_ppm: 40,
        actual_clock_drift_ppm: 20,
        observation_timing_uncertainty_us: 500,
        timestamp_error_span_us: 250,
        maximum_transmit_uncertainty: MaximumTransmitUncertainty::new(5_000).unwrap(),
        environment,
    }
}

#[test]
fn us915_turbo_profile_fully_names_the_phy() {
    let profile = US915_TURBO_SPEC.phy();
    assert_eq!(profile.validate(), Ok(()));
    assert_eq!(profile.bit_rate().bps(), 250_000);
    assert_eq!(profile.frequency_deviation().hz(), 62_500);
    assert_eq!(profile.receiver_bandwidth().hz(), 467_000);
    assert_eq!(profile.modulation_index(), ModulationIndex::Half);
    assert_eq!(profile.sync_word(), *b"PRNS");
    assert_eq!(
        profile.data_whitening(),
        DataWhitening::Pn9 {
            polynomial: 0x021,
            seed: 0x1ff,
        }
    );
    assert_eq!(
        profile.packet_crc(),
        PacketCrc::CcittFalse {
            polynomial: 0x1021,
            initial: 0xffff,
            xor_out: 0,
        }
    );
    assert_eq!(profile.time_on_air_us(255), 8_512);
    assert_eq!(profile.logical_packet_airtime_us(500), 17_024);
}

#[test]
fn us915_turbo_spec_owns_the_complete_schedule_contract() {
    assert_eq!(US915_TURBO_SPEC.channels().len(), 51);
    assert_eq!(US915_TURBO_SPEC.slot_us(), 400_000);
    assert_eq!(US915_TURBO_SPEC.cycle_us(), 20_400_000);
    assert_eq!(US915_TURBO_SPEC.supercycle_slots(), 2_601);
    assert_eq!(US915_TURBO_SPEC.supercycle_us(), 1_040_400_000);
    assert_eq!(US915_TURBO_SPEC.occupancy_window_us(), 10_000_000);
    assert_eq!(US915_TURBO_SPEC.channel_occupancy_budget_us(), 390_000);
    assert_eq!(US915_TURBO_SPEC.minimum_measured_bandwidth_hz(), 250_000);
    assert_eq!(US915_TURBO_SPEC.maximum_measured_bandwidth_hz(), 500_000);
    assert_eq!(US915_TURBO_SPEC.boot_quarantine_us(), 10_000_000);
    assert_eq!(US915_TURBO_SPEC.scan_stride(), 7);
    assert_eq!(US915_TURBO_SPEC.scan_dwell_us(), 341_000);
    assert_eq!(US915_TURBO_SPEC.acquisition_observations(), 3);
    assert_eq!(US915_TURBO_SPEC.acquisition_distinct_channels(), 3);
}

#[test]
fn unsupported_hardware_cannot_construct_a_transmitter() {
    let mut support = hardware_support();
    support.whitening = CapabilitySupport::Unsupported;
    assert!(matches!(
        Us915TurboTransmitter::new(
            MonotonicMicros::new(0),
            trusted_clock(),
            configuration(1, support),
        ),
        Err(TurboTransmissionError::Profile(
            TurboProfileError::UnsupportedHardwareCapability {
                capability: TurboPhyCapability::Whitening
            }
        ))
    ));
}

#[test]
fn utc_schedule_has_stable_epoch_and_boundary_vectors() {
    let first = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(0));
    assert_eq!(first.global_slot(), TurboGlobalSlot::new(0).unwrap());
    assert_eq!(first.supercycle(), 0);
    assert_eq!(first.cycle(), SupercycleCycle::new(0).unwrap());
    assert_eq!(first.position().index(), 0);
    assert_eq!(first.channel_index(), TurboChannelIndex::new(23).unwrap());
    assert_eq!(first.frequency(), US915_TURBO_SPEC.channels()[23]);
    assert_eq!(first.starts_at(), ScheduleMicros::new(0));
    assert_eq!(first.offset_us(), 0);
    assert_eq!(first.remaining_us(), US915_TURBO_SPEC.slot_us());

    let final_microsecond =
        US915_TURBO_SPEC.slot_at(ScheduleMicros::new(US915_TURBO_SPEC.slot_us() - 1));
    assert_eq!(final_microsecond.global_slot(), first.global_slot());
    assert_eq!(final_microsecond.channel_index(), first.channel_index());
    assert_eq!(
        final_microsecond.offset_us(),
        US915_TURBO_SPEC.slot_us() - 1
    );
    assert_eq!(final_microsecond.remaining_us(), 1);

    let second = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(US915_TURBO_SPEC.slot_us()));
    assert_eq!(second.global_slot(), TurboGlobalSlot::new(1).unwrap());
    assert_eq!(second.position().index(), 1);
    assert_eq!(second.channel_index(), TurboChannelIndex::new(35).unwrap());
    assert_eq!(
        second.starts_at(),
        ScheduleMicros::new(US915_TURBO_SPEC.slot_us())
    );
    assert_eq!(second.offset_us(), 0);

    let next_cycle = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(US915_TURBO_SPEC.cycle_us()));
    assert_eq!(next_cycle.global_slot(), TurboGlobalSlot::new(51).unwrap());
    assert_eq!(next_cycle.cycle(), SupercycleCycle::new(1).unwrap());
    assert_eq!(next_cycle.position().index(), 0);
    assert_eq!(
        next_cycle.channel_index(),
        TurboChannelIndex::new(35).unwrap()
    );

    let next_supercycle =
        US915_TURBO_SPEC.slot_at(ScheduleMicros::new(US915_TURBO_SPEC.supercycle_us()));
    assert_eq!(next_supercycle.supercycle(), 1);
    assert_eq!(next_supercycle.cycle(), first.cycle());
    assert_eq!(next_supercycle.position(), first.position());
    assert_eq!(next_supercycle.channel_index(), first.channel_index());
}

#[test]
fn largest_representable_schedule_time_remains_a_valid_slot() {
    let slot = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(u64::MAX));
    assert_eq!(
        slot.starts_at().micros().checked_add(slot.offset_us()),
        Some(u64::MAX)
    );
    assert!(slot.offset_us() < US915_TURBO_SPEC.slot_us());
    assert!((1..=US915_TURBO_SPEC.slot_us()).contains(&slot.remaining_us()));
    assert_eq!(
        slot.offset_us().checked_add(slot.remaining_us()),
        Some(US915_TURBO_SPEC.slot_us())
    );
    assert!(slot.channel_index().index() < TURBO_CHANNEL_COUNT);
}

#[test]
fn typed_global_slots_reject_unrepresentable_start_times() {
    let maximum_index = u64::MAX / US915_TURBO_SPEC.slot_us();
    let maximum_slot = TurboGlobalSlot::new(maximum_index).unwrap();
    assert_eq!(
        US915_TURBO_SPEC
            .slot_for_global_slot(maximum_slot)
            .starts_at(),
        ScheduleMicros::new(maximum_index * US915_TURBO_SPEC.slot_us())
    );
    assert_eq!(
        TurboGlobalSlot::new(maximum_index + 1),
        Err(TurboGlobalSlotError::OutsideScheduleRange {
            index: maximum_index + 1,
            maximum_index,
        })
    );
}

#[test]
fn typed_channel_indices_reject_values_outside_the_hop_set() {
    assert_eq!(
        TurboChannelIndex::new(TURBO_CHANNEL_COUNT),
        Err(ChannelLookupError::OutsideHopSet {
            channel_index: TURBO_CHANNEL_COUNT,
        })
    );
    assert_eq!(
        TurboChannelIndex::new(usize::MAX),
        Err(ChannelLookupError::OutsideHopSet {
            channel_index: usize::MAX,
        })
    );
}

#[test]
fn supercycle_is_balanced_and_spread_across_cycle_boundaries() {
    assert_eq!(TURBO_CHANNEL_COUNT, 51);
    for cycle in 0..TURBO_CHANNEL_COUNT as u64 {
        let mut seen = [false; TURBO_CHANNEL_COUNT];
        for position in 0..TURBO_CHANNEL_COUNT as u64 {
            let channel = global_slot_channel(cycle * TURBO_CHANNEL_COUNT as u64 + position);
            assert!(!seen[channel]);
            seen[channel] = true;
        }
        assert_eq!(seen, [true; TURBO_CHANNEL_COUNT]);
    }
    for position in 0..TURBO_CHANNEL_COUNT as u64 {
        let mut seen = [false; TURBO_CHANNEL_COUNT];
        for cycle in 0..TURBO_CHANNEL_COUNT as u64 {
            seen[global_slot_channel(cycle * TURBO_CHANNEL_COUNT as u64 + position)] = true;
        }
        assert_eq!(seen, [true; TURBO_CHANNEL_COUNT]);
    }
    for global_slot in 0..US915_TURBO_SPEC.supercycle_slots() {
        let current = global_slot_channel(global_slot);
        let next = global_slot_channel((global_slot + 1) % US915_TURBO_SPEC.supercycle_slots());
        assert!(current.abs_diff(next) >= 12);
    }
    assert_ne!(
        scheduled_channel(0),
        scheduled_channel(US915_TURBO_SPEC.cycle_us())
    );
    assert_eq!(
        scheduled_channel(0),
        scheduled_channel(US915_TURBO_SPEC.supercycle_us())
    );
}

#[test]
fn every_frequency_revisit_is_outside_the_ten_second_occupancy_window() {
    let mut previous = [None; TURBO_CHANNEL_COUNT];
    for global_slot in 0..=US915_TURBO_SPEC.supercycle_slots() {
        let channel = global_slot_channel(global_slot);
        if let Some(previous_slot) = previous[channel] {
            assert!(global_slot - previous_slot >= 50);
            assert!(
                (global_slot - previous_slot) * US915_TURBO_SPEC.slot_us()
                    >= US915_TURBO_SPEC.occupancy_window_us()
            );
        }
        previous[channel] = Some(global_slot);
    }
}

#[test]
fn turbo_channel_centers_retain_band_edge_guard() {
    for (index, frequency) in US915_TURBO_SPEC.channels().iter().enumerate() {
        assert_eq!(frequency.hz(), 902_500_000 + index as u32 * 500_000);
    }
    assert_eq!(US915_TURBO_SPEC.channels()[0].hz(), 902_500_000);
    assert_eq!(US915_TURBO_SPEC.channels()[50].hz(), 927_500_000);
}

#[test]
fn scanner_stride_visits_every_channel_without_slot_lockstep() {
    let mut seen = [false; TURBO_CHANNEL_COUNT];
    for step in 0..TURBO_CHANNEL_COUNT {
        seen[step * US915_TURBO_SPEC.scan_stride() % TURBO_CHANNEL_COUNT] = true;
    }
    assert_eq!(seen, [true; TURBO_CHANNEL_COUNT]);
    assert_ne!(
        US915_TURBO_SPEC.slot_us() % US915_TURBO_SPEC.scan_dwell_us(),
        0
    );
    assert_ne!(
        US915_TURBO_SPEC.scan_dwell_us() % US915_TURBO_SPEC.slot_us(),
        0
    );
}

#[test]
fn every_datagram_length_round_trips_with_canonical_fragmentation() {
    let cycle = SupercycleCycle::new(7).unwrap();
    let id = DatagramId::new([1, 2, 3]);
    let payload = [0x5a; TURBO_LOGICAL_PACKET_MAX];
    for len in 1..=TURBO_LOGICAL_PACKET_MAX {
        let encoded = encode_datagram(cycle, id, &payload[..len]).unwrap();
        let mut reassembler = TurboReassembler::new(ReassemblyLifetime::new(1_000_000).unwrap());
        let outcome = match &encoded {
            EncodedDatagram::Single(frame) => reassembler
                .ingest(
                    MonotonicMicros::new(0),
                    decode_frame(frame.as_bytes()).unwrap(),
                )
                .unwrap(),
            EncodedDatagram::Fragmented { first, final_frame } => {
                assert_eq!(
                    reassembler.ingest(
                        MonotonicMicros::new(0),
                        decode_frame(first.as_bytes()).unwrap(),
                    ),
                    Ok(ReassemblyOutcome::WaitingForFinal)
                );
                reassembler
                    .ingest(
                        MonotonicMicros::new(1),
                        decode_frame(final_frame.as_bytes()).unwrap(),
                    )
                    .unwrap()
            }
        };
        let ReassemblyOutcome::Complete(datagram) = outcome else {
            panic!("canonical datagram did not complete")
        };
        assert_eq!(datagram.as_bytes(), &payload[..len]);
    }
}

#[test]
fn wire_decoder_rejects_reserved_bits_and_noncanonical_fragments() {
    let cycle = SupercycleCycle::new(0).unwrap();
    let id = DatagramId::new([1, 2, 3]);
    let encoded = encode_datagram(cycle, id, &[7; 251]).unwrap();
    let EncodedDatagram::Fragmented { first, .. } = encoded else {
        panic!("251-byte datagram must fragment")
    };
    let mut reserved = first.as_bytes().to_vec();
    reserved[0] |= 1;
    assert!(matches!(
        decode_frame(&reserved),
        Err(TurboFrameError::ReservedBitsSet { .. })
    ));
    let shortened = &first.as_bytes()[..first.len() - 1];
    assert!(matches!(
        decode_frame(shortened),
        Err(TurboFrameError::NonCanonicalFirstFragment { .. })
    ));
}

#[test]
fn reassembly_lifetime_rejects_a_late_final_fragment() {
    let encoded = encode_datagram(
        SupercycleCycle::new(0).unwrap(),
        DatagramId::new([1, 2, 3]),
        &[7; 251],
    )
    .unwrap();
    let EncodedDatagram::Fragmented { first, final_frame } = encoded else {
        panic!("251-byte datagram must fragment")
    };
    let mut reassembler = TurboReassembler::new(ReassemblyLifetime::new(100).unwrap());
    assert_eq!(
        reassembler.ingest(
            MonotonicMicros::new(0),
            decode_frame(first.as_bytes()).unwrap(),
        ),
        Ok(ReassemblyOutcome::WaitingForFinal)
    );
    assert_eq!(
        reassembler.ingest(
            MonotonicMicros::new(101),
            decode_frame(final_frame.as_bytes()).unwrap(),
        ),
        Err(ReassemblyError::FirstFragmentExpired)
    );
}

#[test]
fn acquisition_graduates_only_after_three_distinct_quality_observations() {
    let mut tracker =
        AcquisitionTracker::new(40, MaximumTransmitUncertainty::new(1_000).unwrap()).unwrap();
    for cycle_index in 0..3u8 {
        let outcome = tracker
            .observe(acquisition_observation(cycle_index, 500))
            .unwrap();
        if cycle_index < 2 {
            assert!(matches!(outcome, AcquisitionOutcome::Candidate { .. }));
        } else {
            let AcquisitionOutcome::Operational { schedule } = outcome else {
                panic!("expected operational acquisition, got {outcome:?}")
            };
            assert_eq!(schedule.observations(), 3);
            assert_eq!(schedule.distinct_channels(), 3);
        }
    }
}

#[test]
fn repeated_channel_evidence_cannot_finish_acquisition() {
    let mut tracker =
        AcquisitionTracker::new(40, MaximumTransmitUncertainty::new(1_000).unwrap()).unwrap();
    let channel = TurboChannelIndex::new(23).unwrap();
    for cycle_index in 0..3u8 {
        let cycle = SupercycleCycle::new(cycle_index).unwrap();
        let position = US915_TURBO_SPEC.slot_position_for_channel(cycle, channel);
        let global_slot =
            u64::from(cycle_index) * TURBO_CHANNEL_COUNT as u64 + u64::from(position.index());
        let beacon = AcquisitionBeacon::new(cycle, 0).unwrap();
        let received_at = global_slot * US915_TURBO_SPEC.slot_us()
            + beacon.completes_at_slot_offset_us(US915_TURBO_SPEC.phy());
        let outcome = tracker
            .observe(AcquisitionObservation::from_beacon(
                MonotonicMicros::new(received_at),
                channel.index(),
                beacon,
                US915_TURBO_SPEC.phy(),
                500,
            ))
            .unwrap();
        assert!(matches!(outcome, AcquisitionOutcome::Candidate { .. }));
    }
}

#[test]
fn contradiction_discards_all_prior_graduation_progress() {
    let mut tracker =
        AcquisitionTracker::new(40, MaximumTransmitUncertainty::new(1_000).unwrap()).unwrap();
    assert!(matches!(
        tracker.observe(acquisition_observation(0, 500)).unwrap(),
        AcquisitionOutcome::Candidate { .. }
    ));
    let reset = tracker
        .observe(acquisition_observation_with_offset(1, 500, 100_000))
        .unwrap();
    let AcquisitionOutcome::ContradictionReset { candidate } = reset else {
        panic!("contradictory evidence must reset acquisition")
    };
    assert_eq!(candidate.observations(), 1);
    assert_eq!(candidate.distinct_channels(), 1);
    assert!(matches!(
        tracker
            .observe(acquisition_observation_with_offset(2, 500, 100_000))
            .unwrap(),
        AcquisitionOutcome::Candidate { .. }
    ));
    assert!(matches!(
        tracker
            .observe(acquisition_observation_with_offset(3, 500, 100_000))
            .unwrap(),
        AcquisitionOutcome::Operational { .. }
    ));
}

#[test]
fn imprecise_observations_do_not_advance_acquisition() {
    let mut tracker =
        AcquisitionTracker::new(40, MaximumTransmitUncertainty::new(1_000).unwrap()).unwrap();
    assert_eq!(
        tracker.observe(acquisition_observation(0, 1_001)),
        Err(AcquisitionTrackerError::TimingUncertaintyExceeded {
            actual_us: 1_001,
            maximum_us: 1_000,
        })
    );
    let AcquisitionOutcome::Candidate { candidate } =
        tracker.observe(acquisition_observation(1, 500)).unwrap()
    else {
        panic!("first accepted observation must remain a candidate")
    };
    assert_eq!(candidate.observations(), 1);
}

#[test]
fn validated_acquisition_can_drive_the_complete_transmit_state_machine() {
    let schedule = validated_acquired_schedule();
    let window = schedule.window_at(schedule.observed_at()).unwrap();
    let now = schedule.observed_at().micros() + 10_000;
    let channel = scheduled_channel(window.center_schedule_us() + 10_000);
    let receive_dwell =
        TurboRadioDwell::new(MonotonicMicros::new(now), MonotonicMicros::new(now + 1_000)).unwrap();
    assert_eq!(
        schedule
            .authorize_receive(receive_dwell)
            .unwrap()
            .channel_index(),
        TurboChannelIndex::new(channel).unwrap()
    );
    let mut transmitter = Us915TurboTransmitter::new_with_acquired_schedule(
        MonotonicMicros::new(0),
        schedule,
        configuration(7, hardware_support()),
    )
    .unwrap();
    let prepared = transmitter
        .prepare_after_final_clear(
            final_clear_grant(channel, now),
            MonotonicMicros::new(now),
            DatagramId::new([7, 8, 9]),
            &[1, 2, 3],
        )
        .unwrap();
    let keyed_airtime_us = prepared.keyed_airtime_us();
    let active = transmitter
        .mark_rf_started(prepared, MonotonicMicros::new(now + 1))
        .unwrap();
    assert!(transmitter
        .complete(
            active,
            MonotonicMicros::new(now + keyed_airtime_us + 1),
            keyed_airtime_us,
        )
        .is_ok());
}

#[test]
fn acquired_transmit_authority_expires_with_its_uncertainty_budget() {
    let schedule = validated_acquired_schedule();
    let now = schedule.observed_at().micros() + 20_000_000;
    let window = schedule.window_at(MonotonicMicros::new(now)).unwrap();
    let channel = scheduled_channel(window.center_schedule_us());
    let mut transmitter = Us915TurboTransmitter::new_with_acquired_schedule(
        MonotonicMicros::new(0),
        schedule,
        configuration(8, hardware_support()),
    )
    .unwrap();
    assert_eq!(
        transmitter
            .prepare_after_final_clear(
                final_clear_grant(channel, now),
                MonotonicMicros::new(now),
                DatagramId::new([8, 9, 10]),
                &[1, 2, 3],
            )
            .err(),
        Some(TurboTransmissionError::Clock(
            ClockError::TransmitUncertaintyExceeded {
                actual_us: 1_300,
                maximum_us: 1_000,
            }
        ))
    );
}

#[test]
fn authoritative_time_replaces_acquired_time_through_quarantine() {
    let schedule = validated_acquired_schedule();
    let observed_at = schedule.observed_at().micros() + 1_000;
    let mut transmitter = Us915TurboTransmitter::new_with_acquired_schedule(
        MonotonicMicros::new(0),
        schedule,
        configuration(9, hardware_support()),
    )
    .unwrap();
    assert_eq!(
        transmitter
            .update_clock(
                MonotonicMicros::new(observed_at),
                ScheduleMicros::new(1_900_000_000_000_000),
                100,
                1,
                TrustedTimeSource::Gnss,
                UtcTimescale::PosixUnsmeared,
            )
            .unwrap(),
        ClockUpdateDisposition::Discontinuous {
            transmit_not_before_us: observed_at + US915_TURBO_SPEC.boot_quarantine_us(),
        }
    );
    assert_eq!(
        transmitter.status(MonotonicMicros::new(observed_at)),
        TurboTransmitterStatus::Quarantined {
            transmit_not_before_us: observed_at + US915_TURBO_SPEC.boot_quarantine_us(),
        }
    );
}

#[test]
fn acquisition_beacons_are_bounded_to_one_attempt_after_a_quiet_cycle() {
    let mut controller = AcquisitionBeaconController::new(10);
    let entropy = 0x5052_4e53_5455_5242;
    let candidate_for = |cycle: u64| {
        let mixed = entropy
            ^ cycle.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ entropy.rotate_left((cycle % 63) as u32 + 1);
        cycle * TURBO_CHANNEL_COUNT as u64 + mixed % TURBO_CHANNEL_COUNT as u64
    };
    assert!(matches!(
        controller.plan(candidate_for(10), AcquisitionSlotActivity::Quiet, entropy),
        AcquisitionBeaconPlan::Suppress {
            reason: AcquisitionBeaconSuppression::InitialQuietCycleIncomplete
        }
    ));
    assert!(matches!(
        controller.plan(candidate_for(11), AcquisitionSlotActivity::Quiet, entropy),
        AcquisitionBeaconPlan::Contend { .. }
    ));
    assert!(matches!(
        controller.plan(candidate_for(11), AcquisitionSlotActivity::Quiet, entropy),
        AcquisitionBeaconPlan::Suppress {
            reason: AcquisitionBeaconSuppression::AlreadyAttemptedThisCycle
        }
    ));
}

#[test]
fn contention_grant_is_bound_to_channel_and_final_clear_instant() {
    let now = 10_100_000;
    let channel = scheduled_channel(now);
    let grant = final_clear_grant(channel, now);
    let mut transmitter = transmitter();
    let prepared = transmitter
        .prepare_after_final_clear(
            grant,
            MonotonicMicros::new(now),
            DatagramId::new([1, 2, 3]),
            &[9; 500],
        )
        .unwrap();
    assert_eq!(
        prepared.channel_index(),
        TurboChannelIndex::new(channel).unwrap()
    );
    assert_eq!(prepared.keyed_airtime_us(), 17_024);
    assert_eq!(prepared.power().dbm(), 28);
}

#[test]
fn stale_or_wrong_channel_clearance_cannot_reserve_rf() {
    let now = 10_100_000;
    let channel = scheduled_channel(now);
    let mut stale = transmitter();
    assert!(matches!(
        stale.prepare_after_final_clear(
            final_clear_grant(channel, now),
            MonotonicMicros::new(now + timing().final_clear_validity_us() + 1),
            DatagramId::new([1, 2, 3]),
            &[9],
        ),
        Err(TurboTransmissionError::FinalClearExpired { .. })
    ));

    let mut wrong = transmitter();
    assert_eq!(
        wrong
            .prepare_after_final_clear(
                final_clear_grant((channel + 1) % TURBO_CHANNEL_COUNT, now),
                MonotonicMicros::new(now),
                DatagramId::new([1, 2, 3]),
                &[9],
            )
            .err(),
        Some(TurboTransmissionError::FinalClearFromWrongChannel {
            expected: channel,
            actual: (channel + 1) % TURBO_CHANNEL_COUNT,
        })
    );
}

#[test]
fn prepared_capabilities_are_bound_to_their_transmitter_instance() {
    let now = 10_100_000;
    let channel = scheduled_channel(now);
    let mut first = transmitter();
    let mut second = Us915TurboTransmitter::new(
        MonotonicMicros::new(0),
        trusted_clock(),
        configuration(2, hardware_support()),
    )
    .unwrap();
    let first_prepared = first
        .prepare_after_final_clear(
            final_clear_grant(channel, now),
            MonotonicMicros::new(now),
            DatagramId::new([1, 2, 3]),
            &[9],
        )
        .unwrap();
    let second_prepared = second
        .prepare_after_final_clear(
            final_clear_grant(channel, now),
            MonotonicMicros::new(now),
            DatagramId::new([4, 5, 6]),
            &[9],
        )
        .unwrap();
    assert_eq!(
        second
            .mark_rf_started(first_prepared, MonotonicMicros::new(now + 1))
            .err(),
        Some(TurboTransmissionError::InvalidPreparedToken)
    );
    second.abort_before_rf(second_prepared).unwrap();
}

#[test]
fn transmitter_reserves_before_rf_and_accounts_failed_delivery_as_keyed_airtime() {
    let now = 10_100_000;
    let channel = scheduled_channel(now);
    let mut transmitter = transmitter();
    let prepared = transmitter
        .prepare_after_final_clear(
            final_clear_grant(channel, now),
            MonotonicMicros::new(now),
            DatagramId::new([1, 2, 3]),
            &[9; 500],
        )
        .unwrap();
    let keyed_airtime = prepared.keyed_airtime_us();
    let active = transmitter
        .mark_rf_started(prepared, MonotonicMicros::new(now + 10))
        .unwrap();
    let report = transmitter
        .complete(
            active,
            MonotonicMicros::new(now + 10 + keyed_airtime + timing().interframe_us()),
            keyed_airtime,
        )
        .unwrap();
    assert_eq!(report.actual_airtime_us(), keyed_airtime);
    assert_eq!(transmitter.channel_keyed_airtime_us(channel), keyed_airtime);
    assert_eq!(transmitter.channel_use_airtime_us(channel), keyed_airtime);
}

#[test]
fn actual_airtime_overrun_is_accounted_and_faults_closed() {
    let now = 10_100_000;
    let channel = scheduled_channel(now);
    let mut transmitter = transmitter();
    let prepared = transmitter
        .prepare_after_final_clear(
            final_clear_grant(channel, now),
            MonotonicMicros::new(now),
            DatagramId::new([1, 2, 3]),
            &[9],
        )
        .unwrap();
    let reserved = prepared.keyed_airtime_us();
    let active = transmitter
        .mark_rf_started(prepared, MonotonicMicros::new(now + 1))
        .unwrap();
    assert_eq!(
        transmitter
            .complete(
                active,
                MonotonicMicros::new(now + reserved + 2),
                reserved + 1,
            )
            .err(),
        Some(TurboTransmissionError::Faulted {
            reason: TurboFault::ActualAirtimeExceededReservation,
        })
    );
    assert_eq!(
        transmitter.status(MonotonicMicros::new(now + reserved + 2)),
        TurboTransmitterStatus::Faulted {
            reason: TurboFault::ActualAirtimeExceededReservation,
        }
    );
}

#[test]
fn abort_before_rf_releases_both_occupancy_and_balance_reservations() {
    let now = 10_100_000;
    let channel = scheduled_channel(now);
    let mut transmitter = transmitter();
    let prepared = transmitter
        .prepare_after_final_clear(
            final_clear_grant(channel, now),
            MonotonicMicros::new(now),
            DatagramId::new([1, 2, 3]),
            &[9; 500],
        )
        .unwrap();
    transmitter.abort_before_rf(prepared).unwrap();
    assert_eq!(transmitter.channel_use_airtime_us(channel), 0);
    assert_eq!(
        transmitter.status(MonotonicMicros::new(now)),
        TurboTransmitterStatus::Idle
    );
}

#[test]
fn discontinuous_clock_update_reenters_quarantine() {
    let mut transmitter = transmitter();
    let outcome = transmitter
        .update_clock(
            MonotonicMicros::new(11_000_000),
            ScheduleMicros::new(99_000_000),
            100,
            1,
            TrustedTimeSource::AuthenticatedHost,
            UtcTimescale::PosixUnsmeared,
        )
        .unwrap();
    assert_eq!(
        outcome,
        ClockUpdateDisposition::Discontinuous {
            transmit_not_before_us: 21_000_000,
        }
    );
    assert_eq!(
        transmitter.status(MonotonicMicros::new(20_999_999)),
        TurboTransmitterStatus::Quarantined {
            transmit_not_before_us: 21_000_000,
        }
    );
}

#[cfg(feature = "std")]
#[test]
fn deterministic_contention_simulation_executes_the_production_state_machines() {
    let result = simulate_contention(
        ContentionSimulation {
            seed: 0x5052_4e53,
            nodes: 8,
            queued_packets_per_node: 4,
            logical_packet_bytes: 500,
            rounds: 256,
            packet_loss_per_mille: 100,
        },
        US915_TURBO_SPEC.phy(),
    )
    .unwrap();
    assert!(result.delivered_packets > 0);
    assert!(result.occupied_airtime_us >= result.delivered_packets * 17_024);
    assert!(result.jain_fairness_millionths >= 800_000);
}

#[test]
fn acquisition_simulation_graduates_only_through_the_production_tracker() {
    let input = acquisition_simulation(AcquisitionEnvironment::SingleSchedule);
    let result = simulate_acquisition(input, US915_TURBO_SPEC.phy()).unwrap();
    assert!(result.acquired_trials > 0);
    assert_eq!(result.off_primary_schedule_acquisitions, 0);
    assert_eq!(result.contradiction_resets, 0);
    assert_eq!(result.maximum_primary_phase_error_us, 0);
}

#[test]
fn mixed_schedule_population_can_graduate_either_coherent_phase() {
    let input = acquisition_simulation(AcquisitionEnvironment::MixedSchedules {
        alternate_per_mille: 500,
        alternate_cycle_offset: 1,
    });
    let result = simulate_acquisition(input, US915_TURBO_SPEC.phy()).unwrap();
    assert!(result.acquired_trials > result.off_primary_schedule_acquisitions);
    assert!(result.off_primary_schedule_acquisitions > 0);
    assert!(result.maximum_primary_phase_error_us >= US915_TURBO_SPEC.cycle_us());
}

#[test]
fn mixed_completion_timing_forces_contradiction_resets() {
    let input = acquisition_simulation(AcquisitionEnvironment::MixedCompletionTiming {
        displaced_per_mille: 500,
        completion_displacement_us: 2_000,
    });
    let result = simulate_acquisition(input, US915_TURBO_SPEC.phy()).unwrap();
    assert!(result.contradiction_resets > 0);
    assert!(result.acquired_trials > 0);
}

#[test]
fn acquisition_simulation_rejects_invalid_clock_and_environment_models_up_front() {
    let baseline = acquisition_simulation(AcquisitionEnvironment::SingleSchedule);
    assert_eq!(
        simulate_acquisition(
            AcquisitionSimulation {
                actual_clock_drift_ppm: 41,
                ..baseline
            },
            US915_TURBO_SPEC.phy(),
        ),
        Err(AcquisitionSimulationError::ActualClockDriftExceedsBound {
            actual_ppm: 41,
            maximum_ppm: 40,
        })
    );
    assert_eq!(
        simulate_acquisition(
            AcquisitionSimulation {
                timestamp_error_span_us: 501,
                ..baseline
            },
            US915_TURBO_SPEC.phy(),
        ),
        Err(
            AcquisitionSimulationError::TimestampErrorExceedsDeclaredUncertainty {
                error_us: 501,
                uncertainty_us: 500,
            }
        )
    );
    assert_eq!(
        simulate_acquisition(
            AcquisitionSimulation {
                environment: AcquisitionEnvironment::MixedSchedules {
                    alternate_per_mille: 500,
                    alternate_cycle_offset: 0,
                },
                ..baseline
            },
            US915_TURBO_SPEC.phy(),
        ),
        Err(AcquisitionSimulationError::AlternateCycleOffsetOutsideRange { cycle_offset: 0 })
    );
    assert_eq!(
        simulate_acquisition(
            AcquisitionSimulation {
                environment: AcquisitionEnvironment::MixedCompletionTiming {
                    displaced_per_mille: 500,
                    completion_displacement_us: 0,
                },
                ..baseline
            },
            US915_TURBO_SPEC.phy(),
        ),
        Err(AcquisitionSimulationError::EmptyCompletionDisplacement)
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn schedule_repeats_only_after_the_complete_supercycle(schedule_us in any::<u64>()) {
        if let Some(next_supercycle) = schedule_us.checked_add(US915_TURBO_SPEC.supercycle_us()) {
            prop_assert_eq!(scheduled_channel(schedule_us), scheduled_channel(next_supercycle));
        }
    }

    #[test]
    fn clock_uncertainty_is_monotonic_with_forward_holdover(
        initial in 1u64..=100_000,
        drift_ppm in 1u32..=10_000,
        earlier in 0u64..=10_000_000,
        later_delta in 0u64..=10_000_000,
    ) {
        let clock = TrustedScheduleClock::new(
            MonotonicMicros::new(0),
            ScheduleMicros::new(0),
            initial,
            drift_ppm,
            TrustedTimeSource::Gnss,
            UtcTimescale::PosixUnsmeared,
        ).unwrap();
        let later = earlier.saturating_add(later_delta);
        let earlier_window = clock.window_at(MonotonicMicros::new(earlier)).unwrap();
        let later_window = clock.window_at(MonotonicMicros::new(later)).unwrap();
        prop_assert!(later_window.uncertainty_us() >= earlier_window.uncertainty_us());
    }
}
