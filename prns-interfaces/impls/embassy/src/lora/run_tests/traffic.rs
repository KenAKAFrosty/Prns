use super::*;

#[test]
fn receive_faults_preserve_or_discard_partial_frames_according_to_radio_recovery() {
    for pending_tx in [false, true] {
        for fault in [Fault::Recoverable, Fault::ResetRequired] {
            let lab = Lab::new();
            let mut control = LoRaControl::new();
            let (_, target) = control.split();
            let profile = RadioProfile::SubG(US915_AUTO_LORA_PROFILE);
            let status = EmbassyInterfaceStatus::new_accounted(
                LoRaInterface::<Radio>::interface_id(&profile),
                ConnectionState::Initializing,
            );
            let spectrum = LoRaSpectrumStatus::new();
            let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
            let mut queue = [0; LORA_TX_QUEUE_BYTES];
            let interface = LoRaInterface::new(LoRaInterfaceInput {
                radio: Radio(lab.clone()),
                configuration: LoRaConfigurationState::Configured(LoRaConfiguration::manual(
                    profile,
                )),
                airtime_policy: AirtimePolicy::Regional,
                tx_queue: &mut queue,
                control: target,
                status: &status,
                spectrum: &spectrum,
                lifecycle: lifecycle.dyn_sender(),
            })
            .unwrap();
            let seam = Seam {
                lab: lab.clone(),
                current: Vec::new(),
                sink: Vec::new(),
            };
            let scenario = async {
                lab.receive(&[0x11, 0xab]).await;
                while status.frame_accounting().unwrap().frames_in < 1 {
                    yield_now().await;
                }
                if pending_tx {
                    lab.outbound.send(vec![0x42; 10]).await;
                    while lab.accepted.get() < 1 {
                        yield_now().await;
                    }
                }
                lab.fault.set(Some((Operation::Read, fault)));
                lab.receive(&[0, 0xee]).await;
                while lab.fault.get().is_some() {
                    yield_now().await;
                }
                lab.receive(&[0x11, 0xcd]).await;
                while status.frame_accounting().unwrap().frames_in < 2 {
                    yield_now().await;
                }
                assert_eq!(status.connection(), ConnectionState::Connected);
                match fault {
                    Fault::Recoverable => {
                        assert_eq!(&*lab.delivered.borrow(), &[vec![0xab, 0xcd]]);
                        assert_eq!(lab.initializations.get(), 1);
                        assert_eq!(spectrum.snapshot().radio_recoveries, 0);
                        assert_eq!(status.frame_accounting().unwrap().undecodable, 0);
                    }
                    Fault::ResetRequired => {
                        assert!(lab.delivered.borrow().is_empty());
                        assert_eq!(lab.initializations.get(), 2);
                        assert_eq!(spectrum.snapshot().radio_recoveries, 1);
                        assert_eq!(status.frame_accounting().unwrap().undecodable, 1);
                    }
                }
            };
            block_on(async {
                match select(
                    interface.run(seam),
                    embassy_time::with_timeout(Duration::from_secs(3), scenario),
                )
                .await
                {
                    Either::First(()) => panic!("radio task ended"),
                    Either::Second(result) => {
                        result.unwrap();
                    }
                }
            });
        }
    }
}

