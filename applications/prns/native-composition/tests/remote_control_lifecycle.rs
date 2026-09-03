#![cfg(feature = "host-test")]
#![allow(clippy::expect_used, clippy::panic)]

use core::time::Duration;
use std::path::{Path, PathBuf};

use personal_rns::engine::{
    EgressTarget, OpenRemoteControlPairing, OpenRemoteControlPairingFailure,
    OpenRemoteControlPairingRejection, RemoteControlPairingOpened,
    RemoteControlTargetPairingApproval,
};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlInitialControllerGrants, RemoteControlNodeIdentitySecrets,
    RemoteControlPairingAttemptTimeout, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
    RemoteControlRequestSet, RemoteControlSelfAnnouncement, RemoteControlService,
};
use personal_rns::runtime::{
    NodePersistence, NodeRunError, RemoteControlPairingControlError,
    RemoteControlTargetPairingConfirmation,
};
use personal_rns::units::DurationMillis;
use prns_app::contract::{
    DescribeRemoteControlTargetInput, DevelopmentNodeRuntime, DevelopmentNodeSnapshot,
    DevelopmentNodeStartInput, DevelopmentNodeStartOutcome, DevelopmentNodeStopOutcome,
    IdentityCreationOutcome, InitiateRemoteControlPairingInput, LocalHostState,
    RemoteControlDescribeFailureStage, RemoteControlDescribeOutcome,
    RemoteControlPairingCommandOutcome, RemoteControlPairingDecisionInput,
    RemoteControlPairingFailureStage, RemoteControlPairingState, RemoteControlRequestKind,
};
use prns_app::host_test as app;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(12);
const UNAVAILABLE_TARGET_TIMEOUT: Duration = Duration::from_secs(18);
const NORMAL_PAIRING_WINDOW: DurationMillis = DurationMillis(30_000);
const NORMAL_ATTEMPT_TIMEOUT: DurationMillis = DurationMillis(10_000);

struct AppStorage {
    _temporary: tempfile::TempDir,
    root: PathBuf,
}

impl AppStorage {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary application storage");
        Self {
            root: temporary.path().join("prns").join("development"),
            _temporary: temporary,
        }
    }

    fn block_network_persistence(&self) {
        let network = self.root.join("network");
        std::fs::remove_dir_all(&network).expect("network persistence directory is removable");
        std::fs::write(&network, b"block controller authorization persistence")
            .expect("network persistence path is replaceable with a file");
    }

    fn unblock_network_persistence(&self) {
        let network = self.root.join("network");
        std::fs::remove_file(&network).expect("network persistence blocker is removable");
        std::fs::create_dir_all(&network).expect("network persistence directory is restorable");
    }
}

struct TargetHarness {
    _persistence: tempfile::TempDir,
    handle: PrnsNodeHandle,
    confirmation: mpsc::UnboundedReceiver<RemoteControlTargetPairingConfirmation>,
    endpoint: DestinationHash,
    identity_fingerprint: Vec<u8>,
    server_address: String,
    server: Option<AttachedSupervisor>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), NodeRunError>>>,
}

