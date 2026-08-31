#![expect(clippy::expect_used)]

use core::time::Duration;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use personal_rns::engine::{
    AdmitRemoteControlControllerPairingResponseOutcome, AnnounceAppData, AnnounceNow,
    AnnounceTarget, ApproveRemoteControlControllerPairing, ApproveRemoteControlTargetPairing,
    BeginRemoteControlControllerPairing, EgressTarget, OpenRemoteControlPairing,
    OpenRemoteControlPairingFailure, OpenRemoteControlPairingRejection,
    RemoteControlControllerPairingResponseEffect, RemoteControlPairingOpened,
    RemoteControlTargetPairingApproval,
};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::identity::IdentityPublicKeys;
use personal_rns::persistence::{
    read_remote_control_controller_grants_snapshot, read_remote_control_target_accesses_snapshot,
    FileStore, PersistedStore, SnapshotRegion,
};
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    ReceiveRemoteControlControllerPairingCompletedOutcome, RemoteControlControllerGrant,
    RemoteControlPairingAttemptId, RemoteControlPairingAttemptTimeout,
    RemoteControlPairingConfirmationCode, RemoteControlPairingContext,
    RemoteControlPairingEndpoint, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
    RemoteControlTargetAccess, RemoteControlTargetIdentity, SetRemoteControlControllerGrantOutcome,
};
use personal_rns::runtime::RemoteControlPairingControlError;
use personal_rns::units::{DurationMillis, InstantMillis};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const PAIRING_WINDOW: DurationMillis = DurationMillis(60_000);
const PAIRING_ATTEMPT_TIMEOUT: DurationMillis = DurationMillis(30_000);
const PUBLIC_APP_DATA: &[u8] = b"integration target";
const TARGET_NODE_CONTROLLER_SECRET_FILL: u8 = 0xA1;
const TARGET_NODE_TARGET_SECRET_FILL: u8 = 0xA2;
const CONTROLLER_NODE_CONTROLLER_SECRET_FILL: u8 = 0xB1;
const CONTROLLER_NODE_TARGET_SECRET_FILL: u8 = 0xB2;
static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct PairingAvailability {
    endpoint: RemoteControlPairingEndpoint,
    expires_at: InstantMillis,
    public_app_data: std::vec::Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct PairingConfirmation {
    attempt_id: RemoteControlPairingAttemptId,
    context: RemoteControlPairingContext,
    code: RemoteControlPairingConfirmationCode,
    controller: RemoteControlControllerIdentity,
    target: IdentityPublicKeys,
    permissions: RemoteControlPairingPermissions,
}

#[derive(Debug, PartialEq, Eq)]
enum TwoNodeExchangeOutcome {
    Completed,
    TargetStopped,
    ControllerStopped,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_pairing_persists_matching_authorizations_on_both_nodes() {
    let persistence = PairingPersistenceDirectories::new();
    let target_identity_secrets = remote_control_identity_secrets(
        TARGET_NODE_CONTROLLER_SECRET_FILL,
        TARGET_NODE_TARGET_SECRET_FILL,
    );
    let target_public_keys = *target_identity_secrets.identities().target().public_keys();
    let controller_identity_secrets = remote_control_identity_secrets(
        CONTROLLER_NODE_CONTROLLER_SECRET_FILL,
        CONTROLLER_NODE_TARGET_SECRET_FILL,
    );
    let controller_identity = *controller_identity_secrets.identities().controller();

    let (target_confirmation_tx, mut target_confirmation_rx) =
        tokio::sync::mpsc::unbounded_channel();
    let (target_persisted_tx, mut target_persisted_rx) = tokio::sync::mpsc::unbounded_channel();
    let target = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: remote_control_service(target_identity_secrets),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| match event {
            PrnsEvent::Message(Message::RemoteControlTargetPairingConfirmationRequired(
                pairing,
            )) => {
                let _ignored = target_confirmation_tx.send(PairingConfirmation {
                    attempt_id: pairing.attempt_id(),
                    context: pairing.context(),
                    code: pairing.confirmation_code(),
                    controller: *pairing.controller(),
                    target: *pairing.target().public_keys(),
                    permissions: pairing.permissions().clone(),
                });
            }
            PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationPersisted {
                attempt_id,
            }) => {
                let _ignored = target_persisted_tx.send(attempt_id);
            }
            PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
        },
        interfaces: ManuallyAttached,
        persistence: NodePersistence::custom_dir(&persistence.target)
            .expect("the target persistence directory is usable"),
    });
    let target_handle = target.handle();

    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("the target TCP server binds");
    let server_address = server
        .local_addr()
        .expect("the target TCP server has an address")
        .to_string();
    let _server = target_handle.supervise(server);

    let (availability_tx, mut availability_rx) = tokio::sync::mpsc::unbounded_channel();
    let (controller_confirmation_tx, mut controller_confirmation_rx) =
        tokio::sync::mpsc::unbounded_channel();
    let (controller_persisted_tx, mut controller_persisted_rx) =
        tokio::sync::mpsc::unbounded_channel();
    let controller = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: remote_control_service(controller_identity_secrets),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| match event {
            PrnsEvent::Message(Message::RemoteControlPairingAvailable(pairing)) => {
                let _ignored = availability_tx.send(PairingAvailability {
                    endpoint: pairing.endpoint(),
                    expires_at: pairing.expires_at(),
                    public_app_data: pairing.public_app_data().as_bytes().to_vec(),
                });
            }
            PrnsEvent::Message(Message::RemoteControlControllerPairingConfirmationRequired(
                pairing,
            )) => {
                let _ignored = controller_confirmation_tx.send(PairingConfirmation {
                    attempt_id: pairing.attempt_id(),
                    context: pairing.context(),
                    code: pairing.confirmation_code(),
                    controller: *pairing.controller(),
                    target: *pairing.target().public_keys(),
                    permissions: pairing.permissions().clone(),
                });
            }
            PrnsEvent::Message(Message::RemoteControlControllerPairingAuthorizationPersisted {
                attempt_id,
            }) => {
                let _ignored = controller_persisted_tx.send(attempt_id);
            }
            PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(TcpClientInterface::new(server_address));
        },
        persistence: NodePersistence::custom_dir(&persistence.controller)
            .expect("the controller persistence directory is usable"),
    });
    let controller_handle = controller.handle();

    let exchange = async {
        wait_for_direct_connection(&target_handle, &controller_handle).await;

        let opened = open_pairing_when_attached(
            &target_handle,
            OpenRemoteControlPairing {
                target: EgressTarget::AllInterfaces,
                expires_after: RemoteControlPairingExpiresAfter::try_from(PAIRING_WINDOW)
                    .expect("the pairing window is valid"),
                attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(
                    PAIRING_ATTEMPT_TIMEOUT,
                )
                .expect("the pairing attempt timeout is valid"),
                permissions: RemoteControlPairingPermissions::try_from(
                    RemoteControlRequestSet::all(),
                )
                .expect("the request set is not empty"),
                public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(PUBLIC_APP_DATA)
                    .expect("the public app data fits"),
            },
        )
        .await
        .expect("the target opens pairing once its interface is attached");

        let availability = availability_rx
            .recv()
            .await
            .expect("the controller observes pairing availability");
        assert_eq!(availability.endpoint, opened.endpoint);
        assert_eq!(availability.public_app_data, PUBLIC_APP_DATA);

        let link_id = controller_handle
            .establish_link(availability.endpoint.destination_hash())
            .await
            .expect("the controller links to the ephemeral pairing endpoint");

        let _offered = controller_handle
            .begin_remote_control_controller_pairing(BeginRemoteControlControllerPairing {
                context: RemoteControlPairingContext::new(availability.endpoint, link_id),
                invitation_code: opened.invitation_code,
                pairing_expires_at: availability.expires_at,
            })
            .await
            .expect("the target returns a valid pairing offer");

        let target_confirmation = target_confirmation_rx
            .recv()
            .await
            .expect("the target asks for confirmation");
        let controller_confirmation = controller_confirmation_rx
            .recv()
            .await
            .expect("the controller asks for confirmation");
        assert_eq!(controller_confirmation, target_confirmation);

        let attempt_id = controller_confirmation.attempt_id;
        assert_eq!(
            target_handle
                .approve_remote_control_target_pairing(ApproveRemoteControlTargetPairing {
                    attempt_id,
                })
                .await,
            Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit { attempt_id }),
        );
        let completed = controller_handle
            .approve_remote_control_controller_pairing(ApproveRemoteControlControllerPairing {
                attempt_id,
            })
            .await
            .expect("the target returns a valid signed completion");
        assert_eq!(
            completed.admission,
            AdmitRemoteControlControllerPairingResponseOutcome::Completed(
                ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed {
                    attempt_id,
                },
            ),
        );
        assert_eq!(
            completed.effect,
            RemoteControlControllerPairingResponseEffect::Advanced,
        );

        assert_eq!(
            target_persisted_rx
                .recv()
                .await
                .expect("the target authorization persistence completes"),
            attempt_id,
        );
        assert_eq!(
            controller_persisted_rx
                .recv()
                .await
                .expect("the controller authorization persistence completes"),
            attempt_id,
        );

        assert_eq!(
            persisted_controller_grants(&persistence.target),
            [RemoteControlControllerGrant::new(
                controller_identity,
                RemoteControlRequestSet::all(),
            )
            .expect("the complete request set is not empty")],
        );
        assert_eq!(
            persisted_target_accesses(&persistence.controller),
            [RemoteControlTargetAccess::new(
                RemoteControlTargetIdentity::new(target_public_keys),
                RemoteControlRequestSet::all(),
            )
            .expect("the complete request set is not empty")],
        );
    };

    let outcome = tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("pairing completes within the bounded test window");
            TwoNodeExchangeOutcome::Completed
        }
        result = target.run() => {
            result.expect("the target node runs");
            TwoNodeExchangeOutcome::TargetStopped
        }
        result = controller.run() => {
            result.expect("the controller node runs");
            TwoNodeExchangeOutcome::ControllerStopped
        }
    };
    assert_eq!(outcome, TwoNodeExchangeOutcome::Completed);

    describe_through_restored_pairing(&persistence).await;
}

