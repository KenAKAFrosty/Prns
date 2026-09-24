use super::*;

impl<R: LoRaRadio> LoRaInterface<'_, '_, R> {
    pub(super) async fn run_with_clock<Seam: InterfaceSeam, C: RadioClock>(
        self,
        mut seam: Seam,
        clock: C,
    ) {
        let LoRaInterface {
            id,
            mut radio,
            mut configuration,
            airtime_policy,
            tag: _,
            tx_queue,
            control,
            status,
            spectrum,
            lifecycle,
        } = self;
        let mut current_id = id;
        let started = clock.now();
        let mut radio_state = LoRaRadioState::Unknown;
        let mut backlog = TransmitBacklog::new(tx_queue);
        let mut rx_buf = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut tx_frame = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut seq: u8 = 0;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        seam.set_channel_id(current_id);

        'configuration: loop {
            let (mut profile, mut duty_cycle) = match configuration {
                LoRaRuntimeConfiguration::Configured { profile, duty } => (profile, duty),
                LoRaRuntimeConfiguration::Unconfigured => {
                    if !matches!(radio_state, LoRaRadioState::Idle) {
                        if let Err(error) = radio.idle().await {
                            crate::diagnostic_log::warn!(
                                "RNS_LORA unconfigured idle failed: {error:?}"
                            );
                            radio_state = LoRaRadioState::Unknown;
                            status.set_connection(ConnectionState::Failed);
                        } else {
                            radio_state = LoRaRadioState::Idle;
                        }
                    }
                    loop {
                        status.set_connection(match radio_state {
                            LoRaRadioState::Idle => ConnectionState::Disabled,
                            LoRaRadioState::Unknown => ConnectionState::Failed,
                            LoRaRadioState::Receiving => ConnectionState::Disconnected,
                        });
                        match select3(control.wait(), seam.next_outbound(), async {
                            if matches!(radio_state, LoRaRadioState::Unknown) {
                                clock.wait(IDLE_TICK).await;
                            } else {
                                core::future::pending::<()>().await;
                            }
                        })
                        .await
                        {
                            Either3::First(request) => match request.command {
                                LoRaConfigurationCommand::Quiesce => {
                                    configuration = configuration::transact(
                                        &mut radio,
                                        &control,
                                        request.id,
                                        &LoRaRuntimeConfiguration::Unconfigured,
                                        airtime_policy,
                                        &mut current_id,
                                        status,
                                        lifecycle,
                                        &mut radio_state,
                                        &mut backlog,
                                        &mut seam,
                                    )
                                    .await;
                                    continue 'configuration;
                                }
                                LoRaConfigurationCommand::Stage(_)
                                | LoRaConfigurationCommand::Publish
                                | LoRaConfigurationCommand::Resume
                                | LoRaConfigurationCommand::Hold => {
                                    control.complete(request.id, LoRaApplyOutcome::Rejected)
                                }
                                LoRaConfigurationCommand::Apply(requested) => {
                                    let requested_duty = match validate_profile_request(
                                        &radio,
                                        requested,
                                        airtime_policy,
                                    ) {
                                        Ok(duty) => duty,
                                        Err(error) => {
                                            crate::diagnostic_log::warn!(
                                                "RNS_LORA rejected profile: {error:?}"
                                            );
                                            control
                                                .complete(request.id, LoRaApplyOutcome::Rejected);
                                            continue;
                                        }
                                    };
                                    if status.is_enabled() {
                                        if let Err(error) = radio.initialize_band(requested).await {
                                            radio_state = LoRaRadioState::Unknown;
                                            crate::diagnostic_log::warn!(
                                                "RNS_LORA configuration init failed: {error:?}"
                                            );
                                            control
                                                .complete(request.id, LoRaApplyOutcome::Rejected);
                                            continue;
                                        }
                                        if let Err(error) = radio.arm_rx().await {
                                            crate::diagnostic_log::warn!(
                                                "RNS_LORA configuration RX arm failed: {error:?}"
                                            );
                                            radio_state = if radio.idle().await.is_ok() {
                                                LoRaRadioState::Idle
                                            } else {
                                                LoRaRadioState::Unknown
                                            };
                                            control
                                                .complete(request.id, LoRaApplyOutcome::Rejected);
                                            continue;
                                        }
                                        radio_state = LoRaRadioState::Receiving;
                                    }
                                    let new_id = Self::interface_id(&requested);
                                    if configuration::publish(
                                        &control,
                                        lifecycle,
                                        current_id,
                                        lora::descriptor(new_id, &requested, requested_duty),
                                    )
                                    .await
                                        != InterfacePublicationOutcome::Published
                                    {
                                        radio_state = if radio.idle().await.is_ok() {
                                            LoRaRadioState::Idle
                                        } else {
                                            LoRaRadioState::Unknown
                                        };
                                        status.set_connection(ConnectionState::Failed);
                                        control.complete(request.id, LoRaApplyOutcome::Rejected);
                                        continue;
                                    }
                                    current_id = new_id;
                                    status.set_id(new_id);
                                    if matches!(radio_state, LoRaRadioState::Receiving) {
                                        status.set_connection(ConnectionState::Connected);
                                    }
                                    seam.set_channel_id(current_id);
                                    control.complete(request.id, LoRaApplyOutcome::Applied);
                                    break (requested, requested_duty);
                                }
                                LoRaConfigurationCommand::Clear => {
                                    control.complete(request.id, LoRaApplyOutcome::Applied);
                                }
                            },
                            Either3::Second(_) => {
                                seam.complete_outbound(OutboundDisposition::Dropped(
                                    OutboundDropReason::NotConfigured,
                                ));
                            }
                            Either3::Third(()) => {
                                if let Err(error) = radio.idle().await {
                                    crate::diagnostic_log::warn!(
                                        "RNS_LORA unconfigured idle retry failed: {error:?}"
                                    );
                                } else {
                                    radio_state = LoRaRadioState::Idle;
                                }
                            }
                        }
                    }
                }
            };

            let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();
            let mut noise = NoiseFloor::new();
            let mut activity = DemodulatorActivity::new();
            let mut service_age = ServiceAge::new(profile);
            let mut continuation = false;
            backlog.active.recompute_airtime(&profile);
            let now = InstantMillis(clock.now().duration_since(started).as_millis());
            let priority = take_contention_priority(&mut continuation, &mut airtime, now);
            let mut access = backlog.active.channel_access(profile, now.0, priority);
            let mut access_suspended = true;
            let mut duty_was_held = false;
            let mut reported_deferrals = 0u32;
            if matches!(radio_state, LoRaRadioState::Receiving) {
                status.set_connection(ConnectionState::Connected);
            } else if status.is_enabled() {
                status.set_connection(ConnectionState::Initializing);
            } else {
                status.set_connection(ConnectionState::Disabled);
            }

            'serving: loop {
                let channel_change_drops = seam.take_channel_change_drops();
                if channel_change_drops != 0 {
                    spectrum
                        .channel_change_drops
                        .fetch_add(channel_change_drops, Ordering::Relaxed);
                }
                if status.is_enabled() && !matches!(radio_state, LoRaRadioState::Receiving) {
                    let readiness = match radio.initialize_band(profile).await {
                        Err(error) => {
                            radio_state = LoRaRadioState::Unknown;
                            Err(error)
                        }
                        Ok(()) => match radio.arm_rx().await {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                radio_state = if radio.idle().await.is_ok() {
                                    LoRaRadioState::Idle
                                } else {
                                    LoRaRadioState::Unknown
                                };
                                Err(crate::radios::BandRadioError::Radio(error))
                            }
                        },
                    };
                    if let Err(error) = readiness {
                        crate::diagnostic_log::warn!("RNS_LORA enable failed: {error:?}");
                        status.set_connection(ConnectionState::Failed);
                        match select3(
                            control.wait(),
                            status.wait_until_disabled(),
                            clock.wait(IDLE_TICK),
                        )
                        .await
                        {
                            Either3::First(request)
                                if matches!(request.command, LoRaConfigurationCommand::Quiesce) =>
                            {
                                reset_reassembly(&mut reassembler, status);
                                configuration = configuration::transact(
                                    &mut radio,
                                    &control,
                                    request.id,
                                    &LoRaRuntimeConfiguration::Configured {
                                        profile,
                                        duty: duty_cycle,
                                    },
                                    airtime_policy,
                                    &mut current_id,
                                    status,
                                    lifecycle,
                                    &mut radio_state,
                                    &mut backlog,
                                    &mut seam,
                                )
                                .await;
                                continue 'configuration;
                            }
                            Either3::First(request) => {
                                control.complete(request.id, LoRaApplyOutcome::Rejected)
                            }
                            Either3::Second(()) | Either3::Third(()) => {}
                        }
                        continue;
                    }
                    radio_state = LoRaRadioState::Receiving;
                    status.set_connection(ConnectionState::Connected);
                }
                if !status.is_enabled() {
                    if !matches!(radio_state, LoRaRadioState::Idle) {
                        if let Err(error) = radio.idle().await {
                            crate::diagnostic_log::warn!("RNS_LORA disable idle failed: {error:?}");
                            radio_state = LoRaRadioState::Unknown;
                            status.set_connection(ConnectionState::Failed);
                        } else {
                            radio_state = LoRaRadioState::Idle;
                        }
                    }
                    status.set_connection(if matches!(radio_state, LoRaRadioState::Idle) {
                        ConnectionState::Disabled
                    } else {
                        ConnectionState::Failed
                    });
                    reset_reassembly(&mut reassembler, status);
                    activity.frame_finished();
                    noise = NoiseFloor::new();
                    service_age.reset(profile);
                    continuation = false;
                    if backlog.active.clear() {
                        access = None;
                        seam.complete_outbound(OutboundDisposition::Dropped(
                            OutboundDropReason::Disabled,
                        ));
                    }
                    loop {
                        match select3(status.wait_until_enabled(), control.wait(), async {
                            if matches!(radio_state, LoRaRadioState::Unknown) {
                                clock.wait(IDLE_TICK).await;
                            } else {
                                core::future::pending::<()>().await;
                            }
                        })
                        .await
                        {
                            Either3::First(()) => continue 'serving,
                            Either3::Second(request) => {
                                if matches!(request.command, LoRaConfigurationCommand::Quiesce) {
                                    reset_reassembly(&mut reassembler, status);
                                    configuration = configuration::transact(
                                        &mut radio,
                                        &control,
                                        request.id,
                                        &LoRaRuntimeConfiguration::Configured {
                                            profile,
                                            duty: duty_cycle,
                                        },
                                        airtime_policy,
                                        &mut current_id,
                                        status,
                                        lifecycle,
                                        &mut radio_state,
                                        &mut backlog,
                                        &mut seam,
                                    )
                                    .await;
                                    continue 'configuration;
                                }

                                let previous_id = current_id;
                                let outcome = match request.command {
                                    LoRaConfigurationCommand::Quiesce
                                    | LoRaConfigurationCommand::Stage(_)
                                    | LoRaConfigurationCommand::Publish
                                    | LoRaConfigurationCommand::Resume
                                    | LoRaConfigurationCommand::Hold => LoRaApplyOutcome::Rejected,
                                    LoRaConfigurationCommand::Apply(requested) => {
                                        apply_profile(
                                            &mut radio,
                                            requested,
                                            airtime_policy,
                                            &mut profile,
                                            &mut duty_cycle,
                                            &mut current_id,
                                            status,
                                            spectrum,
                                            lifecycle,
                                            &control,
                                            RadioActivation::Inactive,
                                            &mut radio_state,
                                        )
                                        .await
                                    }
                                    LoRaConfigurationCommand::Clear => {
                                        let outcome = clear_configuration(
                                            &mut radio,
                                            &mut current_id,
                                            status,
                                            lifecycle,
                                            &control,
                                            &mut backlog,
                                            &mut seam,
                                            &mut radio_state,
                                        )
                                        .await;
                                        if matches!(outcome, LoRaApplyOutcome::Applied) {
                                            if previous_id != current_id {
                                                seam.set_channel_id(current_id);
                                            }
                                            control.complete(request.id, LoRaApplyOutcome::Applied);
                                            reset_reassembly(&mut reassembler, status);
                                            configuration = LoRaRuntimeConfiguration::Unconfigured;
                                            continue 'configuration;
                                        }
                                        outcome
                                    }
                                };
                                if outcome == LoRaApplyOutcome::Applied && previous_id != current_id
                                {
                                    for _ in 0..backlog.clear() {
                                        seam.complete_outbound(OutboundDisposition::Dropped(
                                            OutboundDropReason::ChannelChanged,
                                        ));
                                    }
                                    seam.set_channel_id(current_id);
                                }
                                control.complete(request.id, outcome);
                            }
                            Either3::Third(()) => {
                                if let Err(error) = radio.idle().await {
                                    crate::diagnostic_log::warn!(
                                        "RNS_LORA disabled idle retry failed: {error:?}"
                                    );
                                    status.set_connection(ConnectionState::Failed);
                                } else {
                                    radio_state = LoRaRadioState::Idle;
                                    status.set_connection(ConnectionState::Disabled);
                                }
                            }
                        }
                    }
                }

