use personal_rns::engine::{
    EstablishLinkFailure, EstablishLinkRejection, SendRequestFailure, WriteEstablishLinkRejection,
};
use personal_rns::identity::IdentityHash;
use personal_rns::prelude::{
    ConnectRemoteControlTargetError, PrnsNodeHandle, RemoteControlError,
    RemoteControlTargetAccessControl, RemoteControlTargetOperationError, SendError,
};

use crate::contract::{
    DescribeRemoteControlTargetInput, DevelopmentNodeOperation, DevelopmentNodeOperationKind,
    RemoteControlDescribeFailureStage, RemoteControlDescribeOutcome, RemoteControlTargetSnapshot,
    U64String,
};
use crate::pairing::request_kinds;
use crate::snapshot::SnapshotStore;

pub async fn refresh_targets(
    handle: &PrnsNodeHandle,
    snapshots: &SnapshotStore,
) -> Result<Vec<RemoteControlTargetSnapshot>, String> {
    let inventory = handle
        .remote_control_target_inventory()
        .await
        .map_err(|_| "The upstream target inventory is unavailable.".to_owned())?;
    let mut targets = Vec::with_capacity(inventory.len());
    for authorized in inventory.targets() {
        let resolved = handle
            .resolve_remote_control_target(authorized.identity_hash())
            .await
            .map_err(|_| "A persisted target could not be resolved.".to_owned())?;
        targets.push(target_snapshot(&resolved));
    }
    snapshots.update(|snapshot| snapshot.paired_targets.clone_from(&targets));
    Ok(targets)
}

pub async fn describe(
    handle: &PrnsNodeHandle,
    snapshots: &SnapshotStore,
    input: DescribeRemoteControlTargetInput,
) -> RemoteControlDescribeOutcome {
    let Some(target_identity) = identity_hash(&input.target_identity_fingerprint) else {
        return failed(
            RemoteControlDescribeFailureStage::Input,
            "A target identity fingerprint must contain exactly 16 bytes.",
        );
    };
    snapshots.update(|snapshot| {
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Describe,
            started_at_millis: U64String::from(monotonic_millis()),
        });
    });

    let resolved = match handle.resolve_remote_control_target(target_identity).await {
        Ok(resolved) => resolved,
        Err(_) => {
            clear_operation(snapshots);
            return failed(
                RemoteControlDescribeFailureStage::Inventory,
                "The selected target is not present in the upstream authorization inventory.",
            );
        }
    };
    let target = target_snapshot(&resolved);
    let connected = match handle.connect_remote_control_target(target_identity).await {
        Ok(connected) => connected,
        Err(error) => {
            clear_operation(snapshots);
            return failed_connect(error);
        }
    };
    let result = connected.describe().await;
    let _closed = connected.close();
    match result {
        Ok((description, rtt)) => {
            let available_requests = request_kinds(description.available_requests());
            let _ = refresh_targets(handle, snapshots).await;
            clear_operation(snapshots);
            RemoteControlDescribeOutcome::Described {
                target,
                available_requests,
                rtt_millis: U64String::from(rtt.millis()),
                snapshot: Box::new(snapshots.read()),
            }
        }
        Err(error) => {
            clear_operation(snapshots);
            failed_operation(error)
        }
    }
}

fn target_snapshot(
    resolved: &personal_rns::runtime::ResolvedRemoteControlTarget,
) -> RemoteControlTargetSnapshot {
    RemoteControlTargetSnapshot {
        target_identity_fingerprint: resolved.target().as_bytes().to_vec(),
        destination: resolved.endpoint().destination_hash().as_bytes().to_vec(),
        controller_identity_fingerprint: resolved.controller().identity_hash().as_bytes().to_vec(),
        permitted_requests: request_kinds(resolved.permitted_requests()),
    }
}

fn identity_hash(bytes: &[u8]) -> Option<IdentityHash> {
    let value = <[u8; 16]>::try_from(bytes).ok()?;
    Some(IdentityHash::new(value))
}