async fn describe_through_restored_pairing(persistence: &PairingPersistenceDirectories) {
    let target_identity_secrets = remote_control_identity_secrets(
        TARGET_NODE_CONTROLLER_SECRET_FILL,
        TARGET_NODE_TARGET_SECRET_FILL,
    );
    let target_identity_hash = target_identity_secrets
        .identities()
        .target()
        .identity_hash();
    let target_endpoint = target_identity_secrets.identities().target().endpoint();
    let (identified_controller_tx, mut identified_controller_rx) =
        tokio::sync::mpsc::unbounded_channel();
    let target = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: remote_control_service(target_identity_secrets),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| match event {
            PrnsEvent::Diagnostic(Diagnostic::PeerIdentified { identity, .. }) => {
                let _ignored = identified_controller_tx.send(identity);
            }
            PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
        },
        interfaces: ManuallyAttached,
        persistence: NodePersistence::custom_dir(&persistence.target)
            .expect("the target persistence directory remains usable"),
    });
    let target_handle = target.handle();

    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("the restarted target TCP server binds");
    let server_address = server
        .local_addr()
        .expect("the restarted target TCP server has an address")
        .to_string();
    let _server = target_handle.supervise(server);

    let controller_identity_secrets = remote_control_identity_secrets(
        CONTROLLER_NODE_CONTROLLER_SECRET_FILL,
        CONTROLLER_NODE_TARGET_SECRET_FILL,
    );
    let controller_identity = *controller_identity_secrets.identities().controller();
    let (target_announce_tx, mut target_announce_rx) = tokio::sync::mpsc::unbounded_channel();
    let controller = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: remote_control_service(controller_identity_secrets),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                let _ignored = target_announce_tx.send(destination);
            }
            PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(TcpClientInterface::new(server_address));
        },
        persistence: NodePersistence::custom_dir(&persistence.controller)
            .expect("the controller persistence directory remains usable"),
    });
    let controller_handle = controller.handle();

    let exchange = async {
        wait_for_direct_connection(&target_handle, &controller_handle).await;
        assert_eq!(
            target_handle
                .set_remote_control_controller_grant(
                    RemoteControlControllerGrant::new(
                        controller_identity,
                        RemoteControlRequestSet::all(),
                    )
                    .expect("the complete request set is not empty"),
                )
                .await,
            Ok(SetRemoteControlControllerGrantOutcome::Unchanged),
        );
        target_handle
            .announce_now(AnnounceNow {
                destination: target_endpoint.destination_hash(),
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .expect("the restarted target announces its stable RemoteControl endpoint");
        assert_eq!(
            target_announce_rx
                .recv()
                .await
                .expect("the restarted controller hears the stable RemoteControl endpoint"),
            target_endpoint.destination_hash(),
        );

        let inventory = controller_handle
            .remote_control_target_inventory()
            .await
            .expect("the restarted controller reads its persisted target inventory");
        let [authorized_target] = inventory.targets() else {
            panic!("the restarted controller has exactly one authorized target")
        };
        assert_eq!(authorized_target.identity_hash(), target_identity_hash);
        let remote_control = controller_handle
            .connect_remote_control_target(authorized_target.identity_hash())
            .await
            .expect("the restarted controller resolves, links, and identifies the paired target");
        assert_eq!(
            identified_controller_rx
                .recv()
                .await
                .expect("the restarted target observes the paired controller identity"),
            controller_identity.identity_hash(),
        );
        let (description, _rtt) = remote_control
            .describe()
            .await
            .expect("the restored controller grant admits RemoteControl describe");
        assert_eq!(
            description.available_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );
    };

    let outcome = tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("restored RemoteControl access works within the bounded test window");
            TwoNodeExchangeOutcome::Completed
        }
        result = target.run() => {
            result.expect("the restarted target node runs");
            TwoNodeExchangeOutcome::TargetStopped
        }
        result = controller.run() => {
            result.expect("the restarted controller node runs");
            TwoNodeExchangeOutcome::ControllerStopped
        }
    };
    assert_eq!(outcome, TwoNodeExchangeOutcome::Completed);
}

