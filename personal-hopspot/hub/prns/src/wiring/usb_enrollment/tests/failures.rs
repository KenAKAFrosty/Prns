use super::super::events::{EnrollmentEventSink, enrollment_events};
use super::*;
use alloc::sync::Arc;
use personal_rns::engine::*;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentityPublicKeys, IdentitySigner};
use personal_rns::routing::links::LinkId;
use personal_rns::units::{InstantMillis, RttMillis};
use std::sync::Mutex;

const LINK: LinkId = LinkId::new([42; 16]);

#[derive(Default)]
struct Calls {
    began: alloc::vec::Vec<BeginRemoteControlControllerPairing>,
    approved: alloc::vec::Vec<ApproveRemoteControlControllerPairing>,
    closed: alloc::vec::Vec<LinkId>,
}

struct Backend {
    calls: Arc<Mutex<Calls>>,
    begin: Result<
        RemoteControlControllerPairingResponseReceived,
        BeginRemoteControlControllerPairingControlError,
    >,
    approve: Result<
        RemoteControlControllerPairingResponseReceived,
        ApproveRemoteControlControllerPairingControlError,
    >,
}

fn advanced() -> RemoteControlControllerPairingResponseReceived {
    RemoteControlControllerPairingResponseReceived {
        delivered: PacketReceiptDelivered {
            rtt: RttMillis::new(1),
            evidence: DeliveryEvidence::Response,
        },
        admission: AdmitRemoteControlControllerPairingResponseOutcome::NoActivePairing,
        effect: RemoteControlControllerPairingResponseEffect::Advanced,
    }
}

impl Backend {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Calls::default())),
            begin: Ok(advanced()),
            approve: Ok(advanced()),
        }
    }
}

impl RemoteControlControllerPairingInitiationTransport for Backend {
    async fn establish_remote_control_pairing_link(
        &self,
        _: personal_rns::wire::DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        Ok(LINK)
    }
    fn close_remote_control_pairing_link(
        &self,
        link: LinkId,
    ) -> RemoteControlPairingLinkCleanupOutcome {
        self.calls.lock().unwrap().closed.push(link);
        RemoteControlPairingLinkCleanupOutcome::Queued
    }
}

impl RemoteControlPairingControl for Backend {
    async fn begin_remote_control_controller_pairing(
        &self,
        command: BeginRemoteControlControllerPairing,
    ) -> Result<
        RemoteControlControllerPairingResponseReceived,
        BeginRemoteControlControllerPairingControlError,
    > {
        assert_eq!(
            command.admission,
            RemoteControlPairingAdmission::DirectPhysical
        );
        self.calls.lock().unwrap().began.push(command);
        self.begin
    }
    async fn approve_remote_control_controller_pairing(
        &self,
        command: ApproveRemoteControlControllerPairing,
    ) -> Result<
        RemoteControlControllerPairingResponseReceived,
        ApproveRemoteControlControllerPairingControlError,
    > {
        self.calls.lock().unwrap().approved.push(command);
        self.approve
    }
    async fn reject_remote_control_controller_pairing(
        &self,
        _: RejectRemoteControlControllerPairing,
    ) -> Result<
        RemoteControlControllerPairingRejection,
        RejectRemoteControlControllerPairingControlError,
    > {
        panic!("unexpected rejection")
    }
    async fn approve_remote_control_target_pairing(
        &self,
        _: ApproveRemoteControlTargetPairing,
    ) -> Result<RemoteControlTargetPairingApproval, ApproveRemoteControlTargetPairingControlError>
    {
        panic!("target command")
    }
    async fn reject_remote_control_target_pairing(
        &self,
        _: RejectRemoteControlTargetPairing,
    ) -> Result<RemoteControlTargetPairingRejection, RejectRemoteControlTargetPairingControlError>
    {
        panic!("target command")
    }
}

fn identity(seed: u8) -> IdentityPublicKeys {
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&[seed; 64]);
    IdentityPublicKeys {
        encryption: signer.encryption_public_key(),
        signing: signer.signing_public_key(),
    }
}

