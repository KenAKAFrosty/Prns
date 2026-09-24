use super::*;

#[derive(Clone, Copy)]
enum StagedConfiguration {
    Confirmed {
        configuration: LoRaRuntimeConfiguration,
        activation: RadioActivation,
    },
    Unknown,
}

enum StageError<E> {
    Profile(LoRaConfigError),
    Initialize(crate::radios::BandRadioError<E>),
    Idle(E),
}

impl<E: core::fmt::Debug> core::fmt::Debug for StageError<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Profile(error) => formatter.debug_tuple("Profile").field(error).finish(),
            Self::Initialize(error) => formatter.debug_tuple("Initialize").field(error).finish(),
            Self::Idle(error) => formatter.debug_tuple("Idle").field(error).finish(),
        }
    }
}

async fn stage<R: LoRaRadio>(
    radio: &mut R,
    requested: LoRaConfigurationState,
    airtime_policy: AirtimePolicy,
    activation: RadioActivation,
) -> Result<LoRaRuntimeConfiguration, StageError<R::Error>> {
    let configuration = match requested {
        LoRaConfigurationState::Unconfigured => LoRaRuntimeConfiguration::Unconfigured,
        LoRaConfigurationState::Configured(configuration) => {
            let profile = configuration.profile();
            let duty = validate_profile_request(radio, profile, airtime_policy)
                .map_err(StageError::Profile)?;
            if matches!(activation, RadioActivation::Active) {
                radio
                    .initialize_band(profile)
                    .await
                    .map_err(StageError::Initialize)?;
            }
            LoRaRuntimeConfiguration::Configured { profile, duty }
        }
    };
    radio.idle().await.map_err(StageError::Idle)?;
    Ok(configuration)
}

pub(super) async fn publish<'a>(
    control: &LoRaControlTarget<'a>,
    lifecycle: DynamicSender<'_, InterfaceLifecycle<'a>>,
    old_id: InterfaceId,
    descriptor: InterfaceDescriptor,
) -> InterfacePublicationOutcome {
    let completion = control.publication();
    completion.reset();
    lifecycle
        .send(InterfaceLifecycle::Publish {
            old_id,
            descriptor,
            completion,
        })
        .await;
    completion.wait().await
}

