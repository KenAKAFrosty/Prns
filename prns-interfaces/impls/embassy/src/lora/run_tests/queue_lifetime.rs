use super::*;

std::thread_local! { static OBSERVED: RefCell<Vec<LoRaQueueDrop>> = const { RefCell::new(Vec::new()) }; }
fn observe(event: LoRaQueueDrop) {
    OBSERVED.with_borrow_mut(|events| events.push(event));
}

struct Clock {
    lab: Rc<Lab>,
    busy: Cell<bool>,
}
impl RadioClock for &Clock {
    fn now(&self) -> Instant {
        self.lab.virtual_clock.set(true);
        Instant::from_millis(self.lab.clock_ms.get())
    }
    async fn wait(&self, duration: Duration) {
        yield_now().await;
        self.lab
            .clock_ms
            .set(self.lab.clock_ms.get().saturating_add(duration.as_millis()));
        if self.busy.get() {
            let _ = self
                .lab
                .events
                .try_send((RadioEvent::PreambleDetected, Vec::new()));
        }
    }
}

#[test]
fn delayed_lane_frames_fail_closed_before_any_radio_transmission() {
    for enqueued_at in [None, Some(InstantMillis(0)), Some(InstantMillis(500_000))] {
        let lab = Lab::new();
        lab.clock_ms.set(200_000);
        let clock = Clock {
            lab: lab.clone(),
            busy: Cell::new(false),
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
            lab.outbound.send((enqueued_at, vec![0xA5; 508])).await;
            while lab.dispositions.borrow().is_empty() {
                yield_now().await;
            }
            let expected = match enqueued_at {
                None => OutboundQueueDropReason::MissingTime,
                Some(InstantMillis(0)) => OutboundQueueDropReason::Expired,
                _ => OutboundQueueDropReason::ClockRollback,
            };
            assert_eq!(
                *lab.dispositions.borrow(),
                [OutboundDisposition::Dropped(
                    OutboundDropReason::QueueLifetime(expected)
                )]
            );
            assert!(lab.transmissions.borrow().is_empty());
            assert_eq!(lab.accepted.get(), 0);
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
fn old_request_cannot_emerge_after_off_on_even_when_off_is_not_polled() {
    for fast_toggle in [false, true] {
        let lab = Lab::new();
        lab.clock_ms.set(1_000);
        let clock = Clock {
            lab: lab.clone(),
            busy: Cell::new(true),
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
            lab.send_outbound(vec![0xA1; 16]).await;
            lab.send_outbound(vec![0xB2; 16]).await;
            while lab.accepted.get() < 2 {
                yield_now().await;
            }
            assert!(lab.transmissions.borrow().is_empty());
            status.disable();
            if fast_toggle {
                status.enable();
            }
            while lab.dispositions.borrow().len() < 2 {
                yield_now().await;
            }
            assert!(lab
                .dispositions
                .borrow()
                .iter()
                .all(|d| *d == OutboundDisposition::Dropped(OutboundDropReason::Disabled)));
            if !fast_toggle {
                lab.send_outbound(vec![0xC3; 16]).await;
                while lab.dispositions.borrow().len() < 3 {
                    yield_now().await;
                }
                status.enable();
            }
            let before = lab.dispositions.borrow().len();
            // A lane frame may be offered after resume with its old enqueue age.
            lab.outbound
                .send((Some(InstantMillis(1_000)), vec![0xD4; 16]))
                .await;
            while lab.dispositions.borrow().len() == before {
                yield_now().await;
            }
            assert_eq!(
                lab.dispositions.borrow().last(),
                Some(&OutboundDisposition::Dropped(
                    OutboundDropReason::QueueLifetime(OutboundQueueDropReason::PreResume)
                ))
            );
            assert!(lab.transmissions.borrow().is_empty());
            lab.clock_ms.set(lab.clock_ms.get() + 1);
            clock.busy.set(false);
            lab.send_outbound(vec![0xF5; 16]).await;
            while lab.transmissions.borrow().is_empty() {
                yield_now().await;
            }
            let transmissions = lab.transmissions.borrow();
            assert_eq!(transmissions.len(), 1);
            assert_eq!(
                &transmissions[0].1[prns_core::interfaces::lora::LORA_HEADER_LEN..],
                &[0xF5; 16]
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

#[test]
fn total_age_expires_full_fifo_active_and_waiting_lane_without_restarting_age() {
    OBSERVED.with_borrow_mut(Vec::clear);
    let lab = Lab::new();
    lab.clock_ms.set(1_000);
    let clock = Clock {
        lab: lab.clone(),
        busy: Cell::new(true),
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
    .unwrap()
    .with_outbound_queue_policy(OutboundQueuePolicy::new(20_000).unwrap())
    .with_queue_observer(observe);
    let seam = Seam {
        lab: lab.clone(),
        current: Vec::new(),
        sink: Vec::new(),
    };
    let scenario = async {
        // Active + eleven maximum-size FIFO entries fill the 6 KiB budget; the
        // thirteenth frame waits in the runtime-side test lane.
        for value in 0..13 {
            lab.send_outbound(vec![value; 508]).await;
        }
        while lab.accepted.get() < 12 {
            yield_now().await;
        }
        lab.clock_ms.set(lab.clock_ms.get() + 20_001);
        while lab.dispositions.borrow().len() < 13 {
            yield_now().await;
        }
        assert!(lab.transmissions.borrow().is_empty());
        assert_eq!(lab.accepted.get(), 12);
        assert_eq!(
            *lab.dispositions.borrow(),
            vec![
                OutboundDisposition::Dropped(OutboundDropReason::QueueLifetime(
                    OutboundQueueDropReason::Expired
                ));
                13
            ]
        );
        OBSERVED.with_borrow(|events| {
            assert_eq!(events.len(), 13);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.source == LoRaQueueSource::Active)
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.source == LoRaQueueSource::DriverQueue)
                    .count(),
                11
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.source == LoRaQueueSource::RuntimeLane)
                    .count(),
                1
            );
            assert!(events
                .iter()
                .all(|e| e.age_millis.is_some_and(|age| age >= 20_000)));
        });
        clock.busy.set(false);
        lab.send_outbound(vec![0xF6; 16]).await;
        while lab.transmissions.borrow().is_empty() {
            yield_now().await;
        }
        assert_eq!(
            &lab.transmissions.borrow()[0].1[prns_core::interfaces::lora::LORA_HEADER_LEN..],
            &[0xF6; 16]
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

#[test]
fn clock_rollback_below_task_start_discards_active_and_queued_frames() {
    let lab = Lab::new();
    lab.clock_ms.set(1_000);
    let clock = Clock {
        lab: lab.clone(),
        busy: Cell::new(true),
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
        lab.send_outbound(vec![0xA1; 508]).await;
        lab.send_outbound(vec![0xB2; 508]).await;
        while lab.accepted.get() < 2 {
            yield_now().await;
        }
        lab.clock_ms.set(0);
        while lab.dispositions.borrow().len() < 2 {
            yield_now().await;
        }
        assert_eq!(
            *lab.dispositions.borrow(),
            vec![
                OutboundDisposition::Dropped(OutboundDropReason::QueueLifetime(
                    OutboundQueueDropReason::ClockRollback
                ));
                2
            ]
        );
        assert!(lab.transmissions.borrow().is_empty());
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