                let activation_time =
                    InstantMillis(clock.now().duration_since(started).as_millis());
                if backlog.activate_next(&profile, activation_time.0) {
                    let priority =
                        take_contention_priority(&mut continuation, &mut airtime, activation_time);
                    access = backlog
                        .active
                        .channel_access(profile, activation_time.0, priority);
                    access_suspended = true;
                    duty_was_held = false;
                    reported_deferrals = 0;
                }

                if backlog.active.len.is_some() {
                    let before_wait =
                        InstantMillis(clock.now().duration_since(started).as_millis());
                    let projected =
                        airtime.projected_utilization(before_wait, backlog.active.airtime_us);
                    let duty_permits = duty_cycle.is_none_or(|duty| duty.permits(projected));
                    if !duty_permits && !duty_was_held {
                        spectrum.add_duty_hold();
                    }
                    duty_was_held = !duty_permits;
                    let suspended_now = !duty_permits;
                    if access_suspended && !suspended_now {
                        if let Some(access) = access.as_mut() {
                            access.restart_contention(before_wait.0);
                        }
                    }
                    access_suspended = suspended_now;

                    let expired = access
                        .as_ref()
                        .is_some_and(|access| access.is_expired(before_wait.0));
                    if expired {
                        let reason = if duty_permits {
                            spectrum.add_contention_timeout();
                            OutboundDropReason::ContentionTimeout
                        } else {
                            spectrum.add_duty_timeout();
                            OutboundDropReason::DutyLimited
                        };
                        backlog.active.clear();
                        access = None;
                        seam.complete_outbound(OutboundDisposition::Dropped(reason));
                        if backlog.queue.is_empty() {
                            service_age.consume();
                            continuation = false;
                        }
                        continue;
                    }

                    let ordinary_tick_ms = if access_suspended {
                        IDLE_TICK.as_millis()
                    } else {
                        access.as_ref().map_or(IDLE_TICK.as_millis(), |access| {
                            access.next_poll_ms(service_age.backoff_rate())
                        })
                    };
                    let tick_ms = activity.next_poll_ms(before_wait.0, ordinary_tick_ms);

                    let mut next_action = None;
                    let can_accept_outbound = backlog.can_accept_outbound();
                    match select5(
                        control.wait(),
                        status.wait_until_disabled(),
                        radio.read_event(&mut rx_buf),
                        clock.wait(Duration::from_millis(tick_ms)),
                        async {
                            if can_accept_outbound {
                                return seam.next_outbound().await;
                            }
                            core::future::pending::<&[u8]>().await
                        },
                    )
                    .await
                    {
                        Either5::First(request) => {
                            if matches!(request.command, LoRaConfigurationCommand::Quiesce) {
                                reset_reassembly(&mut reassembler, status);
                                configuration = configuration::transact(
                                    &mut radio,
                                    &control,
                                    request.id,
                                    &LoRaRuntimeConfiguration::Configured {
                                        profile,
                                        duty: duty_cycle,
                                    },
                                    airtime_policy,
                                    &mut current_id,
                                    status,
                                    lifecycle,
                                    &mut radio_state,
                                    &mut backlog,
                                    &mut seam,
                                )
                                .await;
                                continue 'configuration;
                            }

                            let previous_id = current_id;
                            let outcome = match request.command {
                                LoRaConfigurationCommand::Quiesce
                                | LoRaConfigurationCommand::Stage(_)
                                | LoRaConfigurationCommand::Publish
                                | LoRaConfigurationCommand::Resume
                                | LoRaConfigurationCommand::Hold => LoRaApplyOutcome::Rejected,
                                LoRaConfigurationCommand::Apply(requested) => {
                                    apply_profile(
                                        &mut radio,
                                        requested,
                                        airtime_policy,
                                        &mut profile,
                                        &mut duty_cycle,
                                        &mut current_id,
                                        status,
                                        spectrum,
                                        lifecycle,
                                        &control,
                                        RadioActivation::Active,
                                        &mut radio_state,
                                    )
                                    .await
                                }
                                LoRaConfigurationCommand::Clear => {
                                    let outcome = clear_configuration(
                                        &mut radio,
                                        &mut current_id,
                                        status,
                                        lifecycle,
                                        &control,
                                        &mut backlog,
                                        &mut seam,
                                        &mut radio_state,
                                    )
                                    .await;
                                    if matches!(outcome, LoRaApplyOutcome::Applied) {
                                        if previous_id != current_id {
                                            seam.set_channel_id(current_id);
                                        }
                                        control.complete(request.id, LoRaApplyOutcome::Applied);
                                        reset_reassembly(&mut reassembler, status);
                                        configuration = LoRaRuntimeConfiguration::Unconfigured;
                                        continue 'configuration;
                                    }
                                    outcome
                                }
                            };
                            if matches!(outcome, LoRaApplyOutcome::Applied) {
                                if previous_id != current_id {
                                    for _ in 0..backlog.clear() {
                                        seam.complete_outbound(OutboundDisposition::Dropped(
                                            OutboundDropReason::ChannelChanged,
                                        ));
                                    }
                                    seam.set_channel_id(current_id);
                                }
                                let now =
                                    InstantMillis(clock.now().duration_since(started).as_millis());
                                reset_reassembly(&mut reassembler, status);
                                activity.frame_finished();
                                noise = NoiseFloor::new();
                                service_age.reset(profile);
                                continuation = false;
                                backlog.active.recompute_airtime(&profile);
                                let priority =
                                    take_contention_priority(&mut continuation, &mut airtime, now);
                                access = backlog.active.channel_access(profile, now.0, priority);
                                access_suspended = true;
                                duty_was_held = false;
                                reported_deferrals = 0;
                            }
                            control.complete(request.id, outcome);
                        }
                        Either5::Second(()) => continue,
                        Either5::Third(Ok(event)) => {
                            let now =
                                InstantMillis(clock.now().duration_since(started).as_millis());
                            let evidence = observe_radio_event(
                                event,
                                now,
                                ReceivePath {
                                    profile: &profile,
                                    activity: &mut activity,
                                    spectrum,
                                    rx_buf: &rx_buf,
                                    status,
                                    throughput: &mut throughput,
                                    reassembler: &mut reassembler,
                                    seam: &mut seam,
                                },
                            )
                            .await;
                            if backlog.has_pending() {
                                if let Some(decoded_airtime_us) = evidence.decoded_airtime_us {
                                    service_age.record_peer_airtime(decoded_airtime_us);
                                }
                            }
                            if !access_suspended {
                                if let Some(access) = access.as_mut() {
                                    let action = access.observe(
                                        now.0,
                                        evidence.observation,
                                        service_age.backoff_rate(),
                                    );
                                    next_action = Some(
                                        if matches!(action, ChannelAccessAction::NeedBackoffEntropy)
                                        {
                                            choose_backoff_entropy(access, &mut seam)
                                        } else {
                                            action
                                        },
                                    );
                                }
                            }
                        }
                        Either5::Third(Err(error)) => {
                            crate::diagnostic_log::debug!("RNS_LORA rx event error: {error:?}");
                            activity.frame_finished();
                            if matches!(R::recovery(&error), RadioRecovery::Reinitialize) {
                                record_reinitialization(
                                    reinit_radio(&mut radio, &profile, spectrum).await,
                                    status,
                                    &mut radio_state,
                                );
                                reset_reassembly(&mut reassembler, status);
                            }
                            if !access_suspended {
                                let now =
                                    InstantMillis(clock.now().duration_since(started).as_millis());
                                if let Some(access) = access.as_mut() {
                                    next_action = Some(access.observe(
                                        now.0,
                                        noise.fail_closed(),
                                        service_age.backoff_rate(),
                                    ));
                                }
                            }
                        }
                        Either5::Fourth(()) => {
                            let now =
                                InstantMillis(clock.now().duration_since(started).as_millis());
                            let observation = match sample_channel(
                                &mut radio,
                                now,
                                &mut activity,
                                spectrum,
                                &mut noise,
                            )
                            .await
                            {
                                Ok(observation) => observation,
                                Err(error) => {
                                    crate::diagnostic_log::debug!(
                                        "RNS_LORA channel sample failed: {error:?}"
                                    );
                                    activity.frame_finished();
                                    if matches!(R::recovery(&error), RadioRecovery::Reinitialize) {
                                        record_reinitialization(
                                            reinit_radio(&mut radio, &profile, spectrum).await,
                                            status,
                                            &mut radio_state,
                                        );
                                        reset_reassembly(&mut reassembler, status);
                                    }
                                    noise.fail_closed()
                                }
                            };
                            if !access_suspended {
                                if let Some(access) = access.as_mut() {
                                    let action = access.observe(
                                        now.0,
                                        observation,
                                        service_age.backoff_rate(),
                                    );
                                    next_action = Some(
                                        if matches!(action, ChannelAccessAction::NeedBackoffEntropy)
                                        {
                                            choose_backoff_entropy(access, &mut seam)
                                        } else {
                                            action
                                        },
                                    );
                                }
                            }
                        }
                        Either5::Fifth(outbound) => match backlog.queue.push(outbound) {
                            Ok(()) => seam.accept_outbound_custody(),
                            Err(TransmitQueueError::Full | TransmitQueueError::PacketTooLarge) => {
                                seam.complete_outbound(OutboundDisposition::Dropped(
                                    OutboundDropReason::Rejected,
                                ));
                            }
                        },
                    }

                    if matches!(next_action, Some(ChannelAccessAction::ReadyForFinalCheck)) {
                        let now = InstantMillis(clock.now().duration_since(started).as_millis());
                        let evidence = match radio.poll_event(&mut rx_buf).await {
                            Ok(Some(event)) => {
                                observe_radio_event(
                                    event,
                                    now,
                                    ReceivePath {
                                        profile: &profile,
                                        activity: &mut activity,
                                        spectrum,
                                        rx_buf: &rx_buf,
                                        status,
                                        throughput: &mut throughput,
                                        reassembler: &mut reassembler,
                                        seam: &mut seam,
                                    },
                                )
                                .await
                            }
                            Ok(None) => match sample_channel(
                                &mut radio,
                                now,
                                &mut activity,
                                spectrum,
                                &mut noise,
                            )
                            .await
                            {
                                Ok(observation) => ChannelEvidence {
                                    observation,
                                    decoded_airtime_us: None,
                                },
                                Err(error) => {
                                    crate::diagnostic_log::debug!(
                                        "RNS_LORA final channel check failed: {error:?}"
                                    );
                                    activity.frame_finished();
                                    if matches!(R::recovery(&error), RadioRecovery::Reinitialize) {
                                        record_reinitialization(
                                            reinit_radio(&mut radio, &profile, spectrum).await,
                                            status,
                                            &mut radio_state,
                                        );
                                        reset_reassembly(&mut reassembler, status);
                                    }
                                    ChannelEvidence {
                                        observation: noise.fail_closed(),
                                        decoded_airtime_us: None,
                                    }
                                }
                            },
                            Err(error) => {
                                crate::diagnostic_log::debug!(
                                    "RNS_LORA final IRQ check failed: {error:?}"
                                );
                                activity.frame_finished();
                                if matches!(R::recovery(&error), RadioRecovery::Reinitialize) {
                                    record_reinitialization(
                                        reinit_radio(&mut radio, &profile, spectrum).await,
                                        status,
                                        &mut radio_state,
                                    );
                                    reset_reassembly(&mut reassembler, status);
                                }
                                ChannelEvidence {
                                    observation: noise.fail_closed(),
                                    decoded_airtime_us: None,
                                }
                            }
                        };
                        if let Some(decoded_airtime_us) = evidence.decoded_airtime_us {
                            service_age.record_peer_airtime(decoded_airtime_us);
                        }
                        if let Some(access) = access.as_mut() {
                            next_action = Some(access.final_check(now.0, evidence.observation));
                        }
                    }

                    if let Some(access) = access.as_ref() {
                        let deferrals = access.deferrals();
                        if deferrals > reported_deferrals {
                            spectrum.add_deferrals(deferrals - reported_deferrals);
                            reported_deferrals = deferrals;
                        }
                    }

                    match next_action {
                        Some(ChannelAccessAction::Transmit) => {
                            service_age.consume();
                            let quantum = service_age.quantum();
                            let mut txop_airtime_us = 0u64;
                            let mut quantum_limited = false;
                            let mut transmission_failed = false;

                            while let Some(active_len) = backlog.active.len {
                                // Finish an in-flight logical packet, then yield the radio
                                // before starting another packet from this opportunity.
                                if control.has_pending() {
                                    break;
                                }
                                let packet_airtime_us = backlog.active.airtime_us;
                                if !quantum.permits(txop_airtime_us, packet_airtime_us) {
                                    quantum_limited = true;
                                    break;
                                }

                                let packet_start =
                                    InstantMillis(clock.now().duration_since(started).as_millis());
                                let projected =
                                    airtime.projected_utilization(packet_start, packet_airtime_us);
                                if duty_cycle.is_some_and(|duty| !duty.permits(projected)) {
                                    if !duty_was_held {
                                        spectrum.add_duty_hold();
                                    }
                                    duty_was_held = true;
                                    break;
                                }

                                let tx = transmit_packet(
                                    &mut radio,
                                    &backlog.active.bytes[..active_len],
                                    &mut seq,
                                    &mut airtime,
                                    &mut throughput,
                                    status,
                                    &profile,
                                    &started,
                                    &clock,
                                    &mut tx_frame,
                                )
                                .await;
                                let disposition = match tx {
                                    Ok(()) => {
                                        txop_airtime_us =
                                            txop_airtime_us.saturating_add(packet_airtime_us);
                                        OutboundDisposition::Sent
                                    }
                                    Err(LoRaTransmitError::Radio(error)) => {
                                        transmission_failed = true;
                                        if matches!(
                                            R::recovery(&error),
                                            RadioRecovery::Reinitialize
                                        ) {
                                            record_reinitialization(
                                                reinit_radio(&mut radio, &profile, spectrum).await,
                                                status,
                                                &mut radio_state,
                                            );
                                            reset_reassembly(&mut reassembler, status);
                                        }
                                        OutboundDisposition::Dropped(
                                            OutboundDropReason::TransportFailure,
                                        )
                                    }
                                    Err(LoRaTransmitError::Framing(_)) => {
                                        transmission_failed = true;
                                        OutboundDisposition::Dropped(
                                            OutboundDropReason::TransportFailure,
                                        )
                                    }
                                };
                                backlog.active.clear();
                                seam.complete_outbound(disposition);

                                if transmission_failed {
                                    break;
                                }
                                let activated_at =
                                    InstantMillis(clock.now().duration_since(started).as_millis());
                                if !backlog.activate_next(&profile, activated_at.0) {
                                    break;
                                }
                            }

                            if let Err(error) = radio.arm_rx().await {
                                crate::diagnostic_log::debug!(
                                    "RNS_LORA RX re-arm after tx failed: {error:?}"
                                );
                                radio_state = LoRaRadioState::Unknown;
                                status.set_connection(ConnectionState::Failed);
                                if matches!(R::recovery(&error), RadioRecovery::Reinitialize) {
                                    record_reinitialization(
                                        reinit_radio(&mut radio, &profile, spectrum).await,
                                        status,
                                        &mut radio_state,
                                    );
                                    reset_reassembly(&mut reassembler, status);
                                }
                            }
                            activity.frame_finished();
                            access = None;
                            access_suspended = true;
                            reported_deferrals = 0;

                            if backlog.active.len.is_none() {
                                let activated_at =
                                    InstantMillis(clock.now().duration_since(started).as_millis());
                                let _ = backlog.activate_next(&profile, activated_at.0);
                            }
                            if backlog.active.len.is_some() {
                                if quantum_limited {
                                    service_age.seed_continuation();
                                    continuation = true;
                                }
                                let now =
                                    InstantMillis(clock.now().duration_since(started).as_millis());
                                let priority =
                                    take_contention_priority(&mut continuation, &mut airtime, now);
                                access = backlog.active.channel_access(profile, now.0, priority);
                            } else {
                                service_age.consume();
                                continuation = false;
                                duty_was_held = false;
                            }
                        }
                        Some(ChannelAccessAction::Expired) => {
                            spectrum.add_contention_timeout();
                            backlog.active.clear();
                            access = None;
                            seam.complete_outbound(OutboundDisposition::Dropped(
                                OutboundDropReason::ContentionTimeout,
                            ));
                            if backlog.queue.is_empty() {
                                service_age.consume();
                                continuation = false;
                            }
                        }
                        Some(
                            ChannelAccessAction::Wait
                            | ChannelAccessAction::NeedBackoffEntropy
                            | ChannelAccessAction::ReadyForFinalCheck,
                        )
                        | None => {}
                    }
                } else {
                    let ordinary_idle_tick_ms = if noise.is_calibrated() {
                        IDLE_TICK.as_millis()
                    } else {
                        ChannelTiming::for_profile(profile).sample_ms()
                    };
                    let now_ms = clock.now().duration_since(started).as_millis();
                    let idle_tick =
                        Duration::from_millis(activity.next_poll_ms(now_ms, ordinary_idle_tick_ms));
                    match select4(
                        control.wait(),
                        status.wait_until_disabled(),
                        radio.read_event(&mut rx_buf),
                        select(seam.next_outbound(), clock.wait(idle_tick)),
                    )
                    .await
                    {
                        Either4::First(request) => {
                            if matches!(request.command, LoRaConfigurationCommand::Quiesce) {
                                reset_reassembly(&mut reassembler, status);
                                configuration = configuration::transact(
                                    &mut radio,
                                    &control,
                                    request.id,
                                    &LoRaRuntimeConfiguration::Configured {
                                        profile,
                                        duty: duty_cycle,
                                    },
                                    airtime_policy,
                                    &mut current_id,
                                    status,
                                    lifecycle,
                                    &mut radio_state,
                                    &mut backlog,
                                    &mut seam,
                                )
                                .await;
                                continue 'configuration;
                            }

                            let previous_id = current_id;
                            let outcome = match request.command {
                                LoRaConfigurationCommand::Quiesce
                                | LoRaConfigurationCommand::Stage(_)
                                | LoRaConfigurationCommand::Publish
                                | LoRaConfigurationCommand::Resume
                                | LoRaConfigurationCommand::Hold => LoRaApplyOutcome::Rejected,
                                LoRaConfigurationCommand::Apply(requested) => {
                                    apply_profile(
                                        &mut radio,
                                        requested,
                                        airtime_policy,
                                        &mut profile,
                                        &mut duty_cycle,
                                        &mut current_id,
                                        status,
                                        spectrum,
                                        lifecycle,
                                        &control,
                                        RadioActivation::Active,
                                        &mut radio_state,
                                    )
                                    .await
                                }
                                LoRaConfigurationCommand::Clear => {
                                    let outcome = clear_configuration(
                                        &mut radio,
                                        &mut current_id,
                                        status,
                                        lifecycle,
                                        &control,
                                        &mut backlog,
                                        &mut seam,
                                        &mut radio_state,
                                    )
                                    .await;
                                    if matches!(outcome, LoRaApplyOutcome::Applied) {
                                        if previous_id != current_id {
                                            seam.set_channel_id(current_id);
                                        }
                                        control.complete(request.id, LoRaApplyOutcome::Applied);
                                        reset_reassembly(&mut reassembler, status);
                                        configuration = LoRaRuntimeConfiguration::Unconfigured;
                                        continue 'configuration;
                                    }
                                    outcome
                                }
                            };
                            if matches!(outcome, LoRaApplyOutcome::Applied) {
                                if previous_id != current_id {
                                    for _ in 0..backlog.clear() {
                                        seam.complete_outbound(OutboundDisposition::Dropped(
                                            OutboundDropReason::ChannelChanged,
                                        ));
                                    }
                                    seam.set_channel_id(current_id);
                                }
                                reset_reassembly(&mut reassembler, status);
                                activity.frame_finished();
                                noise = NoiseFloor::new();
                                service_age.reset(profile);
                                continuation = false;
                            }
                            control.complete(request.id, outcome);
                        }
                        Either4::Second(()) => continue,
                        Either4::Third(Ok(event)) => {
                            let now =
                                InstantMillis(clock.now().duration_since(started).as_millis());
                            let _ = observe_radio_event(
                                event,
                                now,
                                ReceivePath {
                                    profile: &profile,
                                    activity: &mut activity,
                                    spectrum,
                                    rx_buf: &rx_buf,
                                    status,
                                    throughput: &mut throughput,
                                    reassembler: &mut reassembler,
                                    seam: &mut seam,
                                },
                            )
                            .await;
                        }
                        Either4::Third(Err(e)) => {
                            crate::diagnostic_log::debug!("RNS_LORA rx event error: {e:?}");
                            activity.frame_finished();
                            if matches!(R::recovery(&e), RadioRecovery::Reinitialize) {
                                record_reinitialization(
                                    reinit_radio(&mut radio, &profile, spectrum).await,
                                    status,
                                    &mut radio_state,
                                );
                                reset_reassembly(&mut reassembler, status);
                            }
                        }
                        Either4::Fourth(Either::First(outbound)) => {
                            let now =
                                InstantMillis(clock.now().duration_since(started).as_millis());
                            match backlog.accept(outbound, &profile, now.0) {
                                Ok(PacketPlacement::Active) => {
                                    seam.accept_outbound_custody();
                                    let priority = take_contention_priority(
                                        &mut continuation,
                                        &mut airtime,
                                        now,
                                    );
                                    access =
                                        backlog.active.channel_access(profile, now.0, priority);
                                    access_suspended = true;
                                    duty_was_held = false;
                                    reported_deferrals = 0;
                                }
                                Ok(PacketPlacement::Queued) => seam.accept_outbound_custody(),
                                Err(
                                    TransmitQueueError::Full | TransmitQueueError::PacketTooLarge,
                                ) => {
                                    seam.complete_outbound(OutboundDisposition::Dropped(
                                        OutboundDropReason::Rejected,
                                    ));
                                }
                            }
                        }
                        Either4::Fourth(Either::Second(())) => {
                            let now =
                                InstantMillis(clock.now().duration_since(started).as_millis());
                            if let Err(error) =
                                sample_channel(&mut radio, now, &mut activity, spectrum, &mut noise)
                                    .await
                            {
                                crate::diagnostic_log::debug!(
                                    "RNS_LORA idle channel sample failed: {error:?}"
                                );
                                activity.frame_finished();
                                if matches!(R::recovery(&error), RadioRecovery::Reinitialize) {
                                    record_reinitialization(
                                        reinit_radio(&mut radio, &profile, spectrum).await,
                                        status,
                                        &mut radio_state,
                                    );
                                    reset_reassembly(&mut reassembler, status);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
