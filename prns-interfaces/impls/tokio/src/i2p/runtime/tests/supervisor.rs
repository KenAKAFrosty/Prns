use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use prns_core::interfaces::i2p::core;
use prns_core::interfaces::{ConfiguredInterfacePolicy, ConnectionState, InterfaceStatus};
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

use super::super::super::{
    I2pInterface, I2pInterfaceName, I2pPeers, I2pReachability, I2pRetryPolicy, I2pRuntimeConfig,
};
use super::fake_sam::{
    public_destination, FakeSamBridge, FakeSamError, RecordedSessionDestination,
};
use crate::i2p::I2pDestinationKeyPath;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "prns-i2p-supervisor-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("the unique test directory is created");
        Self(path)
    }

    fn key_path(&self) -> I2pDestinationKeyPath {
        I2pDestinationKeyPath::new(self.0.join("i2p.key"))
            .expect("the test key path has a file name")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn retry_policy() -> I2pRetryPolicy {
    I2pRetryPolicy::new(
        Duration::from_millis(1),
        Duration::from_millis(1),
        Duration::from_millis(1),
    )
    .expect("the test retry policy is non-zero")
}

fn config(reachability: I2pReachability) -> I2pRuntimeConfig {
    I2pRuntimeConfig {
        name: I2pInterfaceName::new("Test I2P").expect("the test name is valid"),
        peers: I2pPeers::empty(),
        reachability,
        policy: core::configured_policy(ConfiguredInterfacePolicy::default()),
        retry: retry_policy(),
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the runtime reaches the expected state");
}

#[tokio::test]
async fn connectable_supervisor_persists_identity_accepts_members_and_recovers_listener() {
    let directory = TestDirectory::new();
    let key_path = directory.key_path();
    let bridge = FakeSamBridge::new();
    let interface = I2pInterface::new(
        bridge.clone(),
        config(I2pReachability::Connectable {
            key_path: key_path.clone(),
        }),
    );
    let status = interface.status();
    let (fleet, _tail) = Fleet::detached(interface.id());
    let task = tokio::spawn(async move { interface.run(fleet).await });

    wait_until(|| status.initial_attempts_complete() && status.published_destination().is_some())
        .await;
    let published = status
        .published_destination()
        .expect("the endpoint publishes its stable address");
    let persisted = fs::read_to_string(key_path.as_path()).expect("the endpoint key is persisted");

    assert_eq!(status.connection(), ConnectionState::Connected);
    assert_eq!(bridge.destination_generations(), 1);
    assert_eq!(bridge.accept_session_count(), 1);
    assert_eq!(
        bridge.session_destinations(),
        vec![RecordedSessionDestination::Persistent]
    );

    let _accepted = bridge.inject_accepted(public_destination(0x82));
    wait_until(|| status.member_vitals().len() == 1).await;

    bridge.fail_latest_accept(FakeSamError::SessionLost);
    wait_until(|| bridge.accept_session_count() == 2).await;

    assert_eq!(bridge.destination_generations(), 1);
    assert_eq!(
        bridge.session_destinations(),
        vec![
            RecordedSessionDestination::Persistent,
            RecordedSessionDestination::Persistent
        ]
    );
    assert_eq!(status.published_destination(), Some(published));
    assert_eq!(
        fs::read_to_string(key_path.as_path()).expect("the endpoint key remains readable"),
        persisted
    );

    status.disable();
    wait_until(|| status.member_vitals().is_empty()).await;
    assert_eq!(status.connection(), ConnectionState::Disabled);

    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn outbound_only_interface_without_peers_becomes_quiescent_after_initialization() {
    let bridge = FakeSamBridge::new();
    let interface = I2pInterface::new(bridge, config(I2pReachability::OutboundOnly));
    let status = interface.status();
    let (fleet, _tail) = Fleet::detached(interface.id());
    let task = tokio::spawn(async move { interface.run(fleet).await });

    wait_until(|| status.initial_attempts_complete()).await;
    assert_eq!(status.connection(), ConnectionState::Disconnected);
    assert!(status.member_vitals().is_empty());

    status.disable();
    task.abort();
    let _ = task.await;
}
