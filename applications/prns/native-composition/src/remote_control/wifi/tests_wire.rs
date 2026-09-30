#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlApplyOutcome as Apply, RemoteControlWifiTransactionStatus as Transaction,
};
use personal_rns::runtime::{
    NodePersistence, RemoteControlHostCommand, RemoteControlHostCommandError,
    RemoteControlHostControls, RemoteControlHostResponse,
};
use std::cell::Cell;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct State {
    transaction: Transaction,
    next_revision: u32,
    confirmed_revision: core::RemoteControlWifiCredentialRevision,
    ready: bool,
    writes: usize,
}

struct Controls(Arc<Mutex<State>>);
impl RemoteControlHostControls for Controls {
    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        let mut state = self.0.lock().expect("fixture state");
        Ok(match command {
            RemoteControlHostCommand::InspectWifiTransaction { .. } => {
                RemoteControlHostResponse::InspectWifiTransaction(state.transaction)
            }
            RemoteControlHostCommand::StageWifiCredentials { station, .. } => {
                assert_eq!(station.ssid(), "trial-network");
                assert_eq!(station.password(), "test-password");
                let revision = core::RemoteControlWifiCredentialRevision::new(state.next_revision)
                    .expect("fixture revision");
                state.next_revision += 1;
                state.writes += 1;
                state.transaction = Transaction::Staged { revision };
                RemoteControlHostResponse::StageWifiCredentials(
                    core::RemoteControlWifiStageOutcome::Staged(revision),
                )
            }
            RemoteControlHostCommand::ActivateWifiCredentials { revision, .. } => {
                assert_eq!(state.transaction, Transaction::Staged { revision });
                state.writes += 1;
                state.transaction = Transaction::AwaitingConfirmation {
                    revision,
                    remaining: core::RemoteControlWifiConfirmationRemaining::new(120)
                        .expect("fixture window"),
                };
                RemoteControlHostResponse::ActivateWifiCredentials(Apply::Scheduled)
            }
            RemoteControlHostCommand::ConfirmWifiCredentials { revision, .. } => {
                if !state.ready {
                    return Err(RemoteControlHostCommandError::Busy);
                }
                assert!(
                    matches!(state.transaction, Transaction::AwaitingConfirmation { revision: pending, .. } if revision == pending)
                );
                state.writes += 1;
                state.confirmed_revision = revision;
                state.transaction = Transaction::Confirmed { revision };
                RemoteControlHostResponse::ConfirmWifiCredentials(Apply::Applied)
            }
            RemoteControlHostCommand::CancelWifiCredentials { revision, .. } => {
                assert!(
                    matches!(state.transaction, Transaction::Staged { revision: pending } | Transaction::AwaitingConfirmation { revision: pending, .. } if revision == pending)
                );
                state.writes += 1;
                state.transaction = Transaction::Confirmed {
                    revision: state.confirmed_revision,
                };
                RemoteControlHostResponse::CancelWifiCredentials(Apply::Scheduled)
            }
            _ => return Err(RemoteControlHostCommandError::Unsupported),
        })
    }
}

fn secrets(controller: u8, target: u8) -> core::RemoteControlNodeIdentitySecrets {
    core::RemoteControlNodeIdentitySecrets::new(
        core::RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [controller; IDENTITY_SECRET_KEY_LEN],
        )),
        core::RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [target; IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .expect("distinct identities")
}

