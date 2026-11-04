#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;
use prns_groups::{fixture, Client, Error, Job, StoreSession, Submit};
use prns_groups_service::{ClockReading, Timestamp};
use prns_host::IdentityHash;
use std::sync::{Arc, Mutex};
use support::PublicationClient;
use support::{clock, Worker, NOW};

struct Captured(Mutex<Option<Job>>, prns_groups::InputBudget);
impl Submit for Captured {
    fn input_budget(&self) -> &prns_groups::InputBudget {
        &self.1
    }
    fn submit(&self, job: Job) -> Result<(), Error> {
        let mut pending = self.0.lock().unwrap();
        if pending.is_some() {
            return Err(Error::Busy);
        }
        *pending = Some(job);
        Ok(())
    }
}

#[tokio::test]
async fn dropped_waiter_commits_once_and_retry_ignores_later_clock() {
    let identities = [IdentityHash::new([11; 16]), IdentityHash::new([22; 16])];
    let fixtures = fixture::generate("Recovery", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("group.redb");
    let database = redb::Database::create(&path).unwrap();
    let provisioned = fixture::load_and_install(&database, &fixtures[0], identities[0]).unwrap();
    let group = provisioned.group;
    let original_scope = prns_groups::OriginalScope::Control {
        control: provisioned.control,
        incarnation: provisioned.incarnation,
    };
    let mut session = StoreSession::open(&database, provisioned).unwrap();
    let lane = Arc::new(Captured(Mutex::new(None), Default::default()));
    let client = Client::new(lane.clone());
    let request = support::text_request(
        group,
        original_scope,
        prns_groups_core::retry::RequestToken::new(NOW, [7; 20])
            .unwrap()
            .into_bytes(),
        "survives a lost result".into(),
        clock(),
    );
    let waiter = client.admit_request(request.clone()).unwrap();
    assert!(matches!(
        client.admit_request(request.clone()),
        Err(prns_groups_core::ApiError::Busy)
    ));
    drop(waiter);
    lane.0.lock().unwrap().take().unwrap().execute(&mut session);
    drop(session);
    drop(database);
    let worker = Worker::open(path, fixtures[0].clone(), identities[0]);
    let before = worker
        .client
        .messages(group, None, 100, clock())
        .await
        .unwrap();
    assert_eq!(before.messages.len(), 1);
    let mut retry = request;
    retry.1.clock = ClockReading::Unknown;
    assert_eq!(
        worker.client.publish_request(retry).await.unwrap(),
        before.messages[0].id
    );
    assert_eq!(
        worker
            .client
            .messages(group, None, 100, clock())
            .await
            .unwrap(),
        before
    );
    worker.stop();
}

#[test]
fn process_child() {
    let Some(directory) = std::env::var_os("PRNS_GROUP_RUNNER_CRASH_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let bytes = std::fs::read(directory.join("fixture.json")).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let worker = Worker::open(
            directory.join("group.redb"),
            bytes,
            IdentityHash::new([11; 16]),
        );
        let initial = worker.client.network_config().await.unwrap();
        let group = initial.group;
        let original_scope = prns_groups::OriginalScope::Control {
            control: initial.control,
            incarnation: initial.incarnation,
        };
        let id = worker
            .client
            .publish_request(support::text_request(
                group,
                original_scope,
                prns_groups_core::retry::RequestToken::new(NOW, [8; 20])
                    .unwrap()
                    .into_bytes(),
                "durably accepted before process exit".into(),
                clock(),
            ))
            .await
            .unwrap();
        std::fs::write(directory.join("accepted-id"), id.0).unwrap();
        // Exit with the database worker live: no Rust shutdown or Database Drop.
        std::process::exit(73);
    });
}

#[tokio::test]
async fn accepted_send_survives_abrupt_process_exit_with_exact_request_identity() {
    let identities = [IdentityHash::new([11; 16]), IdentityHash::new([22; 16])];
    let fixtures = fixture::generate("Process recovery", &identities, Timestamp(NOW)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("fixture.json"), &fixtures[0]).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process_child", "--nocapture"])
        .env("PRNS_GROUP_RUNNER_CRASH_DIR", directory.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    let id = std::fs::read(directory.path().join("accepted-id")).unwrap();
    let worker = Worker::open(
        directory.path().join("group.redb"),
        fixtures[0].clone(),
        identities[0],
    );
    let initial = worker.client.network_config().await.unwrap();
    let group = initial.group;
    let original_scope = prns_groups::OriginalScope::Control {
        control: initial.control,
        incarnation: initial.incarnation,
    };
    let result = worker
        .client
        .publish_request(support::text_request(
            group,
            original_scope,
            prns_groups_core::retry::RequestToken::new(NOW, [8; 20])
                .unwrap()
                .into_bytes(),
            "durably accepted before process exit".into(),
            ClockReading::Unknown,
        ))
        .await
        .unwrap();
    assert_eq!(result.0.as_slice(), id);
    assert_eq!(
        worker
            .client
            .messages(group, None, 100, clock())
            .await
            .unwrap()
            .messages
            .len(),
        1
    );
    worker.stop();
}