fn work(
    directory: &std::path::Path,
    kind: RemoteControlPairingAvailabilityKind,
    observed: u64,
    lifetime: u64,
) -> (UsbEnrollmentWork, EnrollmentEventSink) {
    let installation = ControllerInstallation::open(directory).unwrap();
    let candidate = crate::tests::usb_observation::with_observation_kind(
        1,
        crate::tests::usb_observation::USB,
        observed,
        lifetime,
        kind,
        |observation| PairingCandidate::from(&observation),
    );
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(4).unwrap()).unwrap();
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("USB").unwrap(),
        })
        .unwrap()
    else {
        panic!("capacity");
    };
    let (sink, events) = enrollment_events();
    (
        UsbEnrollmentWork {
            registry,
            store: installation.devices,
            events,
            candidate,
            device,
            controller: RemoteControlControllerIdentity::new(identity(2)),
        },
        sink,
    )
}

fn confirmation(work: &UsbEnrollmentWork) -> RemoteControlControllerPairingConfirmation {
    let mut controller = RemoteControlControllerPairingState::default();
    let context = RemoteControlPairingContext::new(work.candidate.endpoint(), LINK);
    let BeginRemoteControlControllerPairingOutcome::BeginOwed { begin, .. } = controller.begin(
        work.controller,
        context,
        RemoteControlPairingAdmission::DirectPhysical,
        InstantMillis(0),
        InstantMillis(30_000),
    ) else {
        panic!("begin");
    };
    let (offer, _) = RemoteControlPairingPreparedOffer::new(
        &InMemoryNodeIdentity::from_secret_key_bytes(&[3; 64]),
        context,
        &begin,
        RemoteControlPairingPermissions::new(
            RemoteControlControllerAuthority::Administrator,
            RemoteControlRequestSet::only(RemoteControlRequestKind::InventoryInterfaces),
        )
        .unwrap(),
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(10_000)).unwrap(),
    )
    .unwrap()
    .into_parts();
    assert!(matches!(
        controller.receive_offer(offer, InstantMillis(0)),
        ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired { .. }
    ));
    let RemoteControlControllerPairingView::AwaitingApproval(view) = controller.view() else {
        panic!("offer");
    };
    view.into()
}

fn send_confirmation(
    sink: &EnrollmentEventSink,
    confirmation: RemoteControlControllerPairingConfirmation,
) {
    assert!(
        sink.capture(PrnsEvent::Message(
            Message::RemoteControlControllerPairingConfirmationRequired(confirmation)
        ))
        .is_none()
    );
}

fn persisted(sink: &EnrollmentEventSink, attempt: RemoteControlPairingAttemptId) {
    assert!(
        sink.capture(PrnsEvent::Message(
            Message::RemoteControlControllerPairingAuthorizationPersisted {
                attempt_id: attempt
            }
        ))
        .is_none()
    );
}

#[test]
fn native_preparation_is_inert_and_event_overflow_or_shutdown_is_explicit() {
    within_runtime(async {
        let directory = tempfile::tempdir().unwrap();
        let prepared = prepare_native_usb_enrollment::<2>(
            ControllerInstallation::open(directory.path()).unwrap(),
            |_| panic!("not running"),
            core::future::pending(),
        );
        assert!(
            prepared
                .discovery
                .discovery
                .submit(crate::UsbDiscoveryIntent::Inspect)
                .unwrap()
                .snapshot
                .candidates
                .is_empty()
        );
        drop(prepared);
        let (sink, mut events) = enrollment_events();
        drop(sink);
        assert!(matches!(
            events.receive().await,
            Err(EnrollmentEventsFailure::NodeStopped)
        ));
        let (sink, mut events) = enrollment_events();
        for seed in 0..9 {
            persisted(
                &sink,
                RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([seed; 32]),
            );
        }
        assert!(matches!(
            events.receive().await,
            Err(EnrollmentEventsFailure::Overflow)
        ));
    });
}