#[test]
fn receive_events_preserve_channel_evidence_frame_accounting_and_signal_quality() {
    use prns_core::interfaces::{FrameAccounting, SignalQualityTenthsPercent, SnrQuarterDb};
    let lab = Lab::new();
    let mut seam = Seam {
        lab: lab.clone(),
        current: Vec::new(),
        sink: Vec::new(),
    };
    let mut radio = Radio(lab.clone());
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    let status = EmbassyInterfaceStatus::new_accounted(
        LoRaInterface::<Radio>::interface_id(&profile),
        ConnectionState::Connected,
    );
    let spectrum = LoRaSpectrumStatus::default();
    assert_eq!(spectrum.snapshot().channel_busy_per_mille, 0);
    let mut activity = DemodulatorActivity::new();
    let mut throughput = ThroughputLedger::new();
    let mut reassembler = LoRaReassembler::new();
    block_on(async {
        for (event, expected) in [
            (RadioEvent::PreambleDetected, ChannelObservation::Busy),
            (RadioEvent::HeaderValid, ChannelObservation::Busy),
            (RadioEvent::HeaderError, ChannelObservation::Busy),
            (RadioEvent::CrcError, ChannelObservation::Busy),
            (RadioEvent::Timeout, ChannelObservation::Unknown),
            (RadioEvent::SpuriousInterrupt, ChannelObservation::Unknown),
        ] {
            let evidence = observe_radio_event(
                event,
                InstantMillis(0),
                ReceivePath {
                    profile: &profile,
                    activity: &mut activity,
                    spectrum: &spectrum,
                    rx_buf: &[],
                    status: &status,
                    throughput: &mut throughput,
                    reassembler: &mut reassembler,
                    seam: &mut seam,
                },
            )
            .await;
            assert_eq!(
                evidence,
                ChannelEvidence {
                    observation: expected,
                    decoded_airtime_us: None
                }
            );
        }
        let phy = PacketPhyStats {
            snr: Some(SnrQuarterDb::new(-36)),
            ..PacketPhyStats::default()
        };
        for bytes in [
            &[][..],
            &[0][..],
            &[0x11, 0xab][..],
            &[0, 0xcd][..],
            &[0x11, 0xee][..],
            &[0x11, 0xff][..],
        ] {
            let event = RadioEvent::Frame(ReceivedAirFrame {
                len: bytes.len(),
                phy,
            });
            let evidence = observe_radio_event(
                event,
                InstantMillis(1),
                ReceivePath {
                    profile: &profile,
                    activity: &mut activity,
                    spectrum: &spectrum,
                    rx_buf: bytes,
                    status: &status,
                    throughput: &mut throughput,
                    reassembler: &mut reassembler,
                    seam: &mut seam,
                },
            )
            .await;
            assert_eq!(
                evidence,
                ChannelEvidence {
                    observation: ChannelObservation::Busy,
                    decoded_airtime_us: Some(profile.time_on_air_us(bytes.len()))
                }
            );
        }
        let mut noise = NoiseFloor::new();
        activity.preamble_detected(0, profile);
        let deadline = activity.next_poll_ms(0, 1);
        lab.fault.set(Some((Operation::Rssi, Fault::Recoverable)));
        assert_eq!(
            sample_channel(
                &mut radio,
                InstantMillis(0),
                &mut activity,
                &spectrum,
                &mut noise
            )
            .await,
            Ok(ChannelObservation::Busy)
        );
        assert!(
            lab.fault.get().is_some(),
            "a latched preamble avoids an RSSI SPI transaction"
        );
        assert_eq!(
            sample_channel(
                &mut radio,
                InstantMillis(deadline),
                &mut activity,
                &spectrum,
                &mut noise
            )
            .await,
            Err(Fault::Recoverable)
        );
    });
    assert_eq!(
        status.frame_accounting(),
        Some(FrameAccounting {
            frames_in: 6,
            malformed: 2,
            protocol_violations: 3,
            undecodable: 1,
            delivered: 2
        })
    );
    assert_eq!(&*lab.delivered.borrow(), &[vec![0xcd], vec![0xee, 0xff]]);
    let expected_phy = PacketPhyStats {
        snr: Some(SnrQuarterDb::new(-36)),
        quality: SignalQualityTenthsPercent::new(0),
        ..PacketPhyStats::default()
    };
    assert_eq!(&*lab.received_phy.borrow(), &[expected_phy, expected_phy]);
    assert_eq!(spectrum.snapshot().false_preambles, 2);
}

#[test]
fn legacy_radio_adapters_explicitly_reject_high_frequency_without_touching_hardware() {
    let calls = Rc::new(crate::lora::tests::RadioCalls::default());
    let mut radio = crate::lora::tests::RecordingRadio {
        calls: calls.clone(),
    };
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    assert_eq!(
        radio.validate_band_profile(profile),
        Err(RadioProfileCompatibilityError::UnsupportedBand)
    );
    assert_eq!(
        block_on(radio.initialize_band(profile)),
        Err(BandRadioError::UnsupportedBand)
    );
    assert_eq!(calls.initialize.get(), 0);
}

