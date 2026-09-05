use super::*;
use crate::interfaces::subghz::frequency_hopping::{
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

#[test]
fn us915_turbo_profile_fully_names_the_phy() {
    assert_eq!(US915_TURBO_PHY.validate(), Ok(()));
    assert_eq!(US915_TURBO_PHY.bit_rate().bps(), 250_000);
    assert_eq!(US915_TURBO_PHY.frequency_deviation().hz(), 62_500);
    assert_eq!(US915_TURBO_PHY.receiver_bandwidth().hz(), 467_000);
    assert_eq!(US915_TURBO_PHY.modulation_index(), ModulationIndex::Half);
    assert_eq!(US915_TURBO_PHY.sync_word(), *b"PRNS");
    assert_eq!(
        US915_TURBO_PHY.data_whitening(),
        DataWhitening::Pn9 {
            polynomial: 0x021,
            seed: 0x1ff,
        }
    );
    assert_eq!(
        US915_TURBO_PHY.packet_crc(),
        PacketCrc::CcittFalse {
            polynomial: 0x1021,
            initial: 0xffff,
            xor_out: 0,
        }
    );
    assert_eq!(US915_TURBO_PHY.time_on_air_us(255), 8_512);
    assert_eq!(US915_TURBO_PHY.logical_packet_airtime_us(500), 17_024);
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
fn supercycle_is_balanced_and_spread_across_cycle_boundaries() {
    assert_eq!(TURBO_CHANNEL_COUNT, 51);
    for cycle in 0..TURBO_CHANNEL_COUNT as u64 {
        let mut seen = [false; TURBO_CHANNEL_COUNT];
        for position in 0..TURBO_CHANNEL_COUNT as u64 {
            let channel =
                channel_index_for_global_slot(cycle * TURBO_CHANNEL_COUNT as u64 + position);
            assert!(!seen[channel]);
            seen[channel] = true;
        }
        assert_eq!(seen, [true; TURBO_CHANNEL_COUNT]);
    }
    for position in 0..TURBO_CHANNEL_COUNT as u64 {
        let mut seen = [false; TURBO_CHANNEL_COUNT];
        for cycle in 0..TURBO_CHANNEL_COUNT as u64 {
            seen[channel_index_for_global_slot(cycle * TURBO_CHANNEL_COUNT as u64 + position)] =
                true;
        }
        assert_eq!(seen, [true; TURBO_CHANNEL_COUNT]);
    }
    for global_slot in 0..TURBO_SUPERCYCLE_SLOTS {
        let current = channel_index_for_global_slot(global_slot);
        let next = channel_index_for_global_slot((global_slot + 1) % TURBO_SUPERCYCLE_SLOTS);
        assert!(current.abs_diff(next) >= 12);
    }
    assert_ne!(channel_index_at(0), channel_index_at(TURBO_CYCLE_US));
    assert_eq!(channel_index_at(0), channel_index_at(TURBO_SUPERCYCLE_US));
}

#[test]
fn every_frequency_revisit_is_outside_the_ten_second_occupancy_window() {
    let mut previous = [None; TURBO_CHANNEL_COUNT];
    for global_slot in 0..=TURBO_SUPERCYCLE_SLOTS {
        let channel = channel_index_for_global_slot(global_slot);
        if let Some(previous_slot) = previous[channel] {
            assert!(global_slot - previous_slot >= 50);
            assert!((global_slot - previous_slot) * TURBO_SLOT_US >= 20_000_000);
        }
        previous[channel] = Some(global_slot);
    }
}

#[test]
fn turbo_channel_centers_retain_band_edge_guard() {
    for (index, frequency) in US915_TURBO_CHANNELS.into_iter().enumerate() {
        assert_eq!(frequency.hz(), 902_500_000 + index as u32 * 500_000);
    }
    assert_eq!(US915_TURBO_CHANNELS[0].hz(), 902_500_000);
    assert_eq!(US915_TURBO_CHANNELS[50].hz(), 927_500_000);
}

#[test]
fn scanner_stride_visits_every_channel_without_slot_lockstep() {
    let mut seen = [false; TURBO_CHANNEL_COUNT];
    for step in 0..TURBO_CHANNEL_COUNT {
        seen[step * TURBO_SCAN_STRIDE % TURBO_CHANNEL_COUNT] = true;
    }
    assert_eq!(seen, [true; TURBO_CHANNEL_COUNT]);
    assert_ne!(TURBO_SLOT_US % TURBO_SCAN_DWELL_US, 0);
    assert_ne!(TURBO_SCAN_DWELL_US % TURBO_SLOT_US, 0);
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
fn acquisition_evidence_never_becomes_transmit_authority() {
    let mut tracker = AcquisitionTracker::new(40).unwrap();
    for cycle_index in 0..3u8 {
        let cycle = SupercycleCycle::new(cycle_index).unwrap();
        let beacon = AcquisitionBeacon::new(cycle, 0).unwrap();
        let global_slot = u64::from(cycle_index) * TURBO_CHANNEL_COUNT as u64;
        let received_at =
            global_slot * TURBO_SLOT_US + beacon.completes_at_slot_offset_us(US915_TURBO_PHY);
        let outcome = tracker
            .observe(AcquisitionObservation::from_beacon(
                MonotonicMicros::new(received_at),
                channel_index_for_global_slot(global_slot),
                beacon,
                US915_TURBO_PHY,
                500,
            ))
            .unwrap();
        if cycle_index == 0 {
            assert_eq!(outcome, AcquisitionOutcome::Provisional { observations: 1 });
        } else {
            assert!(matches!(outcome, AcquisitionOutcome::ReceivePhase { .. }));
        }
    }
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
    let channel = channel_index_at(now);
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
    assert_eq!(prepared.channel_index(), channel);
    assert_eq!(prepared.keyed_airtime_us(), 17_024);
    assert_eq!(prepared.power().dbm(), 28);
}

#[test]
fn stale_or_wrong_channel_clearance_cannot_reserve_rf() {
    let now = 10_100_000;
    let channel = channel_index_at(now);
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
    let channel = channel_index_at(now);
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
    let channel = channel_index_at(now);
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
    let channel = channel_index_at(now);
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
    let channel = channel_index_at(now);
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
        US915_TURBO_PHY,
    )
    .unwrap();
    assert!(result.delivered_packets > 0);
    assert!(result.occupied_airtime_us >= result.delivered_packets * 17_024);
    assert!(result.jain_fairness_millionths >= 800_000);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn schedule_repeats_only_after_the_complete_supercycle(schedule_us in any::<u64>()) {
        if let Some(next_supercycle) = schedule_us.checked_add(TURBO_SUPERCYCLE_US) {
            prop_assert_eq!(channel_index_at(schedule_us), channel_index_at(next_supercycle));
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
