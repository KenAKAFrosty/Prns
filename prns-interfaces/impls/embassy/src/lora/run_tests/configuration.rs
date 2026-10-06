use super::*;

fn reconfigure_busy_radio(new: RadioProfile, channel_changed: bool) {
    use core::{future::Future, task::Poll};
    let lab = Lab::new();
    lab.pause_tx.set(true);
    let mut control = LoRaControl::new();
    let (mut controller, target) = control.split();
    let old = RadioProfile::SubG(US915_AUTO_LORA_PROFILE);
    let status = EmbassyInterfaceStatus::new_accounted(
        LoRaInterface::<Radio>::interface_id(&old),
        ConnectionState::Initializing,
    );
    let spectrum = LoRaSpectrumStatus::new();
    let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
    let mut queue = [0; LORA_TX_QUEUE_BYTES];
    let interface = LoRaInterface::new(LoRaInterfaceInput {
        radio: Radio(lab.clone()),
        configuration: LoRaConfigurationState::Configured(LoRaConfiguration::manual(old)),
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
        while status.frame_accounting().unwrap().frames_in == 0 {
            yield_now().await;
        }
        lab.outbound.send(std::vec![0x71; 300]).await;
        lab.outbound.send(std::vec![0x72; 10]).await;
        lab.tx_started.wait().await;
        assert_eq!(lab.accepted.get(), 2);
        {
            let mut quiesce = core::pin::pin!(controller.quiesce());
            core::future::poll_fn(|cx| {
                assert_eq!(quiesce.as_mut().poll(cx), Poll::Pending);
                Poll::Ready(())
            })
            .await;
            assert_eq!(lab.transmissions.borrow().len(), 1);
            lab.tx_release.signal(());
            assert_eq!(quiesce.await, LoRaApplyOutcome::Applied);
        }
        assert_eq!(
            lab.transmissions.borrow().len(),
            2,
            "split packets finish as one logical transmission"
        );
        assert!(!lab.receiving.get());
        assert_eq!(status.frame_accounting().unwrap().undecodable, 1);
        assert_eq!(
            controller
                .stage_configuration(LoRaConfigurationState::Configured(
                    LoRaConfiguration::manual(new)
                ))
                .await,
            LoRaApplyOutcome::Applied
        );
        assert!(!lab.receiving.get());
        assert_eq!(lab.transmissions.borrow().len(), 2);
        assert_eq!(
            controller.publish_configuration().await,
            LoRaApplyOutcome::Applied
        );
        let expected_dispositions = if channel_changed {
            vec![
                OutboundDisposition::Sent,
                OutboundDisposition::Dropped(OutboundDropReason::ChannelChanged),
            ]
        } else {
            vec![OutboundDisposition::Sent]
        };
        assert_eq!(&*lab.dispositions.borrow(), &expected_dispositions);
        assert_eq!(status.id(), LoRaInterface::<Radio>::interface_id(&new));
        lab.receive(&[0x11, 0xcd]).await;
        while status.frame_accounting().unwrap().frames_in < 2 {
            yield_now().await;
        }
        assert!(
            lab.delivered.borrow().is_empty(),
            "the old partial frame cannot join a new-band fragment"
        );
        lab.outbound.send(std::vec![0x73; 10]).await;
        while lab.transmissions.borrow().len() < if channel_changed { 3 } else { 4 } {
            yield_now().await;
        }
        let transmissions = lab.transmissions.borrow();
        assert_eq!(transmissions[0].0, old);
        assert_eq!(transmissions[1].0, old);
        assert_eq!(transmissions[2].0, new);
        if channel_changed {
            assert_eq!(&transmissions[2].1[1..], &[0x73; 10]);
        } else {
            assert_eq!(&transmissions[2].1[1..], &[0x72; 10]);
            assert_eq!(transmissions[3].0, new);
            assert_eq!(&transmissions[3].1[1..], &[0x73; 10]);
        }
    };
    let publication = async {
        match lifecycle.receive().await {
            InterfaceLifecycle::Publish {
                completion,
                descriptor,
                ..
            } => {
                assert_eq!(descriptor.id, LoRaInterface::<Radio>::interface_id(&new));
                completion.signal(InterfacePublicationOutcome::Published);
            }
            _ => panic!("expected publication"),
        }
    };
    block_on(async {
        match select(
            interface.run(seam),
            embassy_time::with_timeout(Duration::from_secs(5), join(scenario, publication)),
        )
        .await
        {
            Either::First(()) => panic!("interface terminated"),
            Either::Second(result) => {
                result.unwrap();
            }
        }
    });
}

#[test]
fn in_flight_split_packet_finishes_before_band_change_and_old_backlog_and_partial_rx_are_discarded()
{
    reconfigure_busy_radio(RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE), true);
}