#[test]
fn sensing_and_transmit_faults_recover_without_losing_packet_dispositions() {
    for operation in [
        Operation::Rssi,
        Operation::Poll,
        Operation::Transmit,
        Operation::ArmRx,
    ] {
        for fault in [Fault::Recoverable, Fault::ResetRequired] {
            let lab = Lab::new();
            let mut control = LoRaControl::new();
            let (_, target) = control.split();
            let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
            let status = EmbassyInterfaceStatus::new_accounted(
                LoRaInterface::<Radio>::interface_id(&profile),
                ConnectionState::Initializing,
            );
            let spectrum = LoRaSpectrumStatus::new();
            let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
            let mut queue = [0; LORA_TX_QUEUE_BYTES];
            let interface = LoRaInterface::new(LoRaInterfaceInput {
                radio: Radio(lab.clone()),
                configuration: LoRaConfigurationState::Configured(LoRaConfiguration::manual(
                    profile,
                )),
                airtime_policy: AirtimePolicy::Regional,
                tx_queue: &mut queue,
                control: target,
                status: &status,
                spectrum: &spectrum,
                lifecycle: lifecycle.dyn_sender(),
            })
            .unwrap();
            let seam = Seam {
                lab: lab.clone(),
                current: Vec::new(),
                sink: Vec::new(),
            };
            let scenario = async {
                while !lab.receiving.get() {
                    yield_now().await;
                }
                lab.fault.set(Some((operation, fault)));
                if operation == Operation::Rssi {
                    while lab.fault.get().is_some() {
                        yield_now().await;
                    }
                    assert_eq!(
                        spectrum.snapshot().radio_recoveries,
                        u32::from(fault == Fault::ResetRequired)
                    );
                    // The next failure occurs during active contention.
                    lab.fault.set(Some((operation, fault)));
                }
                lab.outbound.send(vec![0x42; 10]).await;
                while lab.dispositions.borrow().is_empty() {
                    yield_now().await;
                }
                let expected = if operation == Operation::Transmit {
                    OutboundDisposition::Dropped(OutboundDropReason::TransportFailure)
                } else {
                    OutboundDisposition::Sent
                };
                assert_eq!(&*lab.dispositions.borrow(), &[expected]);
                assert!(lab.fault.get().is_none());
                let recoveries = if fault == Fault::ResetRequired {
                    if operation == Operation::Rssi {
                        2
                    } else {
                        1
                    }
                } else {
                    0
                };
                assert_eq!(spectrum.snapshot().radio_recoveries, recoveries);
                let receive_retry =
                    usize::from(operation == Operation::ArmRx && fault == Fault::Recoverable);
                assert_eq!(
                    lab.initializations.get(),
                    1 + recoveries as usize + receive_retry
                );
                assert!(lab.receiving.get());
                lab.outbound.send(vec![0x43; 10]).await;
                while lab.dispositions.borrow().len() < 2 {
                    yield_now().await;
                }
                assert_eq!(lab.dispositions.borrow()[1], OutboundDisposition::Sent);
                assert_eq!(
                    &lab.transmissions.borrow().last().unwrap().1[1..],
                    &[0x43; 10]
                );
            };
            block_on(async {
                match select(
                    interface.run(seam),
                    embassy_time::with_timeout(Duration::from_secs(5), scenario),
                )
                .await
                {
                    Either::First(()) => panic!("radio task ended"),
                    Either::Second(result) => {
                        result.unwrap();
                    }
                }
            });
        }
    }
}

#[test]
fn final_channel_check_observes_latched_frames_and_fails_closed_on_rssi_errors() {
    for fault in [None, Some(Fault::Recoverable), Some(Fault::ResetRequired)] {
        let lab = Lab::new();
        lab.fault_after_poll.set(fault);
        if fault.is_none() {
            *lab.polled_frame.borrow_mut() = Some(vec![0, 0x99]);
        }
        let mut control = LoRaControl::new();
        let (_, target) = control.split();
        let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<Radio>::interface_id(&profile),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
        let mut queue = [0; LORA_TX_QUEUE_BYTES];
        let interface = LoRaInterface::new(LoRaInterfaceInput {
            radio: Radio(lab.clone()),
            configuration: LoRaConfigurationState::Configured(LoRaConfiguration::manual(profile)),
            airtime_policy: AirtimePolicy::Regional,
            tx_queue: &mut queue,
            control: target,
            status: &status,
            spectrum: &spectrum,
            lifecycle: lifecycle.dyn_sender(),
        })
        .unwrap();
        let seam = Seam {
            lab: lab.clone(),
            current: Vec::new(),
            sink: Vec::new(),
        };
        let scenario = async {
            lab.outbound.send(vec![0x42; 10]).await;
            while lab.dispositions.borrow().is_empty() {
                yield_now().await;
            }
            assert_eq!(&*lab.dispositions.borrow(), &[OutboundDisposition::Sent]);
            assert!(lab.fault.get().is_none());
            assert!(lab.fault_after_poll.get().is_none());
            assert_eq!(
                spectrum.snapshot().radio_recoveries,
                u32::from(fault == Some(Fault::ResetRequired))
            );
            assert!(spectrum.snapshot().deferrals > 0);
            if fault.is_none() {
                assert_eq!(&*lab.delivered.borrow(), &[vec![0x99]]);
            }
        };
        block_on(async {
            match select(
                interface.run(seam),
                embassy_time::with_timeout(Duration::from_secs(5), scenario),
            )
            .await
            {
                Either::First(()) => panic!("radio task ended"),
                Either::Second(result) => {
                    result.unwrap();
                }
            }
        });
    }
}

