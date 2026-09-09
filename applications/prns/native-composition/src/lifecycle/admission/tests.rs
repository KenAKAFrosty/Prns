use super::*;
use crate::test_support::foreign_block_on;
use std::future::Future;
use std::task::{Context, Waker};
fn owner() -> Arc<Supervisor> {
    Arc::new(Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    })
}

#[test]
fn cold_offline_storage_is_native_bootstrapped_and_foreign_calls_create_no_owner() {
    let directory = tempfile::tempdir().expect("temporary storage");
    let root = directory.path().join("prns/development");
    let owner = owner();
    assert!(matches!(
        foreign_block_on(list_contacts_with_supervisor(&owner)),
        ContactListOutcome::DevelopmentUnavailable { .. }
    ));
    assert!(!root.exists());
    assert!(owner.lock_state().application_owner.is_none());
    let native_owner = Arc::clone(&owner);
    let native_root = root.clone();
    assert_eq!(
        std::thread::spawn(move || prepare_native_storage_with_supervisor(
            &native_owner,
            &native_root
        ))
        .join()
        .expect("native background queue"),
        NativeStoragePreparationOutcome::Prepared
    );
    assert!(owner.lock_state().worker.is_none());
    assert!(matches!(
        foreign_block_on(mutation(
            &owner,
            DirectoryRequest::CreateManual {
                destination: [7; 16],
                identity: None,
                alias: Some("  Offline  ".to_owned()),
            }
        )),
        ContactMutationOutcome::Saved { .. }
    ));
    let ContactListOutcome::Listed { contacts } =
        foreign_block_on(list_contacts_with_supervisor(&owner))
    else {
        panic!("offline list");
    };
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].alias.as_deref(), Some("Offline"));
    assert!(owner.lock_state().worker.is_none());
    assert_eq!(
        reset_with_supervisor(&owner, &root),
        DevelopmentNodeStopOutcome::AlreadyStopped
    );
    assert!(!root.exists());
    assert!(matches!(
        foreign_block_on(list_contacts_with_supervisor(&owner)),
        ContactListOutcome::DevelopmentUnavailable { .. }
    ));
    assert!(
        !root.exists(),
        "async calls must not silently reopen after reset"
    );
}