#[tokio::test(flavor = "current_thread")]
async fn wifi_workflow_uses_authenticated_typed_operations_and_confirms_only_when_target_ready() {
    tokio::task::LocalSet::new().run_until(async {
        let directory = tempfile::tempdir().expect("fixture persistence");
        let target_secrets = secrets(0x91, 0x92); let controller_secrets = secrets(0x93, 0x94);
        let target_identity = core::RemoteControlTargetIdentity::new(*target_secrets.identities().target().public_keys());
        let controller_identity = *controller_secrets.identities().controller();
        let endpoint = target_identity.endpoint().destination_hash();
        let mut available = core::RemoteControlRequestSet::only(core::RemoteControlRequestKind::Describe);
        for kind in [core::RemoteControlRequestKind::StageWifiCredentials, core::RemoteControlRequestKind::ActivateWifiCredentials, core::RemoteControlRequestKind::InspectWifiTransaction, core::RemoteControlRequestKind::ConfirmWifiCredentials, core::RemoteControlRequestKind::CancelWifiCredentials] { let _ = available.insert(kind); }
        let grants = [core::RemoteControlControllerGrant::new(controller_identity, core::RemoteControlControllerAuthority::Operator, core::RemoteControlRequestSet::all_operator()).expect("fixture grant")];
        let initial_revision = core::RemoteControlWifiCredentialRevision::new(1).expect("revision");
        let state = Arc::new(Mutex::new(State { transaction: Transaction::Confirmed { revision: initial_revision }, next_revision: 2, confirmed_revision: initial_revision, ready: false, writes: 0 }));
        let target = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: core::RemoteControlService::with_capabilities(target_secrets, core::RemoteControlInitialControllerGrants::Grants(core::RemoteControlControllerGrants::try_from(grants.as_slice()).expect("initial grants")), core::RemoteControlSelfAnnouncement::Unavailable, core::RemoteControlCapabilities::from_requests(available).expect("capabilities")),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0], app_state: Controls(Arc::clone(&state)), storage: GrowableHeap, request_endpoints: request_endpoints![], on_event: |_, _| {}, interfaces: ManuallyAttached,
            persistence: NodePersistence::custom_dir(directory.path().join("target")).expect("target persistence"),
        });
        let target_handle = target.handle();
        let server = TcpServer::bind("127.0.0.1:0").await.expect("server"); let address = server.local_addr().expect("address").to_string(); let _server = target_handle.supervise(server);
        let controller = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None, remote_control: core::RemoteControlService::new(controller_secrets, core::RemoteControlInitialControllerGrants::Nobody, core::RemoteControlSelfAnnouncement::Unavailable),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0], app_state: personal_rns::runtime::NoRemoteControlHostControls, storage: GrowableHeap, request_endpoints: request_endpoints![], on_event: |_, _| {}, interfaces: ManuallyAttached,
            persistence: NodePersistence::custom_dir(directory.path().join("controller")).expect("controller persistence"),
        });
        let controller_handle = controller.handle(); let _client = controller_handle.attach(TcpClientInterface::new(address));
        let target_bytes = target_identity.identity_hash().as_bytes().to_vec();
        let scenario = async {
            controller_handle.set_remote_control_target_access(core::RemoteControlTargetAccess::new(target_identity, core::RemoteControlControllerAuthority::Operator, core::RemoteControlRequestSet::all_operator()).expect("target access")).await.expect("persisted access");
            while !controller_handle.interface_timing_inventory().iter().any(|interface| interface.connection.is_online()) || !target_handle.interface_timing_inventory().iter().any(|interface| interface.connection.is_online()) { tokio::task::yield_now().await; }
            target_handle.announce_now(AnnounceNow { destination: endpoint, target: AnnounceTarget::AllInterfaces, app_data: AnnounceAppData::Registered }).await.expect("announce");
            while controller_handle.route(endpoint).await.is_none() { tokio::task::yield_now().await; }
            let deadline = || Instant::now() + Duration::from_secs(4);
            let start = || Work::Start(core::RemoteControlWifiStation::parse("trial-network", "test-password").expect("fixture credentials"));
            let dispatched = AtomicBool::new(false); let candidate = Cell::new(None);
            let status = execute(&controller_handle, &target_bytes, start(), deadline(), &dispatched, |revision| candidate.set(Some(revision))).await;
            assert!(matches!(status, RemoteWifiStatus::Observed { transaction: RemoteWifiTransaction::AwaitingConfirmation { revision: 2, .. } })); assert_eq!(candidate.get(), Some(2));
            assert_eq!(state.lock().expect("state").writes, 2);
            let _existing = execute(&controller_handle, &target_bytes, start(), deadline(), &AtomicBool::new(false), |_| panic!("must not stage twice")).await;
            assert_eq!(state.lock().expect("state").writes, 2);
            let finish = |revision, decision| Work::Finish { revision: core::RemoteControlWifiCredentialRevision::new(revision).expect("revision"), decision };
            let refused = execute(&controller_handle, &target_bytes, finish(2, RemoteWifiDecision::Keep), deadline(), &AtomicBool::new(false), |_| {}).await;
            assert!(matches!(refused, RemoteWifiStatus::Failed { stage: RemoteManagementFailureStage::Busy, .. }));
            let restored = execute(&controller_handle, &target_bytes, finish(2, RemoteWifiDecision::Restore), deadline(), &AtomicBool::new(false), |_| {}).await;
            assert_eq!(restored, observed(Transaction::Confirmed { revision: initial_revision }));
            let _trial = execute(&controller_handle, &target_bytes, start(), deadline(), &AtomicBool::new(false), |_| {}).await;
            state.lock().expect("state").ready = true;
            let kept = execute(&controller_handle, &target_bytes, finish(3, RemoteWifiDecision::Keep), deadline(), &AtomicBool::new(false), |_| {}).await;
            assert_eq!(kept, RemoteWifiStatus::Observed { transaction: RemoteWifiTransaction::Confirmed { revision: 3 } });
            tokio::time::timeout(Duration::from_secs(1), async { while controller_handle.link_count().await != 0 || target_handle.link_count().await != 0 { tokio::task::yield_now().await; } }).await.expect("links retired");
        };
        tokio::select! {
            _ = target.run() => panic!("target exited"),
            _ = controller.run() => panic!("controller exited"),
            result = tokio::time::timeout(Duration::from_secs(20), scenario) => result.expect("scenario completed"),
        }
    }).await;
}