fn failed_connect(error: ConnectRemoteControlTargetError) -> RemoteControlDescribeOutcome {
    match error {
        ConnectRemoteControlTargetError::Resolve(_) => failed(
            RemoteControlDescribeFailureStage::Inventory,
            "The selected target authorization could not be resolved.",
        ),
        ConnectRemoteControlTargetError::EstablishLink(error) => {
            match classify_establish_link_failure(&error) {
                EstablishLinkFailureClass::Route => failed(
                    RemoteControlDescribeFailureStage::Route,
                    "Prns has no current route to the selected target.",
                ),
                EstablishLinkFailureClass::Link => failed(
                    RemoteControlDescribeFailureStage::Link,
                    "Prns could not establish a Link to the selected target.",
                ),
                EstablishLinkFailureClass::Node => failed(
                    RemoteControlDescribeFailureStage::Node,
                    "The Prns node stopped while opening the target Link.",
                ),
            }
        }
        ConnectRemoteControlTargetError::Identify(_) => failed(
            RemoteControlDescribeFailureStage::Identification,
            "Prns could not identify the controller on the target Link.",
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EstablishLinkFailureClass {
    Route,
    Link,
    Node,
}

pub(crate) fn classify_establish_link_failure(
    error: &SendError<EstablishLinkFailure>,
) -> EstablishLinkFailureClass {
    match error {
        SendError::Failed(EstablishLinkFailure::Rejected(
            EstablishLinkRejection::NoRouteToDestination
            | EstablishLinkRejection::NotDirectlyReachable,
        ))
        | SendError::Failed(EstablishLinkFailure::WriteFailed(
            WriteEstablishLinkRejection::RouteVanished,
        )) => EstablishLinkFailureClass::Route,
        SendError::NodeStopped => EstablishLinkFailureClass::Node,
        SendError::PayloadTooLarge
        | SendError::Busy
        | SendError::Failed(EstablishLinkFailure::WriteFailed(
            WriteEstablishLinkRejection::Serialize
            | WriteEstablishLinkRejection::LinkTableFull
            | WriteEstablishLinkRejection::DuplicateLinkId,
        ))
        | SendError::Failed(EstablishLinkFailure::Timeout) => EstablishLinkFailureClass::Link,
    }
}

fn failed_operation(error: RemoteControlTargetOperationError) -> RemoteControlDescribeOutcome {
    match error {
        RemoteControlTargetOperationError::NotPermitted(_) => failed(
            RemoteControlDescribeFailureStage::Permission,
            "The persisted target grant does not permit Describe.",
        ),
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::Failed(SendRequestFailure::Timeout),
        )) => failed(
            RemoteControlDescribeFailureStage::Timeout,
            "The upstream Link-default request timeout elapsed.",
        ),
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::NodeStopped,
        )) => failed(
            RemoteControlDescribeFailureStage::Node,
            "The Prns node stopped during Describe.",
        ),
        RemoteControlTargetOperationError::Exchange(_) => failed(
            RemoteControlDescribeFailureStage::Request,
            "The upstream RemoteControl Describe exchange failed.",
        ),
    }
}

fn failed(stage: RemoteControlDescribeFailureStage, detail: &str) -> RemoteControlDescribeOutcome {
    RemoteControlDescribeOutcome::Failed {
        stage,
        detail: detail.to_owned(),
    }
}

fn clear_operation(snapshots: &SnapshotStore) {
    snapshots.update(|snapshot| snapshot.active_operation = None);
}