#[test]
fn terminal_failed_generation_allows_offline_mailbox_without_foreign_join() {
    let directory = tempfile::tempdir().expect("temporary storage");
    let root = directory.path().join("prns/development");
    let owner = owner();
    assert_eq!(
        prepare_native_storage_with_supervisor(&owner, &root),
        NativeStoragePreparationOutcome::Prepared
    );
    let (commands, _queue) = mpsc::channel(COMMAND_LANE_CAPACITY);
    let (shutdown, _) = watch::channel(false);
    let (done, done_rx) = std_mpsc::sync_channel(1);
    let join = std::thread::spawn(move || {
        let _ = done.send(Ok(()));
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while !join.is_finished() {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    owner.snapshots.fail(DevelopmentNodeFailure {
        stage: DevelopmentNodeFailureStage::Runtime,
        detail: "terminal generation".to_owned(),
    });
    owner.lock_state().worker = Some(super::super::tests::test_worker(
        commands,
        ShutdownSignal {
            sender: shutdown,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done_rx,
        Some(join),
        root.canonicalize().expect("prepared root"),
    ));
    let request = prns_lxmf::mailbox::MailboxListRequest {
        peer: None,
        direction: None,
        before: None,
        limit: 25,
    };
    let Ok(MailboxResponse::Offline(response)) =
        admit_mailbox(&owner, MailboxRequest::List(request), |response| {
            Command::ListLxmfMessages(request, response)
        })
    else {
        panic!("offline admission after terminal failure");
    };
    assert!(matches!(
        foreign_block_on(bounded_reply(response, LXMF_QUERY_TIMEOUT)),
        Ok(Ok(MailboxReply::Listed { .. }))
    ));
    assert!(owner
        .lock_state()
        .worker
        .as_ref()
        .expect("native cleanup retains handle")
        .join
        .is_some());
    assert_eq!(
        reset_with_supervisor(&owner, &root),
        DevelopmentNodeStopOutcome::Stopped
    );
    assert!(owner.lock_state().worker.is_none());
}

#[test]
fn foreign_offline_timeout_needs_no_tokio_runtime() {
    let (_response, receiver) = oneshot::channel::<()>();
    let started = Instant::now();
    assert!(foreign_block_on(bounded_reply(receiver, Duration::from_millis(15))).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn async_admission_never_waits_for_a_native_transition() {
    let owner = owner();
    let _transition = owner.lock_state();
    let started = Instant::now();
    assert!(matches!(
        foreign_block_on(list_contacts_with_supervisor(&owner)),
        ContactListOutcome::DevelopmentUnavailable { .. }
    ));
    assert!(matches!(
        foreign_block_on(describe_with_supervisor(
            &owner,
            DescribeRemoteControlTargetInput {
                target_identity_fingerprint: vec![1; 16]
            }
        )),
        RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Node,
            ..
        }
    ));
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn admitted_contact_write_commits_after_foreign_caller_drop_and_reset_drains_it() {
    let directory = tempfile::tempdir().expect("temporary storage");
    let root = directory.path().join("prns/development");
    let owner = owner();
    assert_eq!(
        prepare_native_storage_with_supervisor(&owner, &root),
        NativeStoragePreparationOutcome::Prepared
    );
    let (entered, entered_rx) = std_mpsc::sync_channel(1);
    let (release, release_rx) = std_mpsc::sync_channel(1);
    owner
        .lock_state()
        .application_owner
        .as_ref()
        .expect("prepared owner")
        .admit_test_barrier(entered, release_rx)
        .expect("hold database");
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("database is held");
    let mut call = Box::pin(mutation(
        &owner,
        DirectoryRequest::CreateManual {
            destination: [3; 16],
            identity: Some([4; 16]),
            alias: None,
        },
    ));
    assert!(call
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    drop(call);
    release.send(()).expect("resume owner");
    let ContactListOutcome::Listed { contacts } =
        foreign_block_on(list_contacts_with_supervisor(&owner))
    else {
        panic!("read committed mutation");
    };
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].destination, [3; 16]);
    // The next admitted write remains behind another held job; reset must
    // drain it even though the foreign observer has already gone away.
    let (entered, entered_rx) = std_mpsc::sync_channel(1);
    let (release, release_rx) = std_mpsc::sync_channel(1);
    owner
        .lock_state()
        .application_owner
        .as_ref()
        .expect("prepared owner")
        .admit_test_barrier(entered, release_rx)
        .expect("hold database");
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("database is held");
    let response = admit_directory(
        &owner,
        DirectoryRequest::SetAlias {
            destination: [3; 16],
            alias: Some("Retained".to_owned()),
        },
    )
    .expect("admitted write");
    drop(response);
    let reset_owner = Arc::clone(&owner);
    let reset_root = root.clone();
    let reset = std::thread::spawn(move || reset_with_supervisor(&reset_owner, &reset_root));
    // Synchronize on reset holding the supervisor rather than guessing a sleep.
    let deadline = Instant::now() + Duration::from_secs(1);
    while owner.state.try_lock().is_ok() {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!reset.is_finished());
    release.send(()).expect("resume database");
    assert_eq!(
        reset.join().expect("reset joins owner"),
        DevelopmentNodeStopOutcome::AlreadyStopped
    );
    assert!(!root.exists());
}

#[test]
fn async_describe_held_request_releases_on_native_stop_and_caller_drop() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    for caller_leaves in [false, true] {
        let owner = owner();
        owner
            .snapshots
            .begin_generation(PrimaryIdentityState::Missing);
        owner.snapshots.set_runtime(DevelopmentNodeRuntime::Running);
        let (commands, mut queue) = mpsc::channel(COMMAND_LANE_CAPACITY);
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let (done, done_rx) = std_mpsc::sync_channel(1);
        let (entered, entered_rx) = std_mpsc::sync_channel(1);
        let dropped = Arc::new(AtomicBool::new(false));
        let actor_drop = Arc::clone(&dropped);
        let actor_owner = Arc::clone(&owner);
        let join = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("existing node runtime");
            runtime.block_on(async {
                let Some(Command::Describe(command)) = queue.recv().await else {
                    panic!("admitted describe");
                };
                run_describe_command(
                    command,
                    &actor_owner.snapshots,
                    &actor_owner.operation_admitted,
                    &mut shutdown_rx,
                    |_| async move {
                        let _held = Dropped(actor_drop);
                        entered.send(()).expect("held request entered");
                        std::future::pending().await
                    },
                )
                .await;
            });
            done.send(Ok(())).expect("worker completion");
        });
        owner.lock_state().worker = Some(super::super::tests::test_worker(
            commands,
            ShutdownSignal {
                sender: shutdown,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done_rx,
            Some(join),
            PathBuf::from("/unused"),
        ));
        let mut query = Box::pin(describe_with_supervisor(
            &owner,
            DescribeRemoteControlTargetInput {
                target_identity_fingerprint: vec![9; 16],
            },
        ));
        assert!(query
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("held network future");
        if caller_leaves {
            drop(query);
            let deadline = Instant::now() + Duration::from_secs(1);
            while !dropped.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            let mut state = owner.lock_state();
            assert!(matches!(
                stop_locked(&owner, &mut state),
                DevelopmentNodeStopOutcome::Stopped | DevelopmentNodeStopOutcome::AlreadyStopped
            ));
        } else {
            let outcome = {
                let mut state = owner.lock_state();
                stop_locked(&owner, &mut state)
            };
            assert_eq!(outcome, DevelopmentNodeStopOutcome::Stopped);
            assert!(matches!(
                foreign_block_on(query),
                RemoteControlDescribeOutcome::Failed {
                    stage: RemoteControlDescribeFailureStage::Node,
                    ..
                }
            ));
        }
        assert!(dropped.load(Ordering::Acquire));
        assert!(!owner.operation_admitted.load(Ordering::Acquire));
    }
}

#[test]
fn typed_contact_input_cannot_bypass_shared_native_size_bound() {
    let directory = tempfile::tempdir().expect("temporary storage");
    let root = directory.path().join("prns/development");
    let owner = owner();
    assert_eq!(
        prepare_native_storage_with_supervisor(&owner, &root),
        NativeStoragePreparationOutcome::Prepared
    );
    assert!(matches!(
        foreign_block_on(mutation(
            &owner,
            DirectoryRequest::CreateManual {
                destination: [1; 16],
                identity: None,
                alias: Some("x".repeat(crate::input::MAX_INPUT_BYTES + 1)),
            }
        )),
        ContactMutationOutcome::DevelopmentUnavailable { .. }
    ));
    assert_eq!(
        foreign_block_on(list_contacts_with_supervisor(&owner)),
        ContactListOutcome::Listed { contacts: vec![] }
    );
}
