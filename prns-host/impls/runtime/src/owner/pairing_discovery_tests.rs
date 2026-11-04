use super::*;
use crate::remote_control_service::HostRemoteControlEvent;
use crate::tests::{
    attach_supplied_wire, persistent_config, remote_control_config, temporary_root,
    wait_until_connected,
};
use crate::{ApplicationEventDispatch, HostEmbedding, HostRuntimeEventValue};
use personal_rns::engine::{EgressTarget, OpenRemoteControlPairing};
use personal_rns::remote_control::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
    RemoteControlRequestKind, RemoteControlRequestSet,
};
use personal_rns::runtime::{
    InitiateRemoteControlControllerPairing, Message, PrnsEvent,
    RemoteControlControllerPairingSnapshot,
};
use personal_rns::units::DurationMillis;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Release {
    fn drop(&mut self) {
        *crate::lock(&self.0 .0) = true;
        self.0 .1.notify_all();
    }
}

#[test]
fn pairing_discovery_and_lost_initiation_waiter_work_in_both_dispatch_modes() {
    for mode in [
        ApplicationEventDispatch::Queue,
        ApplicationEventDispatch::EmbeddingCallback,
    ] {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let root = temporary_root("pairing-discovery-modes").unwrap();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let target_gate = gate.clone();
        let (offered, offer_seen) = std::sync::mpsc::sync_channel(1);
        let target = runtime
            .block_on(OwnedSession::open_with_embedding(
                persistent_config(&root.join("target")),
                HostEmbedding {
                    remote_control_config: Some(remote_control_config(40)),
                    application_events: ApplicationEventDispatch::EmbeddingCallback,
                    on_event: Some(Box::new(move |event: PrnsEvent<'_>| {
                        if matches!(
                            event,
                            PrnsEvent::Message(
                                Message::RemoteControlTargetPairingConfirmationRequired(_)
                            )
                        ) {
                            let _ = offered.try_send(());
                            let mut released = crate::lock(&target_gate.0);
                            while !*released {
                                released = target_gate.1.wait(released).unwrap();
                            }
                        }
                    })),
                    ..HostEmbedding::default()
                },
            ))
            .unwrap();
        let callbacks = Arc::new(AtomicUsize::new(0));
        let observed_callbacks = callbacks.clone();
        let controller = runtime
            .block_on(OwnedSession::open_with_embedding(
                persistent_config(&root.join("controller")),
                HostEmbedding {
                    remote_control_config: Some(remote_control_config(50)),
                    application_events: mode,
                    on_event: (mode == ApplicationEventDispatch::EmbeddingCallback).then(|| {
                        Box::new(move |event: PrnsEvent<'_>| {
                            if matches!(
                                event,
                                PrnsEvent::Message(Message::RemoteControlPairingAvailable(_))
                            ) {
                                observed_callbacks.fetch_add(1, Ordering::Relaxed);
                            }
                        }) as crate::embedding::HostEventCallback
                    }),
                    ..HostEmbedding::default()
                },
            ))
            .unwrap();
        let release = Release(gate);
        let target_host = target.shared.host().unwrap();
        let controller_host = controller.shared.host().unwrap();
        let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
        let (_left, left_id) = attach_supplied_wire(&target_host, "target", left).unwrap();
        let (_right, right_id) =
            attach_supplied_wire(&controller_host, "controller", right).unwrap();
        wait_until_connected(&target_host, left_id).unwrap();
        wait_until_connected(&controller_host, right_id).unwrap();
        let target_client = target.client();
        let client = controller.client();
        let scope = controller.client_scope();
        let scoped = scope.client();
        let stream = client
            .events()
            .claim_stream(prns_host::ConsumerLane::ApplicationEvents);
        assert_eq!(stream.is_ok(), mode == ApplicationEventDispatch::Queue);
        let initial = runtime
            .block_on(client.remote_control_pairing_discovery())
            .unwrap()
            .unwrap();
        assert!(initial.observations.is_empty());
        let opened = runtime
            .block_on(
                target_client.open_remote_control_pairing(OpenRemoteControlPairing {
                    target: EgressTarget::AllInterfaces,
                    expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(
                        60_000,
                    ))
                    .unwrap(),
                    attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(DurationMillis(
                        30_000,
                    ))
                    .unwrap(),
                    permissions: RemoteControlPairingPermissions::try_from(
                        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
                    )
                    .unwrap(),
                    public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                        b"observed target".as_slice(),
                    )
                    .unwrap(),
                }),
            )
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let available = loop {
            let snapshot = runtime
                .block_on(client.remote_control_pairing_discovery())
                .unwrap()
                .unwrap();
            assert_eq!(snapshot.generation, initial.generation);
            if let Some(value) = snapshot.observations.into_iter().next() {
                break value;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(available.endpoint, opened.endpoint);
        assert_eq!(available.public_app_data.as_bytes(), b"observed target");
        if let Ok(stream) = stream {
            loop {
                let value = stream.next(Some(Duration::from_secs(5))).unwrap();
                if let HostRuntimeEventValue::RemoteControl(
                    HostRemoteControlEvent::PairingAvailable { endpoint, .. },
                ) = value.value
                {
                    assert_eq!(endpoint, available.endpoint);
                    break;
                }
            }
        } else {
            assert_eq!(callbacks.load(Ordering::Relaxed), 1);
        }
        let mut initiating = Box::pin(scoped.initiate_remote_control_controller_pairing(
            InitiateRemoteControlControllerPairing {
                endpoint: available.endpoint,
                invitation_code: opened.invitation_code,
                expires_at: available.expires_at,
            },
        ));
        let mut context = Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            initiating.as_mut().poll(&mut context),
            Poll::Pending
        ));
        offer_seen.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            initiating.as_mut().poll(&mut context),
            Poll::Pending
        ));
        drop(initiating); // The exact initiation is admitted, but its offer is still held.
        scope.fence();
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if matches!(
                runtime
                    .block_on(client.remote_control_controller_pairing_snapshot())
                    .unwrap()
                    .unwrap(),
                RemoteControlControllerPairingSnapshot::AwaitingApproval { .. }
            ) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            runtime
                .block_on(scoped.remote_control_pairing_discovery())
                .unwrap()
                .unwrap()
                .observations
                .len()
                == 1
        );
        let rejected = runtime.block_on(scoped.initiate_remote_control_controller_pairing(
            InitiateRemoteControlControllerPairing {
                endpoint: available.endpoint,
                invitation_code:
                    personal_rns::remote_control::RemoteControlPairingInvitationCode::from_value(1),
                expires_at: available.expires_at,
            },
        ));
        assert!(matches!(rejected, Err(SessionError::Stopped)));
        let status = runtime
            .block_on(client.remote_control_controller_pairing_status())
            .unwrap()
            .unwrap();
        let RemoteControlControllerPairingSnapshot::AwaitingApproval { confirmation } =
            status.current
        else {
            panic!("confirmation")
        };
        assert_eq!(status.terminal, None);
        let attempt_id = confirmation.confirmation().attempt_id();
        let context = confirmation.confirmation().context();
        runtime
            .block_on(client.cancel_remote_control_controller_pairing(
                personal_rns::remote_control::RemoteControlControllerPairingSubject::Attempt {
                    attempt_id,
                    context,
                },
            ))
            .unwrap()
            .unwrap();
        let status = runtime
            .block_on(client.remote_control_controller_pairing_status())
            .unwrap()
            .unwrap();
        assert_eq!(status, personal_rns::runtime::RemoteControlControllerPairingStatus {
            current: RemoteControlControllerPairingSnapshot::Idle,
            terminal: Some(personal_rns::remote_control::RemoteControlControllerPairingTerminal {
                subject: personal_rns::remote_control::RemoteControlControllerPairingSubject::Attempt { attempt_id, context },
                outcome: personal_rns::remote_control::RemoteControlControllerPairingTerminalOutcome::Cancelled,
            }),
        });
        assert_eq!(
            runtime
                .block_on(client.remote_control_controller_pairing_status())
                .unwrap()
                .unwrap(),
            status
        );
        runtime.block_on(controller.stop()).unwrap();
        assert!(matches!(
            runtime.block_on(client.remote_control_pairing_discovery()),
            Err(SessionError::Stopped)
        ));
        runtime.block_on(target.stop()).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