#[expect(
    clippy::too_many_arguments,
    reason = "the radio task owns the complete publication boundary"
)]
pub(super) async fn transact<'a, R: LoRaRadio, S: InterfaceSeam>(
    radio: &mut R,
    control: &LoRaControlTarget<'a>,
    quiesce_id: u64,
    previous: &LoRaRuntimeConfiguration,
    airtime_policy: AirtimePolicy,
    current_id: &mut InterfaceId,
    status: &EmbassyInterfaceStatus,
    lifecycle: DynamicSender<'_, InterfaceLifecycle<'a>>,
    radio_state: &mut LoRaRadioState,
    backlog: &mut TransmitBacklog<'_>,
    seam: &mut S,
) -> LoRaRuntimeConfiguration {
    status.set_connection(ConnectionState::Initializing);
    let mut staged = match radio.idle().await {
        Ok(()) => {
            *radio_state = LoRaRadioState::Idle;
            control.complete(quiesce_id, LoRaApplyOutcome::Applied);
            StagedConfiguration::Confirmed {
                configuration: *previous,
                activation: RadioActivation::Inactive,
            }
        }
        Err(_) => {
            *radio_state = LoRaRadioState::Unknown;
            status.set_connection(ConnectionState::Failed);
            control.complete(quiesce_id, LoRaApplyOutcome::Rejected);
            StagedConfiguration::Unknown
        }
    };
    loop {
        let request = control.wait().await;
        let outcome = match request.command {
            LoRaConfigurationCommand::Stage(requested) => {
                let activation = if status.is_enabled() {
                    RadioActivation::Active
                } else {
                    RadioActivation::Inactive
                };
                match stage(radio, requested, airtime_policy, activation).await {
                    Ok(configuration) => {
                        staged = StagedConfiguration::Confirmed {
                            configuration,
                            activation,
                        };
                        *radio_state = LoRaRadioState::Idle;
                        LoRaApplyOutcome::Applied
                    }
                    Err(error) => {
                        crate::diagnostic_log::warn!("RNS_LORA staging failed: {error:?}");
                        staged = StagedConfiguration::Unknown;
                        *radio_state = LoRaRadioState::Unknown;
                        status.set_connection(ConnectionState::Failed);
                        LoRaApplyOutcome::Rejected
                    }
                }
            }
            LoRaConfigurationCommand::Publish | LoRaConfigurationCommand::Resume => {
                let StagedConfiguration::Confirmed {
                    configuration,
                    activation,
                } = staged
                else {
                    control.complete(request.id, LoRaApplyOutcome::Rejected);
                    continue;
                };
                if matches!(request.command, LoRaConfigurationCommand::Resume)
                    && configuration != *previous
                {
                    control.complete(request.id, LoRaApplyOutcome::Rejected);
                    continue;
                }
                let connection = match (status.is_enabled(), configuration) {
                    (true, LoRaRuntimeConfiguration::Configured { profile, .. }) => {
                        let activated = async {
                            if activation == RadioActivation::Inactive {
                                radio.initialize_band(profile).await?;
                            }
                            radio
                                .arm_rx()
                                .await
                                .map_err(crate::radios::BandRadioError::Radio)
                        }
                        .await;
                        if activated.is_err() {
                            *radio_state = LoRaRadioState::Unknown;
                            staged = StagedConfiguration::Unknown;
                            status.set_connection(ConnectionState::Failed);
                            control.complete(request.id, LoRaApplyOutcome::Rejected);
                            continue;
                        }
                        *radio_state = LoRaRadioState::Receiving;
                        ConnectionState::Connected
                    }
                    (_, LoRaRuntimeConfiguration::Unconfigured)
                    | (false, LoRaRuntimeConfiguration::Configured { .. }) => {
                        ConnectionState::Disabled
                    }
                };
                let (new_id, descriptor) = match configuration {
                    LoRaRuntimeConfiguration::Unconfigured => {
                        let id = unconfigured_interface_id();
                        (id, lora::unconfigured_descriptor(id))
                    }
                    LoRaRuntimeConfiguration::Configured { profile, duty } => {
                        let id = LoRaInterface::<R>::interface_id(&profile);
                        (id, lora::descriptor(id, &profile, duty))
                    }
                };
                if publish(control, lifecycle, *current_id, descriptor).await
                    != InterfacePublicationOutcome::Published
                {
                    status.set_connection(ConnectionState::Failed);
                    control.complete(request.id, LoRaApplyOutcome::Rejected);
                    continue;
                }
                if new_id != *current_id {
                    for _ in 0..backlog.clear() {
                        seam.complete_outbound(OutboundDisposition::Dropped(
                            OutboundDropReason::ChannelChanged,
                        ));
                    }
                }
                seam.set_channel_id(new_id);
                *current_id = new_id;
                status.set_id(new_id);
                status.set_connection(connection);
                control.complete(request.id, LoRaApplyOutcome::Applied);
                return configuration;
            }
            LoRaConfigurationCommand::Hold => {
                staged = StagedConfiguration::Unknown;
                *radio_state = if radio.idle().await.is_ok() {
                    LoRaRadioState::Idle
                } else {
                    LoRaRadioState::Unknown
                };
                status.set_connection(ConnectionState::Failed);
                LoRaApplyOutcome::Applied
            }
            LoRaConfigurationCommand::Apply(_)
            | LoRaConfigurationCommand::Clear
            | LoRaConfigurationCommand::Quiesce => LoRaApplyOutcome::Rejected,
        };
        control.complete(request.id, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lora::tests::{RadioCalls, RecordingRadio, RecordingRadioError};
    use embassy_futures::join::join3;
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
    use prns_core::interfaces::lora::LoRaConfiguration;
    use prns_core::interfaces::InterfaceStatus;
    use std::rc::Rc;

    fn block_on<F: core::future::Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let mut context = core::task::Context::from_waker(std::task::Waker::noop());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "transaction test failed to settle"
            );
            if let core::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::yield_now();
        }
    }

    fn configured() -> LoRaConfigurationState {
        LoRaConfigurationState::Configured(LoRaConfiguration::manual(
            prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE.into(),
        ))
    }
    struct QuietSeam;
    impl InterfaceSeam for QuietSeam {
        fn fill_random(&mut self, _: &mut [u8]) {
            panic!("quiesced radio requested entropy")
        }
        async fn inbound_sink(&mut self) -> &mut dyn prns_core::interfaces::FrameSink {
            panic!("quiesced radio delivered input")
        }
        async fn commit_inbound(&mut self) {
            panic!("quiesced radio committed input")
        }
        async fn next_outbound(&mut self) -> &[u8] {
            panic!("quiesced radio requested traffic")
        }
    }
    #[derive(Clone, Copy)]
    enum Fault {
        Quiesce,
        Initialize,
        StageIdle,
        Activation,
        Receive,
        HoldIdle,
    }

    #[test]
    fn every_hardware_failure_keeps_traffic_paused_until_explicit_restoration() {
        for fault in [
            Fault::Quiesce,
            Fault::Initialize,
            Fault::StageIdle,
            Fault::Activation,
            Fault::Receive,
            Fault::HoldIdle,
        ] {
            let calls = Rc::new(RadioCalls::default());
            let mut radio = RecordingRadio {
                calls: calls.clone(),
            };
            let mut control = LoRaControl::new();
            let (mut controller, target) = control.split();
            let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
            let mut id = unconfigured_interface_id();
            let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Disabled);
            let mut state = LoRaRadioState::Idle;
            let mut bytes = [0; 1024];
            let mut backlog = TransmitBacklog::new(&mut bytes);
            let mut seam = QuietSeam;
            if matches!(fault, Fault::Quiesce) {
                calls.idle_failures.set(1);
            }
            let (settled, (), ()) = block_on(join3(
                async {
                    let request = target.wait().await;
                    transact(
                        &mut radio,
                        &target,
                        request.id,
                        &LoRaRuntimeConfiguration::Unconfigured,
                        AirtimePolicy::Regional,
                        &mut id,
                        &status,
                        lifecycle.dyn_sender(),
                        &mut state,
                        &mut backlog,
                        &mut seam,
                    )
                    .await
                },
                async {
                    let outcome = controller.quiesce().await;
                    if matches!(fault, Fault::Quiesce) {
                        assert_eq!(outcome, LoRaApplyOutcome::Rejected);
                    } else {
                        assert_eq!(outcome, LoRaApplyOutcome::Applied);
                        match fault {
                            Fault::Initialize => calls.initialize_failures.set(1),
                            Fault::StageIdle => calls.idle_failures.set(1),
                            Fault::Activation => status.disable(),
                            Fault::Receive => calls.arm_rx_failures.set(1),
                            Fault::HoldIdle => calls.idle_failures.set(1),
                            Fault::Quiesce => unreachable!(),
                        }
                        if matches!(fault, Fault::HoldIdle) {
                            assert_eq!(
                                controller.hold_configuration().await,
                                LoRaApplyOutcome::Applied
                            );
                        } else {
                            let staged = controller.stage_configuration(configured()).await;
                            if matches!(fault, Fault::Initialize | Fault::StageIdle) {
                                assert_eq!(staged, LoRaApplyOutcome::Rejected);
                            } else {
                                assert_eq!(staged, LoRaApplyOutcome::Applied);
                                if matches!(fault, Fault::Activation) {
                                    calls.initialize_failures.set(1);
                                    status.enable();
                                }
                                assert_eq!(
                                    controller.publish_configuration().await,
                                    LoRaApplyOutcome::Rejected
                                );
                            }
                        }
                    }
                    assert_eq!(
                        controller.publish_configuration().await,
                        LoRaApplyOutcome::Rejected
                    );
                    assert_eq!(status.connection(), ConnectionState::Failed);
                    assert_eq!(controller.apply(prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE).await, LoRaApplyOutcome::Rejected);
                    assert_eq!(controller.clear().await, LoRaApplyOutcome::Rejected);
                    assert_eq!(controller.quiesce().await, LoRaApplyOutcome::Rejected);
                    assert_eq!(
                        controller.hold_configuration().await,
                        LoRaApplyOutcome::Applied
                    );
                    assert_eq!(
                        controller
                            .stage_configuration(LoRaConfigurationState::Unconfigured)
                            .await,
                        LoRaApplyOutcome::Applied
                    );
                    assert_eq!(
                        controller.resume_configuration().await,
                        LoRaApplyOutcome::Applied
                    );
                },
                async {
                    match lifecycle.receive().await {
                        InterfaceLifecycle::Publish {
                            completion,
                            descriptor,
                            ..
                        } => {
                            assert_eq!(descriptor.id, unconfigured_interface_id());
                            completion.signal(InterfacePublicationOutcome::Published);
                        }
                        _ => panic!("expected restoration publication"),
                    }
                },
            ));
            assert_eq!(settled, LoRaRuntimeConfiguration::Unconfigured);
            assert_eq!(state, LoRaRadioState::Idle);
            assert_eq!(calls.transmit.get(), 0);
            assert_eq!(calls.read_event.get(), 0);
        }
    }

    #[test]
    fn profile_errors_are_rejected_before_hardware_access_and_keep_typed_diagnostics() {
        let calls = Rc::new(RadioCalls::default());
        let mut radio = RecordingRadio {
            calls: calls.clone(),
        };
        let policy = AirtimePolicy::Fixed(Some(prns_core::interfaces::AirtimeDutyCycle {
            limit_short_per_mille: Some(0),
            limit_long_per_mille: None,
            max_queued_airtime_ms: 100,
        }));
        let error = block_on(stage(
            &mut radio,
            configured(),
            policy,
            RadioActivation::Active,
        ))
        .unwrap_err();
        assert!(matches!(error, StageError::Profile(_)));
        assert!(std::format!("{error:?}").starts_with("Profile("));
        assert_eq!(
            std::format!(
                "{:?}",
                StageError::Initialize(crate::radios::BandRadioError::Radio(RecordingRadioError))
            ),
            "Initialize(Radio(RecordingRadioError))"
        );
        assert_eq!(
            std::format!("{:?}", StageError::Idle(RecordingRadioError)),
            "Idle(RecordingRadioError)"
        );
        assert_eq!(calls.initialize.get(), 0);
        assert_eq!(calls.idle.get(), 0);
    }
}