#[test]
fn unavailable_candidates_and_failed_initiation_leave_the_registry_and_store_owned() {
    within_runtime(async {
        for mode in 0..3 {
            let directory = tempfile::tempdir().unwrap();
            let kind = if mode == 0 {
                RemoteControlPairingAvailabilityKind::PairingAvailable
            } else {
                RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable
            };
            let (mut work, _sink) = work(directory.path(), kind, 0, 30_000);
            let before = work.registry.step(ReadDevice {
                device: work.device,
            });
            let mut backend = Backend::new();
            backend.begin = Err(RemoteControlPairingControlError::Busy);
            let clock = TokioClock::start_at(InstantMillis(if mode == 1 { 30_000 } else { 0 }));
            let mut exit = work.run::<4>(backend, clock).await.unwrap();
            match mode {
                0 => assert_eq!(
                    exit.outcome.unwrap(),
                    UsbEnrollmentOutcome::InvitationRequired
                ),
                1 => assert_eq!(exit.outcome.unwrap(), UsbEnrollmentOutcome::Expired),
                _ => assert!(matches!(
                    exit.outcome,
                    Ok(UsbEnrollmentOutcome::InitiationFailed(_))
                )),
            }
            assert_eq!(
                exit.registry.step(ReadDevice {
                    device: match before {
                        ReadDeviceOutcome::Found { ref device } => device.id,
                        _ => panic!("missing"),
                    }
                }),
                before
            );
            assert!(exit.coordinator.is_none());
            assert!(matches!(
                exit.store.load(NonZeroU32::new(4).unwrap()).unwrap(),
                LoadDevicesOutcome::Missing
            ));
        }
    });
}

#[test]
fn offers_and_registry_refusals_close_the_attempt_without_granting_or_recording() {
    within_runtime(async {
        for mode in 0..4 {
            let directory = tempfile::tempdir().unwrap();
            let (mut work, sink) = work(
                directory.path(),
                RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable,
                if mode == 3 { 1_000 } else { 0 },
                30_000,
            );
            send_confirmation(&sink, confirmation(&work));
            let device = work.device;
            if mode == 0 {
                work.controller = RemoteControlControllerIdentity::new(identity(9));
            }
            if mode == 1 {
                let _forgotten = work.registry.step(ForgetDevice { device });
            }
            let mut backend = Backend::new();
            let calls = backend.calls.clone();
            if mode == 2 {
                backend.approve = Err(RemoteControlPairingControlError::NodeStopped);
            }
            let exit = work.run::<4>(backend, TokioClock::new()).await.unwrap();
            match mode {
                0 => assert_eq!(
                    exit.outcome.unwrap(),
                    UsbEnrollmentOutcome::OfferRefused(
                        ReviewUsbEnrollmentOfferOutcome::WrongController
                    )
                ),
                1 => assert!(matches!(
                    exit.outcome.unwrap(),
                    UsbEnrollmentOutcome::EnrollmentRefused(
                        BeginEnrollmentOutcome::MissingDevice { .. }
                    )
                )),
                2 => assert!(matches!(
                    exit.outcome,
                    Ok(UsbEnrollmentOutcome::ApprovalFailed(
                        RemoteControlPairingControlError::NodeStopped
                    ))
                )),
                _ => assert!(matches!(exit.outcome, Err(UsbEnrollmentFailure::Clock(_)))),
            }
            assert_eq!(calls.lock().unwrap().closed, [LINK]);
            assert_eq!(calls.lock().unwrap().approved.len(), usize::from(mode == 2));
            assert_eq!(
                exit.cleanup,
                Some(RemoteControlPairingLinkCleanupOutcome::Queued)
            );
        }
    });
}