impl TargetHarness {
    async fn start() -> Self {
        let persistence = tempfile::tempdir().expect("temporary target persistence");
        let identity_secrets = identity_secrets(0xA1, 0xA2);
        let endpoint = identity_secrets.identities().target().endpoint();
        let identity_fingerprint = identity_secrets
            .identities()
            .target()
            .identity_hash()
            .as_bytes()
            .to_vec();
        let (confirmation_tx, confirmation) = mpsc::unbounded_channel();
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: service(identity_secrets),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: move |event, _state| {
                if let PrnsEvent::Message(
                    Message::RemoteControlTargetPairingConfirmationRequired(pairing),
                ) = event
                {
                    let _ignored = confirmation_tx.send(pairing);
                }
            },
            interfaces: ManuallyAttached,
            persistence: NodePersistence::custom_dir(persistence.path())
                .expect("target persistence opens"),
        });
        let handle = node.handle();
        let tcp_server = TcpServer::bind("127.0.0.1:0")
            .await
            .expect("target TCP server binds");
        let server_address = tcp_server
            .local_addr()
            .expect("target TCP server has an address")
            .to_string();
        let server = handle.supervise(tcp_server);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task = tokio::task::spawn_local(node.run_until(async {
            let _ = shutdown_rx.await;
        }));
        Self {
            _persistence: persistence,
            handle,
            confirmation,
            endpoint: endpoint.destination_hash(),
            identity_fingerprint,
            server_address,
            server: Some(server),
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    async fn wait_for_connection(&self) {
        tokio::time::timeout(EXCHANGE_TIMEOUT, async {
            loop {
                if self
                    .handle
                    .interfaces()
                    .iter()
                    .any(|interface| interface.connection.is_online())
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("application controller connects to the target TCP server");
    }

    async fn open_pairing(
        &self,
        permissions: RemoteControlRequestSet,
        pairing_window: DurationMillis,
        attempt_timeout: DurationMillis,
    ) -> RemoteControlPairingOpened {
        let open = OpenRemoteControlPairing {
            target: EgressTarget::AllInterfaces,
            expires_after: RemoteControlPairingExpiresAfter::try_from(pairing_window)
                .expect("pairing window is valid"),
            attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(attempt_timeout)
                .expect("attempt timeout is valid"),
            permissions: RemoteControlPairingPermissions::try_from(permissions)
                .expect("permissions are not empty"),
            public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                b"native aggregate integration test".as_slice(),
            )
            .expect("public application data fits"),
        };
        loop {
            match self.handle.open_remote_control_pairing(open.clone()).await {
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

    async fn next_confirmation(&mut self) -> RemoteControlTargetPairingConfirmation {
        tokio::time::timeout(EXCHANGE_TIMEOUT, self.confirmation.recv())
            .await
            .expect("target confirmation arrives within the bound")
            .expect("target confirmation lane remains open")
    }

    async fn approve(&self, confirmation: RemoteControlTargetPairingConfirmation) {
        assert!(matches!(
            self.handle
                .approve_remote_control_target_pairing(confirmation.approval())
                .await,
            Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit { .. })
        ));
    }

    async fn announce(&self) {
        self.handle
            .announce_now(AnnounceNow {
                destination: self.endpoint,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .expect("target announces its stable endpoint");
        let endpoint = self.endpoint;
        wait_for_snapshot(|snapshot| {
            let LocalHostState::Running { host } = &snapshot.local_host else {
                return false;
            };
            host.destination_identities
                .iter()
                .any(|identity| identity.destination.as_bytes() == endpoint.as_bytes())
        })
        .await;
    }

    async fn close_pairing(&self) {
        assert!(self.handle.close_remote_control_pairing().await.is_ok());
    }

    async fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.teardown();
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let result = tokio::time::timeout(EXCHANGE_TIMEOUT, task)
                .await
                .expect("target stops within the bound")
                .expect("target task joins");
            assert_eq!(result, Ok(()));
        }
    }
}

struct PairingAttempt {
    id: String,
    target_confirmation: RemoteControlTargetPairingConfirmation,
}

#[tokio::test(flavor = "current_thread")]
async fn controlled_tcp_remote_control_matrix_uses_the_public_lifecycle() {
    tokio::task::LocalSet::new()
        .run_until(async {
            rejection_expiry_and_stale_decisions().await;
            permission_refusal().await;
            durable_restart_describe_and_unavailable_target().await;
            persistence_failure().await;
        })
        .await;
}

async fn rejection_expiry_and_stale_decisions() {
    let storage = AppStorage::new();
    let mut target = TargetHarness::start().await;
    create_identity(&storage.root).await;
    start_controller(&storage.root, &target.server_address).await;
    target.wait_for_connection().await;

    let first = begin_pairing(
        &mut target,
        RemoteControlRequestSet::all(),
        NORMAL_PAIRING_WINDOW,
        NORMAL_ATTEMPT_TIMEOUT,
    )
    .await;
    assert!(matches!(
        app_reject(first.id.clone()).await,
        RemoteControlPairingCommandOutcome::Accepted { snapshot }
            if matches!(snapshot.pairing, RemoteControlPairingState::Rejected { .. })
                && snapshot.active_operation.is_none()
    ));
    for duplicate in [
        app_reject(first.id.clone()).await,
        app_approve(first.id.clone()).await,
    ] {
        assert!(matches!(
            duplicate,
            RemoteControlPairingCommandOutcome::Failed {
                stage: RemoteControlPairingFailureStage::Confirmation,
                ..
            }
        ));
        assert!(matches!(
            app_snapshot().await.pairing,
            RemoteControlPairingState::Rejected { .. }
        ));
    }
    assert_links_retired(&target).await;
    target.close_pairing().await;

    let second = begin_pairing(
        &mut target,
        RemoteControlRequestSet::all(),
        NORMAL_PAIRING_WINDOW,
        NORMAL_ATTEMPT_TIMEOUT,
    )
    .await;
    assert_ne!(first.id, second.id);
    let retained = app_snapshot().await;
    assert!(matches!(
        &retained.pairing,
        RemoteControlPairingState::ConfirmationRequired { attempt_id, .. }
            if attempt_id == &second.id
    ));
    for stale_outcome in [
        app_approve(first.id.clone()).await,
        app_reject(first.id).await,
    ] {
        assert!(matches!(
            stale_outcome,
            RemoteControlPairingCommandOutcome::Failed {
                stage: RemoteControlPairingFailureStage::Confirmation,
                ..
            }
        ));
        let after_stale = app_snapshot().await;
        assert_eq!(after_stale.pairing, retained.pairing);
        assert_eq!(after_stale.active_operation, retained.active_operation);
    }
    assert!(matches!(
        app_reject(second.id).await,
        RemoteControlPairingCommandOutcome::Accepted { snapshot }
            if matches!(snapshot.pairing, RemoteControlPairingState::Rejected { .. })
                && snapshot.active_operation.is_none()
    ));
    assert_links_retired(&target).await;
    stop_controller().await;
    target.stop().await;

    let storage = AppStorage::new();
    let mut target = TargetHarness::start().await;
    create_identity(&storage.root).await;
    start_controller(&storage.root, &target.server_address).await;
    target.wait_for_connection().await;
    let expired = begin_pairing(
        &mut target,
        RemoteControlRequestSet::all(),
        DurationMillis(5_000),
        DurationMillis(1_000),
    )
    .await;
    let expired_snapshot = wait_for_snapshot(|snapshot| {
        matches!(snapshot.pairing, RemoteControlPairingState::Expired { .. })
    })
    .await;
    assert!(expired_snapshot.active_operation.is_none());
    assert_links_retired(&target).await;
    assert!(matches!(
        app_reject(expired.id).await,
        RemoteControlPairingCommandOutcome::Failed {
            stage: RemoteControlPairingFailureStage::Confirmation,
            ..
        }
    ));
    stop_controller().await;
    target.stop().await;
}

async fn permission_refusal() {
    let storage = AppStorage::new();
    let mut target = TargetHarness::start().await;
    create_identity(&storage.root).await;
    start_controller(&storage.root, &target.server_address).await;
    target.wait_for_connection().await;
    let attempt = begin_pairing(
        &mut target,
        RemoteControlRequestSet::only(
            personal_rns::remote_control::RemoteControlRequestKind::AnnounceSelf,
        ),
        NORMAL_PAIRING_WINDOW,
        NORMAL_ATTEMPT_TIMEOUT,
    )
    .await;
    approve_pairing(&target, attempt).await;
    target.announce().await;
    assert!(matches!(
        app_describe(target.identity_fingerprint.clone()).await,
        RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Permission,
            ..
        }
    ));
    assert_links_retired(&target).await;
    stop_controller().await;
    target.stop().await;
}

async fn durable_restart_describe_and_unavailable_target() {
    let storage = AppStorage::new();
    let mut target = TargetHarness::start().await;
    create_identity(&storage.root).await;
    start_controller(&storage.root, &target.server_address).await;
    target.wait_for_connection().await;
    let attempt = begin_pairing(
        &mut target,
        RemoteControlRequestSet::all(),
        NORMAL_PAIRING_WINDOW,
        NORMAL_ATTEMPT_TIMEOUT,
    )
    .await;
    approve_pairing(&target, attempt).await;
    assert_links_retired(&target).await;
    stop_controller().await;

    let restored = start_controller(&storage.root, &target.server_address).await;
    target.wait_for_connection().await;
    assert!(restored
        .paired_targets
        .iter()
        .any(|paired| paired.target_identity_fingerprint == target.identity_fingerprint));
    target.announce().await;
    assert!(matches!(
        app_describe(target.identity_fingerprint.clone()).await,
        RemoteControlDescribeOutcome::Described {
            available_requests,
            ..
        } if available_requests == vec![RemoteControlRequestKind::Describe]
    ));
    assert_links_retired(&target).await;

    target.stop().await;
    let unavailable = tokio::time::timeout(
        UNAVAILABLE_TARGET_TIMEOUT,
        app_describe(target.identity_fingerprint.clone()),
    )
    .await
    .expect("unavailable target settles within the aggregate bound");
    assert!(matches!(
        unavailable,
        RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Link,
            ..
        }
    ));
    assert_links_retired(&target).await;
    stop_controller().await;
}

async fn persistence_failure() {
    let storage = AppStorage::new();
    let mut target = TargetHarness::start().await;
    create_identity(&storage.root).await;
    start_controller(&storage.root, &target.server_address).await;
    target.wait_for_connection().await;
    let attempt = begin_pairing(
        &mut target,
        RemoteControlRequestSet::all(),
        NORMAL_PAIRING_WINDOW,
        NORMAL_ATTEMPT_TIMEOUT,
    )
    .await;
    target.approve(attempt.target_confirmation).await;
    storage.block_network_persistence();
    assert!(matches!(
        app_approve(attempt.id).await,
        RemoteControlPairingCommandOutcome::Accepted { snapshot }
            if matches!(snapshot.pairing, RemoteControlPairingState::Persisting { .. })
    ));
    let failed = wait_for_snapshot(|snapshot| {
        matches!(
            snapshot.pairing,
            RemoteControlPairingState::Failed {
                stage: RemoteControlPairingFailureStage::Persistence,
                ..
            }
        )
    })
    .await;
    assert_eq!(failed.runtime, DevelopmentNodeRuntime::Running);
    assert!(failed.active_operation.is_none());
    assert_links_retired(&target).await;
    storage.unblock_network_persistence();
    stop_controller().await;
    target.stop().await;
}

async fn begin_pairing(
    target: &mut TargetHarness,
    permissions: RemoteControlRequestSet,
    pairing_window: DurationMillis,
    attempt_timeout: DurationMillis,
) -> PairingAttempt {
    let opened = target
        .open_pairing(permissions, pairing_window, attempt_timeout)
        .await;
    let candidate_snapshot =
        wait_for_snapshot(|snapshot| !snapshot.pairing_candidates.is_empty()).await;
    let candidate = candidate_snapshot
        .pairing_candidates
        .into_iter()
        .next()
        .expect("application aggregate retains the TCP pairing candidate");
    let candidate_id = candidate.candidate_id;
    assert!(matches!(
        app_initiate(InitiateRemoteControlPairingInput {
            candidate_id: candidate_id.clone(),
            invitation_code: opened.invitation_code.to_string(),
        })
        .await,
        RemoteControlPairingCommandOutcome::Accepted { .. }
    ));
    let confirmation_snapshot = wait_for_snapshot(|snapshot| {
        matches!(
            snapshot.pairing,
            RemoteControlPairingState::ConfirmationRequired { .. }
        )
    })
    .await;
    let RemoteControlPairingState::ConfirmationRequired { attempt_id, .. } =
        confirmation_snapshot.pairing
    else {
        panic!("application aggregate did not project the pairing confirmation");
    };
    let retained = app_snapshot().await;
    assert!(matches!(
        app_initiate(InitiateRemoteControlPairingInput {
            candidate_id,
            invitation_code: opened.invitation_code.to_string(),
        })
        .await,
        RemoteControlPairingCommandOutcome::Busy
    ));
    assert!(matches!(
        app_describe(target.identity_fingerprint.clone()).await,
        RemoteControlDescribeOutcome::Busy
    ));
    let after_conflicts = app_snapshot().await;
    assert_eq!(after_conflicts.pairing, retained.pairing);
    assert_eq!(after_conflicts.active_operation, retained.active_operation);
    let target_confirmation = target.next_confirmation().await;
    assert_eq!(
        attempt_id,
        bytes_hex(
            target_confirmation
                .confirmation()
                .attempt_id()
                .transcript()
                .as_bytes()
        )
    );
    PairingAttempt {
        id: attempt_id,
        target_confirmation,
    }
}

async fn approve_pairing(target: &TargetHarness, attempt: PairingAttempt) {
    target.approve(attempt.target_confirmation).await;
    let attempt_id = attempt.id;
    assert!(matches!(
        app_approve(attempt_id.clone()).await,
        RemoteControlPairingCommandOutcome::Accepted { snapshot }
            if matches!(snapshot.pairing, RemoteControlPairingState::Persisting { .. })
    ));
    for duplicate in [
        app_approve(attempt_id.clone()).await,
        app_reject(attempt_id.clone()).await,
    ] {
        assert!(matches!(
            duplicate,
            RemoteControlPairingCommandOutcome::Failed {
                stage: RemoteControlPairingFailureStage::Confirmation,
                ..
            }
        ));
        assert!(matches!(
            app_snapshot().await.pairing,
            RemoteControlPairingState::Persisting { .. } | RemoteControlPairingState::Paired { .. }
        ));
    }
    wait_for_snapshot(|snapshot| {
        matches!(snapshot.pairing, RemoteControlPairingState::Paired { .. })
    })
    .await;
}

async fn create_identity(storage_root: &Path) {
    let storage_root = storage_root.to_owned();
    assert!(matches!(
        run_blocking(move || app::create_generated_identity(&storage_root)).await,
        IdentityCreationOutcome::Created { .. }
    ));
}

async fn start_controller(storage_root: &Path, server_address: &str) -> DevelopmentNodeSnapshot {
    let storage_root = storage_root.to_owned();
    let server_address = server_address.to_owned();
    let outcome = run_blocking(move || {
        app::start_configured(
            &storage_root,
            DevelopmentNodeStartInput {
                development_tcp_target: Some(server_address),
            },
        )
    })
    .await;
    let DevelopmentNodeStartOutcome::Started { snapshot } = outcome else {
        panic!("application controller did not start: {outcome:?}");
    };
    assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Running);
    snapshot
}

async fn stop_controller() {
    assert_eq!(
        run_blocking(app::stop).await,
        DevelopmentNodeStopOutcome::Stopped
    );
}

async fn app_snapshot() -> DevelopmentNodeSnapshot {
    run_blocking(app::snapshot).await
}

async fn app_initiate(
    input: InitiateRemoteControlPairingInput,
) -> RemoteControlPairingCommandOutcome {
    run_blocking(move || app::initiate(input)).await
}

async fn app_approve(attempt_id: String) -> RemoteControlPairingCommandOutcome {
    run_blocking(move || app::approve(RemoteControlPairingDecisionInput { attempt_id })).await
}

async fn app_reject(attempt_id: String) -> RemoteControlPairingCommandOutcome {
    run_blocking(move || app::reject(RemoteControlPairingDecisionInput { attempt_id })).await
}

async fn app_describe(target_identity_fingerprint: Vec<u8>) -> RemoteControlDescribeOutcome {
    run_blocking(move || {
        app::describe(DescribeRemoteControlTargetInput {
            target_identity_fingerprint,
        })
    })
    .await
}

async fn wait_for_snapshot(
    predicate: impl Fn(&DevelopmentNodeSnapshot) -> bool,
) -> DevelopmentNodeSnapshot {
    tokio::time::timeout(EXCHANGE_TIMEOUT, async {
        loop {
            let snapshot = app_snapshot().await;
            if predicate(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("application projection reaches the expected state")
}

async fn assert_links_retired(target: &TargetHarness) {
    tokio::time::timeout(EXCHANGE_TIMEOUT, async {
        loop {
            let snapshot = app_snapshot().await;
            let app_links = match &snapshot.local_host {
                LocalHostState::Running { host } => Some(host.active_link_count),
                _ => None,
            };
            if app_links == Some(0) && target.handle.link_count().await == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both nodes retire the bounded RemoteControl Link");
}

async fn run_blocking<T>(operation: impl FnOnce() -> T + Send + 'static) -> T
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .expect("application lifecycle call joins")
}

fn identity_secrets(controller_fill: u8, target_fill: u8) -> RemoteControlNodeIdentitySecrets {
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

fn service(identity_secrets: RemoteControlNodeIdentitySecrets) -> RemoteControlService<'static> {
    RemoteControlService::new(
        identity_secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    )
}

fn bytes_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}
