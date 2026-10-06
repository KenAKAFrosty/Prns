use super::*;

struct VirtualClock {
    lab: Rc<Lab>,
    now: Cell<Instant>,
    medium: Medium,
}

enum Medium {
    Clear,
    Occupied(Rc<Lab>),
    Interruptions {
        remaining: RefCell<std::collections::VecDeque<u64>>,
        fault: Option<Fault>,
    },
}

impl RadioClock for &VirtualClock {
    fn now(&self) -> Instant {
        self.now.get()
    }

    async fn wait(&self, duration: Duration) {
        let deadline = self.now.get() + duration;
        yield_now().await;
        self.now.set(deadline);
        self.lab.clock_ms.set(deadline.as_millis());
        if let Medium::Interruptions { remaining, fault } = &self.medium {
            let due = remaining
                .borrow()
                .front()
                .is_some_and(|time| *time <= deadline.as_millis());
            if due {
                remaining.borrow_mut().pop_front();
                if let Some(fault) = fault {
                    self.lab.fault.set(Some((Operation::Read, *fault)));
                }
                self.lab
                    .events
                    .try_send((RadioEvent::HeaderError, Vec::new()))
                    .unwrap();
            }
        }
        if let Medium::Occupied(lab) = &self.medium {
            lab.events
                .try_send((RadioEvent::PreambleDetected, Vec::new()))
                .unwrap();
        }
    }
}