fn monotonic_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_input_is_exactly_sized() {
        assert!(identity_hash(&[0; 15]).is_none());
        assert!(identity_hash(&[0; 16]).is_some());
        assert!(identity_hash(&[0; 17]).is_none());
    }

    #[test]
    fn establish_link_failures_distinguish_route_from_link_failures() {
        let route_failures = [
            SendError::Failed(EstablishLinkFailure::Rejected(
                EstablishLinkRejection::NoRouteToDestination,
            )),
            SendError::Failed(EstablishLinkFailure::Rejected(
                EstablishLinkRejection::NotDirectlyReachable,
            )),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::RouteVanished,
            )),
        ];
        for failure in route_failures {
            assert_eq!(
                classify_establish_link_failure(&failure),
                EstablishLinkFailureClass::Route
            );
            assert!(matches!(
                failed_connect(ConnectRemoteControlTargetError::EstablishLink(failure)),
                RemoteControlDescribeOutcome::Failed {
                    stage: RemoteControlDescribeFailureStage::Route,
                    ..
                }
            ));
        }

        let link_failures = [
            SendError::Failed(EstablishLinkFailure::Timeout),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::Serialize,
            )),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::LinkTableFull,
            )),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::DuplicateLinkId,
            )),
            SendError::Busy,
            SendError::PayloadTooLarge,
        ];
        for failure in link_failures {
            assert_eq!(
                classify_establish_link_failure(&failure),
                EstablishLinkFailureClass::Link
            );
            assert!(matches!(
                failed_connect(ConnectRemoteControlTargetError::EstablishLink(failure)),
                RemoteControlDescribeOutcome::Failed {
                    stage: RemoteControlDescribeFailureStage::Link,
                    ..
                }
            ));
        }

        let node_stopped = SendError::NodeStopped;
        assert_eq!(
            classify_establish_link_failure(&node_stopped),
            EstablishLinkFailureClass::Node
        );
        assert!(matches!(
            failed_connect(ConnectRemoteControlTargetError::EstablishLink(node_stopped)),
            RemoteControlDescribeOutcome::Failed {
                stage: RemoteControlDescribeFailureStage::Node,
                ..
            }
        ));
    }

    #[cfg(feature = "host-test")]
    mod host {
        use core::time::Duration;

        use personal_rns::engine::{
            EgressTarget, OpenRemoteControlPairing, OpenRemoteControlPairingFailure,
            OpenRemoteControlPairingRejection, RemoteControlPairingOpened,
            RemoteControlTargetPairingApproval,
        };
        use personal_rns::identity::vault::IdentitySecretKey;
        use personal_rns::prelude::*;
        use personal_rns::remote_control::{
            RemoteControlInitialControllerGrants, RemoteControlPairingAttemptTimeout,
            RemoteControlPairingExpiresAfter, RemoteControlPairingPermissions,
            RemoteControlPairingPublicAppDataBytes, RemoteControlSelfAnnouncement,
            RemoteControlService,
        };
        use personal_rns::runtime::{NodePersistence, RemoteControlPairingControlError};
        use personal_rns::units::DurationMillis;

        use super::*;
        use crate::contract::{DevelopmentNodeRuntime, RemoteControlRequestKind};

        const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
        const PAIRING_WINDOW: DurationMillis = DurationMillis(60_000);
        const PAIRING_ATTEMPT_TIMEOUT: DurationMillis = DurationMillis(30_000);

        struct PersistenceDirectories {
            _temporary: tempfile::TempDir,
            target: std::path::PathBuf,
            controller: std::path::PathBuf,
        }

        impl PersistenceDirectories {
            fn new() -> Self {
                let temporary = tempfile::tempdir().expect("temporary persistence directory");
                Self {
                    target: temporary.path().join("target"),
                    controller: temporary.path().join("controller"),
                    _temporary: temporary,
                }
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn restored_inventory_drives_describe_over_upstream_tcp() {
            let persistence = PersistenceDirectories::new();
            pair_once(&persistence).await;
            describe_after_restart(&persistence).await;
        }

        async fn pair_once(persistence: &PersistenceDirectories) {
            let target_identity_secrets = identity_secrets(0xA1, 0xA2);
            let controller_identity_secrets = identity_secrets(0xB1, 0xB2);

            let (target_confirmation_tx, mut target_confirmation_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let (target_persisted_tx, mut target_persisted_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let target = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(target_identity_secrets),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| match event {
                    PrnsEvent::Message(
                        Message::RemoteControlTargetPairingConfirmationRequired(pairing),
                    ) => {
                        let _ignored = target_confirmation_tx.send(pairing);
                    }
                    PrnsEvent::Message(
                        Message::RemoteControlTargetPairingAuthorizationPersisted { attempt_id },
                    ) => {
                        let _ignored = target_persisted_tx.send(attempt_id);
                    }
                    PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
                },
                interfaces: ManuallyAttached,
                persistence: NodePersistence::custom_dir(&persistence.target)
                    .expect("target persistence opens"),
            });
            let target_handle = target.handle();
            let server = TcpServer::bind("127.0.0.1:0")
                .await
                .expect("target TCP server binds");
            let server_address = server
                .local_addr()
                .expect("target TCP server has an address")
                .to_string();
            let _server = target_handle.supervise(server);

            let (availability_tx, mut availability_rx) = tokio::sync::mpsc::unbounded_channel();
            let (controller_confirmation_tx, mut controller_confirmation_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let (controller_persisted_tx, mut controller_persisted_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let controller = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(controller_identity_secrets),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| match event {
                    PrnsEvent::Message(Message::RemoteControlPairingAvailable(pairing)) => {
                        let _ignored =
                            availability_tx.send((pairing.endpoint(), pairing.expires_at()));
                    }
                    PrnsEvent::Message(
                        Message::RemoteControlControllerPairingConfirmationRequired(pairing),
                    ) => {
                        let _ignored = controller_confirmation_tx.send(pairing);
                    }
                    PrnsEvent::Message(
                        Message::RemoteControlControllerPairingAuthorizationPersisted {
                            attempt_id,
                        },
                    ) => {
                        let _ignored = controller_persisted_tx.send(attempt_id);
                    }
                    PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
                },
                interfaces: move |node: &PrnsNodeHandle| {
                    node.attach(TcpClientInterface::new(server_address));
                },
                persistence: NodePersistence::custom_dir(&persistence.controller)
                    .expect("controller persistence opens"),
            });
            let controller_handle = controller.handle();

            let exchange = async {
                wait_for_direct_connection(&target_handle, &controller_handle).await;
                let opened = open_pairing_when_attached(
                    &target_handle,
                    OpenRemoteControlPairing {
                        target: EgressTarget::AllInterfaces,
                        expires_after: RemoteControlPairingExpiresAfter::try_from(PAIRING_WINDOW)
                            .expect("pairing window is valid"),
                        attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(
                            PAIRING_ATTEMPT_TIMEOUT,
                        )
                        .expect("attempt timeout is valid"),
                        permissions: RemoteControlPairingPermissions::try_from(
                            RemoteControlRequestSet::all(),
                        )
                        .expect("permissions are not empty"),
                        public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                            b"native composition host test".as_slice(),
                        )
                        .expect("public application data fits"),
                    },
                )
                .await;
                let (endpoint, expires_at) = availability_rx
                    .recv()
                    .await
                    .expect("controller observes pairing availability");
                let _offered = controller_handle
                    .initiate_remote_control_controller_pairing(
                        InitiateRemoteControlControllerPairing {
                            endpoint,
                            invitation_code: opened.invitation_code,
                            expires_at,
                        },
                    )
                    .await
                    .expect("controller receives a pairing offer");
                let target_confirmation = target_confirmation_rx
                    .recv()
                    .await
                    .expect("target asks for confirmation");
                let controller_confirmation = controller_confirmation_rx
                    .recv()
                    .await
                    .expect("controller asks for confirmation");
                let attempt_id = controller_confirmation.confirmation().attempt_id();
                assert_eq!(
                    target_handle
                        .approve_remote_control_target_pairing(target_confirmation.approval())
                        .await,
                    Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit { attempt_id }),
                );
                controller_handle
                    .approve_remote_control_controller_pairing(controller_confirmation.approval())
                    .await
                    .expect("controller completes pairing");
                assert_eq!(
                    target_persisted_rx
                        .recv()
                        .await
                        .expect("target persists its controller grant"),
                    attempt_id,
                );
                assert_eq!(
                    controller_persisted_rx
                        .recv()
                        .await
                        .expect("controller persists its target access"),
                    attempt_id,
                );
            };

            tokio::select! {
                result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
                    result.expect("pairing completes within the bounded window");
                }
                result = target.run() => panic!("target stopped while pairing: {result:?}"),
                result = controller.run() => panic!("controller stopped while pairing: {result:?}"),
            }
        }

        async fn describe_after_restart(persistence: &PersistenceDirectories) {
            let target_identity_secrets = identity_secrets(0xA1, 0xA2);
            let target_identity_hash = target_identity_secrets
                .identities()
                .target()
                .identity_hash();
            let target_endpoint = target_identity_secrets.identities().target().endpoint();
            let controller_identity_secrets = identity_secrets(0xB1, 0xB2);

            let (target_restored_tx, mut target_restored_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let target = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(target_identity_secrets),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| {
                    if matches!(
                        event,
                        PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored { .. })
                    ) {
                        let _ignored = target_restored_tx.send(());
                    }
                },
                interfaces: ManuallyAttached,
                persistence: NodePersistence::custom_dir(&persistence.target)
                    .expect("restarted target persistence opens"),
            });
            let target_handle = target.handle();
            let server = TcpServer::bind("127.0.0.1:0")
                .await
                .expect("restarted target TCP server binds");
            let server_address = server
                .local_addr()
                .expect("restarted target TCP server has an address")
                .to_string();
            let _server = target_handle.supervise(server);

            let (target_announce_tx, mut target_announce_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let (controller_restored_tx, mut controller_restored_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let controller = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(controller_identity_secrets),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| match event {
                    PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                        let _ignored = target_announce_tx.send(destination);
                    }
                    PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored { .. }) => {
                        let _ignored = controller_restored_tx.send(());
                    }
                    PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
                },
                interfaces: move |node: &PrnsNodeHandle| {
                    node.attach(TcpClientInterface::new(server_address));
                },
                persistence: NodePersistence::custom_dir(&persistence.controller)
                    .expect("restarted controller persistence opens"),
            });
            let controller_handle = controller.handle();
            let snapshots = SnapshotStore::new();
            snapshots.set_runtime(DevelopmentNodeRuntime::Running);

            let exchange = async {
                target_restored_rx
                    .recv()
                    .await
                    .expect("restarted target restores its controller grant");
                controller_restored_rx
                    .recv()
                    .await
                    .expect("restarted controller restores its target access");
                wait_for_direct_connection(&target_handle, &controller_handle).await;
                target_handle
                    .announce_now(AnnounceNow {
                        destination: target_endpoint.destination_hash(),
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })
                    .await
                    .expect("restarted target announces its stable endpoint");
                assert_eq!(
                    target_announce_rx
                        .recv()
                        .await
                        .expect("restarted controller hears the target announcement"),
                    target_endpoint.destination_hash(),
                );

                let targets = refresh_targets(&controller_handle, &snapshots)
                    .await
                    .expect("application projection restores the upstream inventory");
                assert_eq!(targets.len(), 1);
                assert_eq!(
                    targets[0].target_identity_fingerprint,
                    target_identity_hash.as_bytes(),
                );

                let outcome = describe(
                    &controller_handle,
                    &snapshots,
                    DescribeRemoteControlTargetInput {
                        target_identity_fingerprint: target_identity_hash.as_bytes().to_vec(),
                    },
                )
                .await;
                let RemoteControlDescribeOutcome::Described {
                    target,
                    available_requests,
                    snapshot,
                    ..
                } = outcome
                else {
                    panic!("application Describe failed after restart: {outcome:?}");
                };
                assert_eq!(
                    target.target_identity_fingerprint,
                    target_identity_hash.as_bytes(),
                );
                assert_eq!(available_requests, vec![RemoteControlRequestKind::Describe]);
                assert_eq!(snapshot.paired_targets, targets);
                assert!(snapshot.active_operation.is_none());
            };

            tokio::select! {
                result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
                    result.expect("restored Describe completes within the bounded window");
                }
                result = target.run() => panic!("restarted target stopped: {result:?}"),
                result = controller.run() => panic!("restarted controller stopped: {result:?}"),
            }
        }

        fn identity_secrets(
            controller_fill: u8,
            target_fill: u8,
        ) -> RemoteControlNodeIdentitySecrets {
            RemoteControlNodeIdentitySecrets::new(
                RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                    [controller_fill; IDENTITY_SECRET_KEY_LEN],
                )),
                RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                    [target_fill; IDENTITY_SECRET_KEY_LEN],
                )),
            )
            .expect("controller and target identities are distinct")
        }

        fn service(
            identity_secrets: RemoteControlNodeIdentitySecrets,
        ) -> RemoteControlService<'static> {
            RemoteControlService::new(
                identity_secrets,
                RemoteControlInitialControllerGrants::Nobody,
                RemoteControlSelfAnnouncement::Unavailable,
            )
        }

        async fn wait_for_direct_connection(target: &PrnsNodeHandle, controller: &PrnsNodeHandle) {
            loop {
                let target_connected = target
                    .interfaces()
                    .iter()
                    .any(|interface| interface.connection.is_online());
                let controller_connected = controller
                    .interfaces()
                    .iter()
                    .any(|interface| interface.connection.is_online());
                if target_connected && controller_connected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        }

        async fn open_pairing_when_attached(
            target: &PrnsNodeHandle,
            open: OpenRemoteControlPairing,
        ) -> RemoteControlPairingOpened {
            loop {
                match target.open_remote_control_pairing(open.clone()).await {
                    Ok(opened) => return opened,
                    Err(RemoteControlPairingControlError::Failed(
                        OpenRemoteControlPairingFailure::Rejected(
                            OpenRemoteControlPairingRejection::NoTransmittingInterfaces,
                        ),
                    )) => tokio::task::yield_now().await,
                    Err(error) => panic!("target could not open pairing: {error:?}"),
                }
            }
        }
    }
}