#[test]
fn framing_and_radio_failures_advance_sequence_without_claiming_transmitted_bytes() {
    let lab = Lab::new();
    let mut radio = Radio(lab.clone());
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    let status = EmbassyInterfaceStatus::new_accounted(
        LoRaInterface::<Radio>::interface_id(&profile),
        ConnectionState::Connected,
    );
    let mut airtime = AirtimeLedger::new();
    let mut throughput = ThroughputLedger::new();
    let mut frame = [0; LORA_SINGLE_FRAME_MAX];
    let mut sequence = 0xf0;
    let started = Instant::now();
    block_on(async {
        radio.initialize_band(profile).await.unwrap();
        assert_eq!(
            transmit_packet(
                &mut radio,
                &[0; LORA_MAX_PAYLOAD + 1],
                &mut sequence,
                &mut airtime,
                &mut throughput,
                &status,
                &profile,
                &started,
                &EmbassyClock,
                &mut frame
            )
            .await,
            Err(LoRaTransmitError::Framing(AirFrameError::PayloadExceedsMax))
        );
        assert_eq!(sequence, 0);
        assert_eq!(status.tx_bytes(), 0);
        lab.fault
            .set(Some((Operation::Transmit, Fault::Recoverable)));
        assert_eq!(
            transmit_packet(
                &mut radio,
                &[1],
                &mut sequence,
                &mut airtime,
                &mut throughput,
                &status,
                &profile,
                &started,
                &EmbassyClock,
                &mut frame
            )
            .await,
            Err(LoRaTransmitError::Radio(Fault::Recoverable))
        );
        assert_eq!(sequence, 0x10);
        assert_eq!(status.tx_bytes(), 0);
        assert!(lab.transmissions.borrow().is_empty());
    });
    let mut storage = [0; 1024];
    let mut backlog = TransmitBacklog::new(&mut storage);
    assert!(backlog.can_accept_outbound());
    assert_eq!(
        backlog.accept(&[0; LORA_MAX_PAYLOAD + 1], &profile, 0),
        Err(TransmitQueueError::PacketTooLarge)
    );
    assert!(!backlog.has_pending());
    let mut continuation = true;
    assert_eq!(
        take_contention_priority(&mut continuation, &mut airtime, InstantMillis(0)),
        ContentionPriority::Continuation
    );
    assert!(!continuation);
}

#[test]
fn failed_reinitialization_keeps_observed_hardware_failed_until_full_recovery() {
    for operation in [Operation::Initialize, Operation::ArmRx] {
        let lab = Lab::new();
        let mut radio = Radio(lab.clone());
        let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<Radio>::interface_id(&profile),
            ConnectionState::Connected,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let mut radio_state = LoRaRadioState::Receiving;
        lab.fault.set(Some((operation, Fault::ResetRequired)));
        let result = block_on(reinit_radio(&mut radio, &profile, &spectrum));
        assert_eq!(result, RadioReinitialization::Failed);
        record_reinitialization(result, &status, &mut radio_state);
        assert_eq!(radio_state, LoRaRadioState::Unknown);
        assert_eq!(status.observed_connection(), ConnectionState::Failed);
        assert_eq!(spectrum.snapshot().radio_recoveries, 0);
        let result = block_on(reinit_radio(&mut radio, &profile, &spectrum));
        assert_eq!(result, RadioReinitialization::Recovered);
        record_reinitialization(result, &status, &mut radio_state);
        assert_eq!(radio_state, LoRaRadioState::Receiving);
        assert_eq!(status.observed_connection(), ConnectionState::Connected);
        assert_eq!(spectrum.snapshot().radio_recoveries, 1);
    }
}

#[test]
fn over_capacity_received_packet_is_accounted_without_delivery() {
    let lab = Lab::new();
    let mut seam = Seam {
        lab: lab.clone(),
        current: Vec::new(),
        sink: Vec::new(),
    };
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    let status = EmbassyInterfaceStatus::new_accounted(
        LoRaInterface::<Radio>::interface_id(&profile),
        ConnectionState::Connected,
    );
    let mut throughput = ThroughputLedger::new();
    let mut reassembler = LoRaReassembler::new();
    block_on(deliver_rx(
        ObservedAirFrame {
            bytes: &[0; LORA_MAX_PAYLOAD + 2],
            spreading_factor: SpreadingFactor::Sf7,
            phy: PacketPhyStats::default(),
            arrived_at: InstantMillis(0),
        },
        &status,
        &mut throughput,
        &mut reassembler,
        &mut seam,
    ));
    assert_eq!(status.frame_accounting().unwrap().undecodable, 1);
    assert!(lab.delivered.borrow().is_empty());
}