#[test]
fn contention_and_duty_deadlines_drop_once_and_release_the_next_packet() {
    for occupied in [false, true] {
        let lab = Lab::new();
        let clock = VirtualClock {
            lab: lab.clone(),
            now: Cell::new(Instant::from_ticks(0)),
            medium: if occupied {
                Medium::Occupied(lab.clone())
            } else {
                Medium::Clear
            },
        };
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<Radio>::interface_id(&profile),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
        let mut queue = [0; LORA_TX_QUEUE_BYTES];
        let policy = if occupied {
            AirtimePolicy::Regional
        } else {
            AirtimePolicy::Fixed(Some(AirtimeDutyCycle {
                limit_short_per_mille: Some(1),
                limit_long_per_mille: Some(1),
                max_queued_airtime_ms: 2_000,
            }))
        };
        let interface = LoRaInterface::new(LoRaInterfaceInput {
            radio: Radio(lab.clone()),
            configuration: LoRaConfigurationState::Configured(LoRaConfiguration::manual(profile)),
            airtime_policy: policy,
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
            lab.outbound.send(vec![0x41; LORA_MAX_PAYLOAD]).await;
            lab.outbound.send(vec![0x42; LORA_MAX_PAYLOAD]).await;
            if occupied {
                while lab.accepted.get() < 2 || clock.now.get().as_millis() < 10 {
                    yield_now().await;
                }
                clock.now.set(Instant::from_millis(31_000));
                assert_eq!(
                    controller.publish_configuration().await,
                    LoRaApplyOutcome::Rejected
                );
            }
            while lab.dispositions.borrow().len() < 2 {
                yield_now().await;
            }
            let reason = if occupied {
                OutboundDropReason::ContentionTimeout
            } else {
                OutboundDropReason::DutyLimited
            };
            assert_eq!(
                &*lab.dispositions.borrow(),
                &[
                    OutboundDisposition::Dropped(reason.clone()),
                    OutboundDisposition::Dropped(reason)
                ]
            );
            assert_eq!(lab.accepted.get(), 2);
            assert!(lab.transmissions.borrow().is_empty());
            assert!(clock.now.get().as_millis() >= 60_000);
            let snapshot = spectrum.snapshot();
            if occupied {
                assert_eq!(snapshot.contention_timeouts, 2);
                assert_eq!(snapshot.duty_holds, 0);
            } else {
                assert_eq!(snapshot.contention_timeouts, 0);
                assert_eq!(snapshot.duty_holds, 2);
            }
        };
        block_on(async {
            match select(
                interface.run_with_clock(seam, &clock),
                embassy_time::with_timeout(Duration::from_secs(2), scenario),
            )
            .await
            {
                Either::First(()) => panic!("radio task ended"),
                Either::Second(result) => result.unwrap(),
            }
        });
    }
}

#[test]
fn saturated_fifo_yields_at_quantum_and_duty_boundaries_without_splitting_packets() {
    for duty_limited in [false, true] {
        let lab = Lab::new();
        let clock = VirtualClock {
            lab: lab.clone(),
            now: Cell::new(Instant::from_ticks(0)),
            medium: Medium::Clear,
        };
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
        let packet_us = packet_airtime(&[0; LORA_MAX_PAYLOAD], &profile);
        let policy = if duty_limited {
            AirtimePolicy::Fixed(Some(AirtimeDutyCycle {
                limit_short_per_mille: Some(u16::try_from(packet_us.div_ceil(15_000)).unwrap()),
                limit_long_per_mille: None,
                max_queued_airtime_ms: 2_000,
            }))
        } else {
            AirtimePolicy::Regional
        };
        let interface = LoRaInterface::new(LoRaInterfaceInput {
            radio: Radio(lab.clone()),
            configuration: LoRaConfigurationState::Configured(LoRaConfiguration::manual(profile)),
            airtime_policy: policy,
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
            lab.outbound.send(vec![0; LORA_MAX_PAYLOAD + 1]).await;
            while lab.dispositions.borrow().is_empty() {
                yield_now().await;
            }
            for value in 0..14 {
                lab.outbound.send(vec![value; LORA_MAX_PAYLOAD]).await;
                if value == 0 {
                    lab.outbound.send(vec![0; LORA_MAX_PAYLOAD + 1]).await;
                }
            }
            while lab.dispositions.borrow().len() < 16 {
                yield_now().await;
            }
            let expected = core::iter::repeat_n(
                OutboundDisposition::Dropped(OutboundDropReason::Rejected),
                2,
            )
            .chain(core::iter::repeat_n(OutboundDisposition::Sent, 14))
            .collect::<Vec<_>>();
            assert_eq!(*lab.dispositions.borrow(), expected);
            assert_eq!(lab.accepted.get(), 14);
            let transmissions = lab.transmissions.borrow();
            assert_eq!(transmissions.len(), 28);
            for (value, frames) in transmissions.as_chunks::<2>().0.iter().enumerate() {
                for (_, bytes) in frames {
                    assert_eq!(bytes[0], ((value as u8) << 4) | 1);
                    assert!(bytes[1..].iter().all(|byte| *byte == value as u8));
                }
            }
            let times = lab.transmission_times.borrow();
            let packets_per_quantum = usize::try_from(
                airtime_quantum::AirtimeQuantum::for_profile(profile).us() / packet_us,
            )
            .unwrap();
            if duty_limited {
                assert!(times[2] >= 15_000 + 2 * ChannelTiming::for_profile(profile).slot_ms());
            } else {
                assert!(times[..2 * packets_per_quantum]
                    .iter()
                    .all(|time| *time == times[0]));
                assert!(times[2 * packets_per_quantum] > times[0]);
            }
            let snapshot = spectrum.snapshot();
            assert_eq!(snapshot.contention_timeouts, 0);
            assert_eq!(snapshot.duty_timeouts, 0);
            if duty_limited {
                assert!(snapshot.duty_holds >= 13);
                assert!(clock.now.get().as_millis() >= 13 * 15_000);
            } else {
                assert_eq!(snapshot.duty_holds, 0);
                assert!(clock.now.get().as_millis() < 1_000);
            }
        };
        block_on(async {
            match select(
                interface.run_with_clock(seam, &clock),
                embassy_time::with_timeout(Duration::from_secs(2), scenario),
            )
            .await
            {
                Either::First(()) => panic!("radio task ended"),
                Either::Second(result) => result.unwrap(),
            }
        });
    }
}

#[test]
fn interrupted_contention_restarts_difs_and_counts_each_deferral_once() {
    for fault in [None, Some(Fault::Recoverable)] {
        let lab = Lab::new();
        let clock = VirtualClock {
            lab: lab.clone(),
            now: Cell::new(Instant::from_ticks(0)),
            medium: Medium::Interruptions {
                remaining: RefCell::new([99, 108].into()),
                fault,
            },
        };
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
            lab.outbound.send(vec![1; 10]).await;
            while lab.dispositions.borrow().is_empty() {
                yield_now().await;
            }
            assert_eq!(*lab.dispositions.borrow(), [OutboundDisposition::Sent]);
            assert_eq!(
                spectrum.snapshot().deferrals,
                2,
                "fault {fault:?}; tx {:?}",
                lab.transmission_times.borrow()
            );
            assert!(
                lab.transmission_times.borrow()[0] >= 120,
                "tx {:?}",
                lab.transmission_times.borrow()
            );
        };
        block_on(async {
            match select(
                interface.run_with_clock(seam, &clock),
                embassy_time::with_timeout(Duration::from_secs(2), scenario),
            )
            .await
            {
                Either::First(()) => panic!("radio task ended"),
                Either::Second(result) => result.unwrap(),
            }
        });
    }
}