#[test]
fn settlement_correlation_duplicate_confirmation_and_failed_persistence_preserve_retry() {
    within_runtime(async {
        for mode in 0..4 {
            let directory = tempfile::tempdir().unwrap();
            let (work, sink) = work(
                directory.path(),
                RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable,
                0,
                30_000,
            );
            let confirmed = confirmation(&work);
            let attempt = confirmed.confirmation().attempt_id();
            let unrelated =
                RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([9; 32]);
            persisted(&sink, unrelated);
            send_confirmation(&sink, confirmed);
            send_confirmation(&sink, confirmation(&work));
            persisted(&sink, unrelated);
            if mode == 0 {
                assert!(
                    sink.capture(PrnsEvent::Message(
                        Message::RemoteControlControllerPairingAuthorizationPersistenceFailed {
                            attempt_id: attempt
                        }
                    ))
                    .is_none()
                );
            } else {
                persisted(&sink, attempt);
            }
            if mode == 2 {
                std::fs::create_dir(directory.path().join("devices.hopspot")).unwrap();
            }
            let backend = Backend::new();
            let calls = backend.calls.clone();
            let mut exit = if mode == 3 {
                work.run::<0>(backend, TokioClock::new()).await.unwrap()
            } else {
                work.run::<4>(backend, TokioClock::new()).await.unwrap()
            };
            assert_eq!(
                calls.lock().unwrap().approved,
                [ApproveRemoteControlControllerPairing {
                    attempt_id: attempt
                }]
            );
            match mode {
                0 => assert!(matches!(
                    exit.outcome.unwrap(),
                    UsbEnrollmentOutcome::Settlement(EnrollmentCoordinatorOutcome::Failed {
                        reason: EnrollmentFailure::PersistenceFailed,
                        ..
                    })
                )),
                1 => assert!(matches!(
                    exit.outcome.unwrap(),
                    UsbEnrollmentOutcome::Ready { .. }
                )),
                2 => {
                    assert!(matches!(
                        exit.outcome,
                        Err(UsbEnrollmentFailure::Persistence(_))
                    ));
                    assert_eq!(
                        exit.coordinator.as_mut().unwrap().inspect().status,
                        EnrollmentSettlementStatus::AwaitingPersistence
                    );
                    std::fs::remove_dir(directory.path().join("devices.hopspot")).unwrap();
                    assert!(matches!(
                        exit.coordinator
                            .as_mut()
                            .unwrap()
                            .retry::<4>(&mut exit.registry, &mut exit.store)
                            .unwrap(),
                        EnrollmentCoordinatorOutcome::Recorded { .. }
                    ));
                }
                _ => assert!(matches!(
                    exit.outcome.unwrap(),
                    UsbEnrollmentOutcome::Settlement(
                        EnrollmentCoordinatorOutcome::InsufficientCapacity { required: 1, .. }
                    )
                )),
            }
        }
    });
}

#[test]
fn stopped_events_and_unadvanced_approval_return_resources_for_recovery() {
    within_runtime(async {
        for has_confirmation in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let (work, sink) = work(
                directory.path(),
                RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable,
                0,
                30_000,
            );
            if has_confirmation {
                send_confirmation(&sink, confirmation(&work));
            }
            drop(sink);
            let mut backend = Backend::new();
            let mut response = advanced();
            response.effect =
                RemoteControlControllerPairingResponseEffect::Expired { retired_link: LINK };
            backend.approve = Ok(response);
            let exit = work.run::<4>(backend, TokioClock::new()).await.unwrap();
            assert_eq!(
                exit.outcome.unwrap(),
                if has_confirmation {
                    UsbEnrollmentOutcome::ApprovalNotAdvanced(response)
                } else {
                    UsbEnrollmentOutcome::EventsUnavailable {
                        reason: EnrollmentEventsFailure::NodeStopped,
                    }
                }
            );
            assert_eq!(exit.coordinator.is_some(), has_confirmation);
            assert_eq!(exit.cleanup.is_some(), has_confirmation);
        }
    });
}

#[test]
fn exhausted_enrollment_identifiers_preserve_the_rejected_offer_and_close_its_link() {
    within_runtime(async {
        let directory = tempfile::tempdir().unwrap();
        let (mut work, sink) = work(
            directory.path(),
            RemoteControlPairingAvailabilityKind::DirectPhysicalAvailable,
            0,
            30_000,
        );
        let confirmed = confirmation(&work);
        let offer = confirmed.confirmation();
        let rejected = BeginEnrollment {
            device: work.device,
            target: RemoteControlTargetIdentity::new(*offer.target().public_keys()),
            attempt: offer.attempt_id(),
        };
        let before = work.registry.step(ReadDevice {
            device: work.device,
        });
        work.registry.exhaust_enrollment_identifiers_for_test();
        send_confirmation(&sink, confirmed);
        let backend = Backend::new();
        let calls = backend.calls.clone();
        let mut exit = work.run::<4>(backend, TokioClock::new()).await.unwrap();
        assert!(
            matches!(exit.outcome, Err(UsbEnrollmentFailure::Begin(BeginEnrollmentError::IdentifiersExhausted { rejected: actual })) if actual == rejected)
        );
        assert_eq!(
            exit.registry.step(ReadDevice {
                device: rejected.device
            }),
            before
        );
        assert!(exit.coordinator.is_none());
        assert!(calls.lock().unwrap().approved.is_empty());
        assert_eq!(calls.lock().unwrap().closed, [LINK]);
    });
}