#[test]
fn local_power_and_preamble_changes_preserve_queued_packets() {
    use prns_core::interfaces::lora::{PreambleSymbols, TxPower};
    for profile in [
        US915_AUTO_LORA_PROFILE
            .with_tx_power(TxPower::new(10))
            .unwrap(),
        US915_AUTO_LORA_PROFILE
            .with_preamble(PreambleSymbols::new(24))
            .unwrap(),
    ] {
        reconfigure_busy_radio(RadioProfile::SubG(profile), false);
    }
}

#[test]
fn transactions_run_from_unconfigured_idle_and_disabled_states_and_clear_back_to_idle() {
    for configured in [false, true] {
        for enabled in [false, true] {
            let lab = Lab::new();
            let mut control = LoRaControl::new();
            let (mut controller, target) = control.split();
            let old = RadioProfile::SubG(US915_AUTO_LORA_PROFILE);
            let new = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
            let initial = if configured {
                LoRaConfigurationState::Configured(LoRaConfiguration::manual(old))
            } else {
                LoRaConfigurationState::Unconfigured
            };
            let id = if configured {
                LoRaInterface::<Radio>::interface_id(&old)
            } else {
                unconfigured_interface_id()
            };
            let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Initializing);
            if !enabled {
                status.disable();
            }
            let spectrum = LoRaSpectrumStatus::new();
            let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
            let mut queue = [0; LORA_TX_QUEUE_BYTES];
            let interface = LoRaInterface::new(LoRaInterfaceInput {
                radio: Radio(lab.clone()),
                configuration: initial,
                airtime_policy: AirtimePolicy::Regional,
                tx_queue: &mut queue,
                control: target,
                status: &status,
                spectrum: &spectrum,
                lifecycle: lifecycle.dyn_sender(),
            })
            .unwrap();
            assert_eq!(interface.descriptor().id, id);
            assert!(!interface.channel_tag().is_empty());
            let seam = Seam {
                lab: lab.clone(),
                current: Vec::new(),
                sink: Vec::new(),
            };
            let scenario = async {
                for requested in [
                    LoRaConfigurationState::Configured(LoRaConfiguration::manual(new)),
                    LoRaConfigurationState::Unconfigured,
                ] {
                    assert_eq!(controller.quiesce().await, LoRaApplyOutcome::Applied);
                    assert!(!lab.receiving.get());
                    assert_eq!(
                        controller.stage_configuration(requested).await,
                        LoRaApplyOutcome::Applied
                    );
                    assert!(!lab.receiving.get());
                    assert_eq!(
                        controller.publish_configuration().await,
                        LoRaApplyOutcome::Applied
                    );
                    let receiving =
                        enabled && matches!(requested, LoRaConfigurationState::Configured(_));
                    assert_eq!(lab.receiving.get(), receiving);
                    assert_eq!(
                        status.connection(),
                        if receiving {
                            ConnectionState::Connected
                        } else {
                            ConnectionState::Disabled
                        }
                    );
                    if !enabled {
                        assert_eq!(lab.initialized.get(), None);
                    }
                }
                assert_eq!(status.id(), unconfigured_interface_id());
                lab.outbound.send(vec![1, 2, 3]).await;
                while lab.dispositions.borrow().is_empty() {
                    yield_now().await;
                }
                assert_eq!(
                    &*lab.dispositions.borrow(),
                    &[OutboundDisposition::Dropped(
                        OutboundDropReason::NotConfigured
                    )]
                );
                assert!(lab.transmissions.borrow().is_empty());
                assert_eq!(
                    controller.publish_configuration().await,
                    LoRaApplyOutcome::Rejected
                );
                assert_eq!(controller.clear().await, LoRaApplyOutcome::Applied);
            };
            let publications = async {
                for _ in 0..2 {
                    match lifecycle.receive().await {
                        InterfaceLifecycle::Publish { completion, .. } => {
                            completion.signal(InterfacePublicationOutcome::Published)
                        }
                        _ => panic!("unexpected publication"),
                    }
                }
            };
            block_on(async {
                match select(
                    interface.run(seam),
                    embassy_time::with_timeout(
                        Duration::from_secs(3),
                        join(scenario, publications),
                    ),
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
fn legacy_commands_restore_the_old_channel_on_hardware_or_publication_failure() {
    for idle_failure in [false, true] {
        for failure in [Some(Operation::Initialize), Some(Operation::ArmRx), None] {
            for configured in [false, true] {
                let lab = Lab::new();
                let mut control = LoRaControl::new();
                let (mut controller, target) = control.split();
                let old = RadioProfile::SubG(US915_AUTO_LORA_PROFILE);
                let new = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
                let initial = if configured {
                    LoRaConfigurationState::Configured(LoRaConfiguration::manual(old))
                } else {
                    LoRaConfigurationState::Unconfigured
                };
                let id = if configured {
                    LoRaInterface::<Radio>::interface_id(&old)
                } else {
                    unconfigured_interface_id()
                };
                let status =
                    EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Initializing);
                let spectrum = LoRaSpectrumStatus::new();
                let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
                let mut queue = [0; LORA_TX_QUEUE_BYTES];
                let interface = LoRaInterface::new(LoRaInterfaceInput {
                    radio: Radio(lab.clone()),
                    configuration: initial,
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
                    while status.connection() == ConnectionState::Initializing {
                        yield_now().await;
                    }
                    if configured {
                        lab.outbound.send(vec![0x42; 10]).await;
                        while lab.accepted.get() < 1 {
                            yield_now().await;
                        }
                    } else if idle_failure && failure != Some(Operation::Initialize) {
                        lab.idle_failures.set(1);
                    }
                    if let Some(operation) = failure {
                        lab.fault.set(Some((operation, Fault::ResetRequired)));
                    }
                    assert_eq!(controller.apply(new).await, LoRaApplyOutcome::Rejected);
                    assert_eq!(status.id(), id);
                    if configured {
                        assert_eq!(lab.initialized.get(), Some(old));
                        assert!(lab.receiving.get());
                        assert_eq!(spectrum.snapshot().radio_recoveries, 1);
                        lab.fault.set(Some((Operation::Idle, Fault::Recoverable)));
                        assert_eq!(controller.clear().await, LoRaApplyOutcome::Rejected);
                        assert_eq!(status.id(), id);
                    } else if failure.is_none() {
                        assert_eq!(lab.receiving.get(), idle_failure);
                        assert_eq!(
                            status.observed_connection(),
                            if idle_failure {
                                ConnectionState::Failed
                            } else {
                                ConnectionState::Disabled
                            }
                        );
                    } else {
                        assert!(!lab.receiving.get());
                    }
                    assert!(lab.transmissions.borrow().is_empty());
                };
                let publication = async {
                    loop {
                        match lifecycle.receive().await {
                            InterfaceLifecycle::Publish { completion, .. } => {
                                completion.signal(InterfacePublicationOutcome::IdentityConflict)
                            }
                            _ => panic!("unexpected publication"),
                        }
                    }
                };
                block_on(async {
                    let run = async {
                        select(interface.run(seam), publication).await;
                    };
                    match select(
                        run,
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
}

#[test]
fn failed_startup_accepts_quiesce_and_clear_without_waiting_for_hardware_recovery() {
    for idle_failure in [false, true] {
        for failure in [Operation::Initialize, Operation::ArmRx] {
            let lab = Lab::new();
            lab.fault.set(Some((failure, Fault::ResetRequired)));
            if idle_failure && failure == Operation::ArmRx {
                lab.idle_failures.set(1);
            }
            let mut control = LoRaControl::new();
            let (mut controller, target) = control.split();
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
                while status.connection() != ConnectionState::Failed {
                    yield_now().await;
                }
                lab.fault
                    .set(Some((Operation::Initialize, Fault::ResetRequired)));
                assert_eq!(
                    controller.publish_configuration().await,
                    LoRaApplyOutcome::Rejected
                );
                // Keep the next initialization unavailable, proving that the control
                // request, rather than successful hardware retry, releases the task.
                lab.fault
                    .set(Some((Operation::Initialize, Fault::ResetRequired)));
                assert_eq!(controller.quiesce().await, LoRaApplyOutcome::Applied);
                assert_eq!(
                    controller
                        .stage_configuration(LoRaConfigurationState::Unconfigured)
                        .await,
                    LoRaApplyOutcome::Applied
                );
                assert_eq!(
                    controller.publish_configuration().await,
                    LoRaApplyOutcome::Applied
                );
                assert_eq!(status.connection(), ConnectionState::Disabled);
                assert!(!lab.receiving.get());
                assert_eq!(
                    lab.initializations.get(),
                    usize::from(failure == Operation::ArmRx)
                );
            };
            let publication = async {
                match lifecycle.receive().await {
                    InterfaceLifecycle::Publish { completion, .. } => {
                        completion.signal(InterfacePublicationOutcome::Published)
                    }
                    _ => panic!("unexpected publication"),
                }
            };
            block_on(async {
                match select(
                    interface.run(seam),
                    embassy_time::with_timeout(Duration::from_secs(3), join(scenario, publication)),
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
fn legacy_changes_publish_before_releasing_active_or_disabled_traffic() {
    for pending in [false, true] {
        for disabled in [false, true] {
            for request in [
                LoRaConfigurationState::Unconfigured,
                LoRaConfigurationState::Configured(LoRaConfiguration::manual(RadioProfile::Ghz24(
                    GHZ24_BALANCED_PROFILE,
                ))),
                LoRaConfigurationState::Configured(LoRaConfiguration::manual(RadioProfile::SubG(
                    US915_AUTO_LORA_PROFILE
                        .with_tx_power(prns_core::interfaces::lora::TxPower::new(10))
                        .unwrap(),
                ))),
            ] {
                let lab = Lab::new();
                let mut control = LoRaControl::new();
                let (mut controller, target) = control.split();
                let profile = RadioProfile::SubG(US915_AUTO_LORA_PROFILE);
                let old_id = LoRaInterface::<Radio>::interface_id(&profile);
                let status =
                    EmbassyInterfaceStatus::new_accounted(old_id, ConnectionState::Initializing);
                if disabled {
                    status.disable();
                }
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
                    while status.connection() == ConnectionState::Initializing {
                        yield_now().await;
                    }
                    // A same-profile request is idempotent and sends no publication.
                    assert_eq!(controller.apply(profile).await, LoRaApplyOutcome::Applied);
                    if pending && !disabled {
                        lab.outbound.send(vec![0x42; 10]).await;
                        while lab.accepted.get() < 1 {
                            yield_now().await;
                        }
                    }
                    assert_eq!(
                        controller.apply_configuration(request).await,
                        LoRaApplyOutcome::Applied
                    );
                    match request {
                        LoRaConfigurationState::Unconfigured => {
                            assert_eq!(status.id(), unconfigured_interface_id());
                            assert_eq!(status.connection(), ConnectionState::Disabled);
                            assert!(!lab.receiving.get());
                            if pending && !disabled {
                                assert_eq!(
                                    &*lab.dispositions.borrow(),
                                    &[OutboundDisposition::Dropped(
                                        OutboundDropReason::NotConfigured
                                    )]
                                );
                            }
                        }
                        LoRaConfigurationState::Configured(configuration) => {
                            let new_profile = configuration.profile();
                            let new_id = LoRaInterface::<Radio>::interface_id(&new_profile);
                            assert_eq!(status.id(), new_id);
                            if pending && !disabled {
                                if old_id != new_id {
                                    assert_eq!(
                                        &*lab.dispositions.borrow(),
                                        &[OutboundDisposition::Dropped(
                                            OutboundDropReason::ChannelChanged
                                        )]
                                    );
                                } else {
                                    while lab.transmissions.borrow().is_empty() {
                                        yield_now().await;
                                    }
                                    assert_eq!(
                                        lab.transmissions.borrow()[0],
                                        (
                                            new_profile,
                                            vec![
                                                0, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
                                                0x42, 0x42
                                            ]
                                        )
                                    );
                                }
                            }
                            assert_eq!(lab.receiving.get(), !disabled);
                        }
                    }
                    assert_eq!(lab.channel_ids.borrow().last(), Some(&status.id()));
                    if disabled {
                        assert_eq!(lab.initializations.get(), 0);
                    }
                };
                let publication = async {
                    match lifecycle.receive().await {
                        InterfaceLifecycle::Publish {
                            completion,
                            old_id: observed_old,
                            ..
                        } => {
                            assert_eq!(observed_old, old_id);
                            assert_eq!(status.id(), old_id);
                            assert!(lab.transmissions.borrow().is_empty());
                            completion.signal(InterfacePublicationOutcome::Published);
                        }
                        _ => panic!("unexpected publication"),
                    }
                };
                block_on(async {
                    match select(
                        interface.run(seam),
                        embassy_time::with_timeout(
                            Duration::from_secs(3),
                            join(scenario, publication),
                        ),
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
}

#[test]
fn disabling_retries_idle_and_enabling_resumes_the_retained_queue() {
    exercise_disabled_backlog(false);
    exercise_disabled_backlog(true);
}

fn exercise_disabled_backlog(change_channel: bool) {
    let lab = Lab::new();
    lab.seam_drops.set(3);
    let mut control = LoRaControl::new();
    let (mut controller, target) = control.split();
    let profile = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
    let id = LoRaInterface::<Radio>::interface_id(&profile);
    let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Initializing);
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
    let phase = Cell::new("accepting");
    let scenario = async {
        lab.outbound.send(vec![0x42; 10]).await;
        lab.outbound.send(vec![0x43; 10]).await;
        while lab.accepted.get() < 2 {
            yield_now().await;
        }
        lab.idle_failures.set(2);
        phase.set("disabling");
        status.disable();
        while status.observed_connection() != ConnectionState::Failed {
            yield_now().await;
        }
        phase.set("rejecting stray publish");
        assert_eq!(
            controller.publish_configuration().await,
            LoRaApplyOutcome::Rejected
        );
        phase.set("waiting idle retry");
        while status.observed_connection() != ConnectionState::Disabled {
            yield_now().await;
        }
        assert_eq!(
            &*lab.dispositions.borrow(),
            &[OutboundDisposition::Dropped(OutboundDropReason::Disabled)]
        );
        assert_eq!(spectrum.snapshot().channel_change_drops, 3);
        assert!(!lab.receiving.get());
        phase.set("rejecting clear while disabled");
        assert_eq!(controller.clear().await, LoRaApplyOutcome::Rejected);
        assert_eq!(status.id(), id);
        phase.set("updating power while disabled");
        let changed = if change_channel {
            RadioProfile::SubG(US915_AUTO_LORA_PROFILE)
        } else {
            RadioProfile::Ghz24(
                GHZ24_BALANCED_PROFILE
                    .with_tx_power(prns_core::interfaces::lora::TxPower::new(9))
                    .unwrap(),
            )
        };
        assert_eq!(controller.apply(changed).await, LoRaApplyOutcome::Applied);
        assert_eq!(
            lab.dispositions.borrow().len(),
            if change_channel { 2 } else { 1 }
        );
        if change_channel {
            assert_eq!(
                lab.dispositions.borrow()[1],
                OutboundDisposition::Dropped(OutboundDropReason::ChannelChanged)
            );
            lab.outbound.send(vec![0x44; 10]).await;
        }
        phase.set("enabling retained queue");
        status.enable();
        while lab.dispositions.borrow().len() < if change_channel { 3 } else { 2 } {
            yield_now().await;
        }
        assert_eq!(
            *lab.dispositions.borrow().last().unwrap(),
            OutboundDisposition::Sent
        );
        assert_eq!(
            &lab.transmissions.borrow()[0].1[1..],
            &[if change_channel { 0x44 } else { 0x43 }; 10]
        );
        assert!(lab.receiving.get());
        phase.set("rejecting clear publication");
        assert_eq!(controller.clear().await, LoRaApplyOutcome::Rejected);
        assert_eq!(status.id(), LoRaInterface::<Radio>::interface_id(&changed));
        phase.set("disabling empty radio");
        status.disable();
        while status.observed_connection() != ConnectionState::Disabled {
            yield_now().await;
        }
        assert!(!lab.receiving.get());
    };
    let publication = async {
        for outcome in [
            InterfacePublicationOutcome::UnknownInterface,
            InterfacePublicationOutcome::Published,
            InterfacePublicationOutcome::UnknownInterface,
        ] {
            match lifecycle.receive().await {
                InterfaceLifecycle::Publish { completion, .. } => completion.signal(outcome),
                _ => panic!("unexpected publication"),
            }
        }
    };
    block_on(async {
        match select(
            interface.run(seam),
            embassy_time::with_timeout(Duration::from_secs(3), join(scenario, publication)),
        )
        .await
        {
            Either::First(()) => panic!("radio task ended"),
            Either::Second(result) => {
                assert!(
                    result.is_ok(),
                    "{}: {:?}, accepted {}, dispositions {:?}, idle failures {}",
                    phase.get(),
                    status.observed_connection(),
                    lab.accepted.get(),
                    lab.dispositions.borrow(),
                    lab.idle_failures.get()
                );
            }
        }
    });
}

#[test]
fn incompatible_profile_requests_leave_the_operating_configuration_untouched() {
    for configured in [false, true] {
        for disabled in [false, true] {
            let lab = Lab::new();
            let mut control = LoRaControl::new();
            let (mut controller, target) = control.split();
            let old = RadioProfile::SubG(US915_AUTO_LORA_PROFILE);
            let new = RadioProfile::Ghz24(GHZ24_BALANCED_PROFILE);
            let initial = if configured {
                LoRaConfigurationState::Configured(LoRaConfiguration::manual(old))
            } else {
                LoRaConfigurationState::Unconfigured
            };
            let id = if configured {
                LoRaInterface::<Radio>::interface_id(&old)
            } else {
                unconfigured_interface_id()
            };
            let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Initializing);
            if disabled {
                status.disable();
            }
            let spectrum = LoRaSpectrumStatus::new();
            let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
            let mut queue = [0; LORA_TX_QUEUE_BYTES];
            let interface = LoRaInterface::new(LoRaInterfaceInput {
                radio: Radio(lab.clone()),
                configuration: initial,
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
                while status.observed_connection() == ConnectionState::Initializing {
                    yield_now().await;
                }
                let before = lab.initializations.get();
                lab.reject_profile.set(true);
                assert_eq!(controller.apply(new).await, LoRaApplyOutcome::Rejected);
                assert_eq!(lab.initializations.get(), before);
                assert_eq!(status.id(), id);
                assert_eq!(lab.receiving.get(), configured && !disabled);
                assert_eq!(
                    controller
                        .stage_configuration(LoRaConfigurationState::Unconfigured)
                        .await,
                    LoRaApplyOutcome::Rejected
                );
                assert_eq!(
                    controller.resume_configuration().await,
                    LoRaApplyOutcome::Rejected
                );
                assert_eq!(
                    controller.hold_configuration().await,
                    LoRaApplyOutcome::Rejected
                );
                assert!(lifecycle.try_receive().is_err());
                if configured && !disabled {
                    lab.outbound.send(vec![0x42; 10]).await;
                    while lab.accepted.get() < 1 {
                        yield_now().await;
                    }
                    assert_eq!(
                        controller.publish_configuration().await,
                        LoRaApplyOutcome::Rejected
                    );
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
fn unconfigured_idle_failure_remains_visible_until_a_retry_succeeds() {
    let lab = Lab::new();
    lab.idle_failures.set(2);
    let mut control = LoRaControl::new();
    let (_, target) = control.split();
    let id = unconfigured_interface_id();
    let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Initializing);
    let spectrum = LoRaSpectrumStatus::new();
    let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
    let mut queue = [0; LORA_TX_QUEUE_BYTES];
    let interface = LoRaInterface::new(LoRaInterfaceInput {
        radio: Radio(lab.clone()),
        configuration: LoRaConfigurationState::Unconfigured,
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
        while status.observed_connection() == ConnectionState::Initializing {
            yield_now().await;
        }
        assert_eq!(status.observed_connection(), ConnectionState::Failed);
        while status.observed_connection() != ConnectionState::Disabled {
            yield_now().await;
        }
        assert_eq!(lab.idle_failures.get(), 0);
        assert_eq!(lab.initializations.get(), 0);
        assert!(!lab.receiving.get());
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