struct PairingPersistenceDirectories {
    target: PathBuf,
    controller: PathBuf,
}

impl PairingPersistenceDirectories {
    fn new() -> Self {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "prns-pairing-integration-{}-{sequence}",
            std::process::id(),
        ));
        Self {
            target: root.join("target"),
            controller: root.join("controller"),
        }
    }
}

impl Drop for PairingPersistenceDirectories {
    fn drop(&mut self) {
        let Some(root) = self.target.parent() else {
            return;
        };
        let _removed = std::fs::remove_dir_all(root);
    }
}

fn remote_control_identity_secrets(
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
    .expect("the controller and target identities are distinct")
}

fn remote_control_service(
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
) -> Result<RemoteControlPairingOpened, OpenRemoteControlPairingControlError> {
    loop {
        match target.open_remote_control_pairing(open.clone()).await {
            Ok(opened) => return Ok(opened),
            Err(RemoteControlPairingControlError::Failed(
                OpenRemoteControlPairingFailure::Rejected(
                    OpenRemoteControlPairingRejection::NoTransmittingInterfaces,
                ),
            )) => tokio::task::yield_now().await,
            Err(error) => return Err(error),
        }
    }
}

fn persisted_controller_grants(directory: &Path) -> std::vec::Vec<RemoteControlControllerGrant> {
    let snapshot = load_snapshot(directory, SnapshotRegion::RemoteControlControllerGrants);
    read_remote_control_controller_grants_snapshot(&snapshot)
        .expect("the target stored a valid controller-grant snapshot")
        .collect()
}

fn persisted_target_accesses(directory: &Path) -> std::vec::Vec<RemoteControlTargetAccess> {
    let snapshot = load_snapshot(directory, SnapshotRegion::RemoteControlTargetAccesses);
    read_remote_control_target_accesses_snapshot(&snapshot)
        .expect("the controller stored a valid target-access snapshot")
        .collect()
}

fn load_snapshot(directory: &Path, region: SnapshotRegion) -> std::vec::Vec<u8> {
    let store = FileStore::new(directory);
    let stored_len = store
        .stored_len(region)
        .expect("the authorization snapshot metadata is readable")
        .expect("the authorization snapshot exists");
    let mut snapshot = std::vec![0; stored_len];
    let loaded = store
        .load(region, &mut snapshot)
        .expect("the authorization snapshot is readable")
        .expect("the authorization snapshot exists");
    loaded.to_vec()
}
