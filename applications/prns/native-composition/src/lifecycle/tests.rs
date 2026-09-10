use super::*;
use crate::test_support::foreign_block_on;

fn test_describe_command(
    deadline: tokio::time::Instant,
) -> (
    DescribeCommand,
    oneshot::Sender<()>,
    oneshot::Receiver<RemoteControlDescribeOutcome>,
) {
    let (response, receiver) = oneshot::channel();
    let (caller, cancelled) = oneshot::channel();
    (
        DescribeCommand {
            input: DescribeRemoteControlTargetInput {
                target_identity_fingerprint: vec![1; 16],
            },
            response,
            deadline,
            caller: cancelled,
        },
        caller,
        receiver,
    )
}

struct DescribeDropProbe(Arc<AtomicBool>);

impl Drop for DescribeDropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug)]
enum DescribeInterruption {
    Deadline,
    CallerLeft,
    Shutdown,
}

#[tokio::test(start_paused = true)]
async fn describe_readiness_wait_releases_operation_and_admission_on_each_interruption() {
    for interruption in [
        DescribeInterruption::Deadline,
        DescribeInterruption::CallerLeft,
        DescribeInterruption::Shutdown,
    ] {
        let snapshots = Arc::new(SnapshotStore::new());
        snapshots.set_runtime(DevelopmentNodeRuntime::Running);
        let admitted = Arc::new(AtomicBool::new(true));
        let dropped = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let route_ready = Arc::new(AtomicBool::new(false));
        let emitted = Arc::new(AtomicBool::new(false));
        let started = tokio::time::Instant::now();
        let (command, caller, mut response) = test_describe_command(started + COMMAND_TIMEOUT);
        let mut caller = Some(caller);
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let running_snapshots = Arc::clone(&snapshots);
        let running_admitted = Arc::clone(&admitted);
        let running_dropped = Arc::clone(&dropped);
        let running_entered = Arc::clone(&entered);
        let running_ready = Arc::clone(&route_ready);
        let running_emitted = Arc::clone(&emitted);
        let actor = tokio::spawn(async move {
            run_describe_command(
                command,
                &running_snapshots,
                &running_admitted,
                &mut shutdown_rx,
                |_input| async {
                    let _probe = DescribeDropProbe(running_dropped);
                    running_snapshots.update(|snapshot| {
                        snapshot.active_operation = Some(DevelopmentNodeOperation {
                            kind: DevelopmentNodeOperationKind::Describe,
                            started_at_millis: 0,
                        });
                    });
                    running_entered.notify_one();
                    crate::remote_control::tests::wait_for_route_with_probe(
                        started + COMMAND_TIMEOUT,
                        &running_ready,
                        &running_emitted,
                    )
                    .await
                },
            )
            .await
        });
        entered.notified().await;
        assert!(admitted.load(Ordering::Acquire));
        assert!(snapshots.read().active_operation.is_some());
        match interruption {
            DescribeInterruption::Deadline => {
                tokio::time::advance(COMMAND_TIMEOUT - Duration::from_millis(1)).await;
                tokio::task::yield_now().await;
                assert!(!actor.is_finished());
                tokio::time::advance(Duration::from_millis(1)).await;
            }
            DescribeInterruption::CallerLeft | DescribeInterruption::Shutdown => {
                tokio::time::advance(Duration::from_secs(6)).await;
                tokio::task::yield_now().await;
                assert!(
                    !actor.is_finished(),
                    "readiness keeps the original deadline"
                );
                if matches!(interruption, DescribeInterruption::CallerLeft) {
                    drop(caller.take());
                } else {
                    shutdown.send(true).expect("request Stop");
                }
            }
        }
        let stopping = actor.await.expect("Describe actor settles");
        assert_eq!(
            stopping,
            matches!(interruption, DescribeInterruption::Shutdown)
        );
        let expected_stage = if stopping {
            RemoteControlDescribeFailureStage::Node
        } else {
            RemoteControlDescribeFailureStage::Timeout
        };
        assert!(matches!(
            response.try_recv().expect("caller receives the terminal result"),
            RemoteControlDescribeOutcome::Failed { stage, .. } if stage == expected_stage
        ));
        assert!(dropped.load(Ordering::Acquire), "{interruption:?}");
        assert!(!admitted.load(Ordering::Acquire), "{interruption:?}");
        assert!(
            snapshots.read().active_operation.is_none(),
            "{interruption:?}"
        );
        route_ready.store(true, Ordering::Release);
        tokio::time::advance(COMMAND_TIMEOUT).await;
        assert!(!emitted.load(Ordering::Acquire), "{interruption:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn describe_expired_or_abandoned_in_queue_never_starts_network_work() {
    for abandoned in [false, true] {
        let snapshots = SnapshotStore::new();
        let admitted = AtomicBool::new(true);
        let (command, caller, mut response) =
            test_describe_command(tokio::time::Instant::now() + COMMAND_TIMEOUT);
        let mut caller = Some(caller);
        if abandoned {
            drop(caller.take());
        } else {
            tokio::time::advance(COMMAND_TIMEOUT).await;
        }
        let (_shutdown, mut shutdown_rx) = watch::channel(false);
        let invoked = AtomicBool::new(false);
        assert!(
            !run_describe_command(command, &snapshots, &admitted, &mut shutdown_rx, |_input| {
                invoked.store(true, Ordering::Release);
                std::future::ready(RemoteControlDescribeOutcome::Busy)
            },)
            .await
        );
        assert!(!invoked.load(Ordering::Acquire));
        assert!(!admitted.load(Ordering::Acquire));
        assert!(matches!(
            response.try_recv().expect("expired command settles"),
            RemoteControlDescribeOutcome::Failed {
                stage: RemoteControlDescribeFailureStage::Timeout,
                ..
            }
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn describe_queue_time_consumes_the_network_wait_budget() {
    let snapshots = SnapshotStore::new();
    let admitted = AtomicBool::new(true);
    let started = tokio::time::Instant::now();
    let (command, _caller, mut response) = test_describe_command(started + COMMAND_TIMEOUT);
    let (_shutdown, mut shutdown_rx) = watch::channel(false);
    let queued_for = Duration::from_secs(15);
    tokio::time::advance(queued_for).await;
    let entered = tokio::sync::Notify::new();
    let route_ready = AtomicBool::new(false);
    let emitted = AtomicBool::new(false);
    let (stopping, ()) = tokio::join!(
        run_describe_command(
            command,
            &snapshots,
            &admitted,
            &mut shutdown_rx,
            |_input| async {
                entered.notify_one();
                crate::remote_control::tests::wait_for_route_with_probe(
                    started + COMMAND_TIMEOUT,
                    &route_ready,
                    &emitted,
                )
                .await
            }
        ),
        async {
            entered.notified().await;
            tokio::time::advance(COMMAND_TIMEOUT - queued_for - Duration::from_millis(1)).await;
            tokio::task::yield_now().await;
            assert!(admitted.load(Ordering::Acquire));
            tokio::time::advance(Duration::from_millis(1)).await;
        },
    );
    assert!(!stopping);
    assert_eq!(tokio::time::Instant::now(), started + COMMAND_TIMEOUT);
    assert!(!admitted.load(Ordering::Acquire));
    assert!(!emitted.load(Ordering::Acquire));
    assert!(matches!(
        response
            .try_recv()
            .expect("original deadline settles the command"),
        RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Timeout,
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn describe_shutdown_has_priority_over_expiry_and_ready_network_work() {
    let snapshots = SnapshotStore::new();
    let admitted = AtomicBool::new(true);
    let (command, _caller, mut response) = test_describe_command(tokio::time::Instant::now());
    let (_shutdown, mut shutdown_rx) = watch::channel(true);
    let invoked = AtomicBool::new(false);
    assert!(
        run_describe_command(command, &snapshots, &admitted, &mut shutdown_rx, |_input| {
            invoked.store(true, Ordering::Release);
            std::future::ready(RemoteControlDescribeOutcome::Busy)
        },)
        .await
    );
    assert!(!invoked.load(Ordering::Acquire));
    assert!(!admitted.load(Ordering::Acquire));
    assert!(matches!(
        response
            .try_recv()
            .expect("shutdown replies without waiting"),
        RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Node,
            ..
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn describe_busy_does_not_clear_a_pairing_operation_or_retained_announcement() {
    let snapshots = SnapshotStore::new();
    snapshots.update(|snapshot| {
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Pairing,
            started_at_millis: 0,
        });
        snapshot.last_announcement = Some(RemoteControlAnnounceOperation {
            operation_id: 1,
            target_identity_fingerprint: vec![1; 16],
            status: RemoteControlAnnounceStatus::Pending,
        });
    });
    let before = snapshots.read();
    let admitted = AtomicBool::new(true);
    let (command, _caller, mut response) =
        test_describe_command(tokio::time::Instant::now() + COMMAND_TIMEOUT);
    let (_shutdown, mut shutdown_rx) = watch::channel(false);
    assert!(
        !run_describe_command(command, &snapshots, &admitted, &mut shutdown_rx, |_input| {
            std::future::ready(RemoteControlDescribeOutcome::Busy)
        },)
        .await
    );
    assert_eq!(
        response.try_recv().expect("busy reply"),
        RemoteControlDescribeOutcome::Busy
    );
    assert_eq!(snapshots.read(), before);
    assert!(!admitted.load(Ordering::Acquire));
}

#[test]
fn worker_terminal_paths_do_not_leave_an_announcement_pending() {
    for result in [
        Ok(()),
        Err((DevelopmentNodeStopStage::Node, "unexpected exit".to_owned())),
    ] {
        let snapshots = SnapshotStore::new();
        snapshots.set_runtime(DevelopmentNodeRuntime::Running);
        snapshots.update(|snapshot| {
            snapshot.lxmf.state = crate::contract::LxmfHealthState::Ready;
            snapshot.last_announcement = Some(RemoteControlAnnounceOperation {
                operation_id: 1,
                target_identity_fingerprint: vec![1; 16],
                status: RemoteControlAnnounceStatus::Pending,
            });
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::AnnounceSelf,
                started_at_millis: 0,
            });
        });
        let admitted = AtomicBool::new(true);
        settle_worker_result(&admitted, &snapshots, &result);
        assert!(matches!(
            snapshots
                .read()
                .last_announcement
                .map(|operation| operation.status),
            Some(RemoteControlAnnounceStatus::OutcomeUnknown {
                reason: crate::contract::RemoteControlAnnounceUnknownReason::NodeStopped
            })
        ));
        assert!(snapshots.read().active_operation.is_none());
        if result.is_ok() {
            let stopped = snapshots.read();
            assert_eq!(stopped.runtime, DevelopmentNodeRuntime::Stopped);
            assert_eq!(
                stopped.lxmf.state,
                crate::contract::LxmfHealthState::Stopped
            );
            assert!(matches!(stopped.local_host, LocalHostState::Stopped { .. }));
            assert!(stopped.paired_targets.is_empty());
        }
        assert!(!admitted.load(Ordering::Acquire));
    }
}

#[test]
fn announcement_admission_cannot_publish_after_stop_owns_the_supervisor() {
    let supervisor = Arc::new(Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    });
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    let (commands, mut command_rx) = mpsc::channel(1);
    let (sender, _shutdown_rx) = watch::channel(false);
    let (done_tx, done) = std_mpsc::channel();
    supervisor.lock_state().worker = Some(test_worker(
        commands,
        ShutdownSignal {
            sender,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done,
        None,
        PathBuf::from("/tmp/prns/announcement-test"),
    ));

    let accepted = foreign_block_on(admission::announce_target_with_supervisor(
        &supervisor,
        AnnounceRemoteControlTargetInput {
            target_identity_fingerprint: vec![1; 16],
        },
    ));
    assert!(matches!(
        accepted,
        RemoteControlAnnounceOutcome::Accepted { .. }
    ));
    assert!(matches!(
        foreign_block_on(admission::announce_target_with_supervisor(
            &supervisor,
            AnnounceRemoteControlTargetInput {
                target_identity_fingerprint: vec![1; 16]
            }
        )),
        RemoteControlAnnounceOutcome::Busy
    ));
    assert!(matches!(
        command_rx.try_recv(),
        Ok(Command::AnnounceSelf(_))
    ));
    // Model a consumed command that never settles before priority shutdown.
    let mut state = supervisor.lock_state();
    supervisor.snapshots.begin_stop(2);
    let blocked = Arc::clone(&supervisor);
    let (started_tx, started_rx) = std_mpsc::channel();
    let (result_tx, result_rx) = std_mpsc::sync_channel(1);
    let submit = std::thread::spawn(move || {
        started_tx.send(()).expect("submission thread started");
        let outcome = foreign_block_on(admission::announce_target_with_supervisor(
            &blocked,
            AnnounceRemoteControlTargetInput {
                target_identity_fingerprint: vec![2; 16],
            },
        ));
        result_tx.send(outcome).expect("publish admission result");
    });
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("submission reached held supervisor");
    let rejected_during_stop = result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("submission returns while lifecycle lock is held");
    submit.join().expect("completed submission thread joins");
    assert_eq!(rejected_during_stop, RemoteControlAnnounceOutcome::Busy);
    done_tx.send(Ok(())).expect("worker shutdown completed");
    assert_eq!(
        stop_locked(&supervisor, &mut state),
        DevelopmentNodeStopOutcome::Stopped
    );
    drop(state);
    assert!(command_rx.try_recv().is_err());
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Stopped
    );
    assert!(supervisor.snapshots.read().active_operation.is_none());
    assert!(matches!(
        supervisor
            .snapshots
            .read()
            .last_announcement
            .map(|operation| operation.status),
        Some(RemoteControlAnnounceStatus::OutcomeUnknown { .. })
    ));
}

pub(super) fn test_worker(
    commands: mpsc::Sender<Command>,
    shutdown: ShutdownSignal,
    done: std_mpsc::Receiver<WorkerResult>,
    join: Option<JoinHandle<()>>,
    storage_root: PathBuf,
) -> Worker {
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let bluetooth_owner_key = AppleBluetoothOwnerKey {
        storage_root: storage_root.clone(),
        preparation: AppleBluetoothPreparation::WithoutRestoration,
        identity: BleIdentity::new([0xa5; 16]),
    };
    Worker {
        runtime: Arc::new(OnceLock::new()),
        commands,
        shutdown,
        done,
        join,
        incomplete_stop: None,
        storage_root,
        #[cfg(all(feature = "apple", target_os = "ios"))]
        bluetooth_owner_key,
    }
}

fn test_supervisor_state(
    worker: Option<Worker>,
    application_owner: Option<DevelopmentStoreOwner>,
    identity_owner: Option<IdentityOwner>,
) -> SupervisorState {
    SupervisorState {
        worker,
        application_owner,
        identity_owner,
        #[cfg(all(feature = "apple", target_os = "ios"))]
        pending_apple_bluetooth: None,
    }
}

#[derive(Default)]
struct PendingProofNetwork {
    send_entered: tokio::sync::Notify,
    release_send: tokio::sync::Notify,
    send_count: std::sync::atomic::AtomicUsize,
}

struct BlockingInsertSubmitter {
    inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
    insert_entered: tokio::sync::Notify,
    release_insert: tokio::sync::Notify,
}

impl BlockingInsertSubmitter {
    fn new(inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>) -> Self {
        Self {
            inner,
            insert_entered: tokio::sync::Notify::new(),
            release_insert: tokio::sync::Notify::new(),
        }
    }
}

impl prns_lxmf::mailbox::MailboxSubmitter for BlockingInsertSubmitter {
    fn submit(
        &self,
        request: prns_lxmf::mailbox::MailboxRequest,
    ) -> prns_lxmf::mailbox::MailboxFuture<'_> {
        Box::pin(async move {
            if matches!(
                &request,
                prns_lxmf::mailbox::MailboxRequest::InsertOutbound(_)
            ) {
                self.insert_entered.notify_one();
                self.release_insert.notified().await;
            }
            self.inner.submit(request).await
        })
    }
}

struct ResetCompletionSubmitter {
    inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
}

impl prns_lxmf::mailbox::MailboxSubmitter for ResetCompletionSubmitter {
    fn submit(
        &self,
        request: prns_lxmf::mailbox::MailboxRequest,
    ) -> prns_lxmf::mailbox::MailboxFuture<'_> {
        Box::pin(async move {
            if matches!(
                &request,
                prns_lxmf::mailbox::MailboxRequest::CompleteAttempt { .. }
            ) {
                return Err(prns_lxmf::mailbox::MailboxFailure::ResetRequired(
                    "mailbox record became unreadable".to_owned(),
                ));
            }
            self.inner.submit(request).await
        })
    }
}

struct TransientListFailureSubmitter {
    inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
    remaining_failures: std::sync::atomic::AtomicUsize,
    list_submissions: std::sync::atomic::AtomicUsize,
}

impl prns_lxmf::mailbox::MailboxSubmitter for TransientListFailureSubmitter {
    fn submit(
        &self,
        request: prns_lxmf::mailbox::MailboxRequest,
    ) -> prns_lxmf::mailbox::MailboxFuture<'_> {
        Box::pin(async move {
            if matches!(&request, prns_lxmf::mailbox::MailboxRequest::List(_)) {
                self.list_submissions.fetch_add(1, Ordering::AcqRel);
                let remaining = self.remaining_failures.fetch_update(
                    Ordering::AcqRel,
                    Ordering::Acquire,
                    |remaining| remaining.checked_sub(1),
                );
                if let Ok(remaining) = remaining {
                    return Err(if remaining == 2 {
                        prns_lxmf::mailbox::MailboxFailure::Busy
                    } else {
                        prns_lxmf::mailbox::MailboxFailure::Unavailable(
                            "transient mailbox read failure".to_owned(),
                        )
                    });
                }
            }
            self.inner.submit(request).await
        })
    }
}

impl prns_lxmf::direct::DirectNetwork for PendingProofNetwork {
    fn has_route(
        &self,
        _destination: [u8; 16],
    ) -> prns_lxmf::direct::DirectNetworkFuture<'_, bool> {
        Box::pin(async { true })
    }

    fn request_path(
        &self,
        _destination: [u8; 16],
    ) -> prns_lxmf::direct::DirectNetworkFuture<'_, Result<(), prns_lxmf::direct::DirectSendFailure>>
    {
        Box::pin(async { Ok(()) })
    }

    fn establish_link(
        &self,
        _destination: [u8; 16],
    ) -> prns_lxmf::direct::DirectNetworkFuture<
        '_,
        Result<[u8; 16], prns_lxmf::direct::DirectSendFailure>,
    > {
        Box::pin(async { Ok([0xa5; 16]) })
    }

    fn send_link_packet(
        &self,
        _link: [u8; 16],
        _complete_wire: Vec<u8>,
    ) -> prns_lxmf::direct::DirectNetworkFuture<
        '_,
        Result<prns_lxmf::direct::DirectDeliveryReceipt, prns_lxmf::direct::DirectSendFailure>,
    > {
        Box::pin(async move {
            self.send_count.fetch_add(1, Ordering::AcqRel);
            self.send_entered.notify_one();
            self.release_send.notified().await;
            Ok(prns_lxmf::direct::DirectDeliveryReceipt { rtt_millis: 23 })
        })
    }

    fn destination_public_key(
        &self,
        _destination: [u8; 16],
    ) -> prns_lxmf::direct::DirectNetworkFuture<'_, Option<[u8; 64]>> {
        Box::pin(async { None })
    }

    fn announce(
        &self,
        _destination: [u8; 16],
    ) -> prns_lxmf::direct::DirectNetworkFuture<
        '_,
        Result<(), prns_lxmf::direct::DirectAnnounceFailure>,
    > {
        Box::pin(async { Ok(()) })
    }
}

fn running_host_state() -> LocalHostState {
    LocalHostState::Running {
        host: Box::new(prns_host::HostSnapshot {
            revision: 1,
            backend: BackendInfo::new(
                BackendKind::Native,
                [Capability::Bluetooth],
                [InterfaceKind::AutomaticBluetoothLe],
            ),
            interfaces: Vec::new(),
            routes: Vec::new(),
            active_link_count: 0,
            destination_identities: Vec::new(),
            runtime: prns_host::RuntimeHealthSnapshot {
                running: true,
                uptime_millis: 1,
                interface_count: 0,
                online_interface_count: 0,
                route_count: 0,
                link_count: 0,
                transported_link_count: 0,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_bps: 0,
                tx_bps: 0,
            },
            persistence: PersistenceSnapshot::persistent(),
        }),
    }
}

fn seed_active_pairing(snapshots: &SnapshotStore, pairing: RemoteControlPairingState) {
    snapshots.update(|snapshot| {
        snapshot.runtime = DevelopmentNodeRuntime::Running;
        snapshot.pairing = pairing;
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Pairing,
            started_at_millis: 7,
        });
    });
}

fn host_revision(snapshot: &DevelopmentNodeSnapshot) -> u64 {
    match &snapshot.local_host {
        LocalHostState::Running { host } => host.revision,
        LocalHostState::Stopped { .. }
        | LocalHostState::Unavailable { .. }
        | LocalHostState::DevelopmentResetRequired { .. } => 0,
    }
}

fn seed_failed_outbound_message(
    supervisor: &Supervisor,
    paths: &NodeStoragePaths,
    destination: [u8; 16],
) {
    let identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
        &[0x71; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    )
    .expect("the fixed LXMF destination name is valid");
    let source = identity.destination();
    let mut encoded = [0_u8; prns_lxmf::wire::MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = prns_lxmf::wire::compose_basic_direct_lxmf(
        destination,
        source,
        1_700_000_009_000,
        b"Offline",
        b"Exact wire",
        None,
        &identity,
        &mut encoded,
    )
    .expect("the small direct message fits");
    let inserted = {
        let mut state = supervisor.lock_state();
        ensure_application_owner_locked(&mut state, paths)
            .expect("application owner")
            .admit_mailbox_async(prns_lxmf::mailbox::MailboxRequest::InsertOutbound(
                prns_lxmf::mailbox::NewOutboundMessage {
                    message_id: prepared.message_id(),
                    source,
                    destination,
                    timestamp_unix_ms: 1_700_000_009_000,
                    title: b"Offline".to_vec(),
                    content: b"Exact wire".to_vec(),
                    exact_wire: encoded[..usize::from(prepared.wire_len())].to_vec(),
                },
            ))
            .expect("insert admission")
    }
    .blocking_recv()
    .expect("insert reply")
    .expect("insert succeeds");
    assert!(matches!(
        inserted,
        prns_lxmf::mailbox::MailboxReply::OutboundInserted { .. }
    ));
    {
        let mut state = supervisor.lock_state();
        ensure_application_owner_locked(&mut state, paths)
            .expect("application owner")
            .admit_mailbox_async(prns_lxmf::mailbox::MailboxRequest::CompleteAttempt {
                key: prns_lxmf::mailbox::AttemptKey {
                    local_record_id: 1,
                    generation: 1,
                },
                completion: prns_lxmf::mailbox::AttemptCompletion::Failed {
                    failure: prns_lxmf::direct::DirectSendFailure::DeliveryTimedOut,
                },
            })
            .expect("failure admission")
    }
    .blocking_recv()
    .expect("failure reply")
    .expect("failure succeeds");
}

fn install_finished_failed_worker(supervisor: &Supervisor, paths: &NodeStoragePaths) {
    let (done_tx, done) = std_mpsc::sync_channel(1);
    let join = std::thread::spawn(move || {
        done_tx
            .send(Err((
                DevelopmentNodeStopStage::Node,
                "node stopped unexpectedly".to_owned(),
            )))
            .expect("terminal worker result");
    });
    while !join.is_finished() {
        std::thread::yield_now();
    }
    let (commands, _commands_rx) = mpsc::channel(1);
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    supervisor.lock_state().worker = Some(test_worker(
        commands,
        ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done,
        Some(join),
        paths.root.clone(),
    ));
}

fn assert_offline_list_retry_cancel(supervisor: &Supervisor, destination: [u8; 16]) {
    let list_input = ListLxmfMessagesInput {
        peer: Some(destination),
        before: None,
        limit: 25,
    };
    let LxmfMessageListOutcome::Listed { messages } = foreign_block_on(
        admission::list_lxmf_messages_with_supervisor(supervisor, list_input.clone()),
    ) else {
        panic!("the offline mailbox did not return its durable row");
    };
    assert!(matches!(
        messages[0].delivery_state,
        crate::contract::LxmfDeliveryState::Failed {
            failed_attempts: ref value,
            last_failure: crate::contract::LxmfDeliveryFailure::DeliveryTimedOut,
        } if *value == 1
    ));

    assert_eq!(
        foreign_block_on(admission::retry_lxmf_message_with_supervisor(
            supervisor,
            RetryLxmfMessageInput { local_record_id: 1 }
        )),
        RetryLxmfMessageOutcome::Accepted { local_record_id: 1 }
    );
    let LxmfMessageListOutcome::Listed { messages } = foreign_block_on(
        admission::list_lxmf_messages_with_supervisor(supervisor, list_input.clone()),
    ) else {
        panic!("the retried mailbox row was not listed");
    };
    assert_eq!(
        messages[0].delivery_state,
        crate::contract::LxmfDeliveryState::Queued { failed_attempts: 1 }
    );

    assert_eq!(
        foreign_block_on(admission::cancel_lxmf_message_with_supervisor(
            supervisor,
            CancelLxmfMessageInput { local_record_id: 1 }
        )),
        CancelLxmfMessageOutcome::Cancelled { local_record_id: 1 }
    );
    let LxmfMessageListOutcome::Listed { messages } = foreign_block_on(
        admission::list_lxmf_messages_with_supervisor(supervisor, list_input),
    ) else {
        panic!("the cancelled mailbox row was not listed");
    };
    assert!(matches!(
        messages[0].delivery_state,
        crate::contract::LxmfDeliveryState::Cancelled { .. }
    ));
}

async fn learn_pending_peer(service: &prns_lxmf::mailbox::DurableDirectLxmfService) -> [u8; 16] {
    use personal_rns::interfaces::InterfaceId;
    use personal_rns::routing::announce::{derive_single_destination_hash, AnnounceObservation};
    use personal_rns::units::{HopCount, InstantMillis};

    let peer_material = PrivateIdentityMaterial::from_bytes(
        [0x52; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    );
    let peer_destination = derive_single_destination_hash(
        &peer_material.identity_hash(),
        prns_lxmf::wire::LXMF_APP_NAME,
        prns_lxmf::wire::LXMF_DELIVERY_ASPECTS,
    )
    .expect("the fixed LXMF destination name is valid");
    let mut announce = [0_u8; 64];
    let announce_len =
        prns_lxmf::wire::encode_current_lxmf_announce(b"Pending peer", &mut announce)
            .expect("the small announce fits");
    let mut refresh = service.subscribe();
    assert_eq!(
        service
            .callbacks()
            .on_accepted_announce(AnnounceObservation {
                destination: peer_destination,
                announced_identity: peer_material.identity_hash(),
                hops: HopCount(1),
                source_interface: InterfaceId::new([4, 1, 2, 3, 4, 5, 6, 7]),
                arrived_at: InstantMillis(4_200),
                app_data: &announce[..announce_len],
                is_path_response: false,
            }),
        prns_lxmf::direct::CallbackOutcome::Enqueued
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if service
                .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                    peer: None,
                    direction: None,
                    before: None,
                    limit: 25,
                })
                .await
                .expect("the mailbox query succeeds")
                .peers
                .is_empty()
            {
                refresh
                    .changed()
                    .await
                    .expect("the service remains running");
            } else {
                break;
            }
        }
    })
    .await
    .expect("the peer observation is consumed");
    *peer_destination.as_bytes()
}

#[test]
fn invitation_input_is_exact_uppercase_hex() {
    assert!(parse_invitation_code("00000000").is_some());
    assert!(parse_invitation_code("89ABCDEF").is_some());
    assert!(parse_invitation_code("89abcdef").is_none());
    assert!(parse_invitation_code("123456").is_none());
    assert!(parse_invitation_code("123456789").is_none());
}

#[test]
fn pairing_request_timeout_has_a_distinct_failure_stage() {
    use personal_rns::engine::{
        RemoteControlControllerPairingRequestFailureCause, SendRequestFailure,
    };

    assert_eq!(
        classify_pairing_request_failure(
            RemoteControlControllerPairingRequestFailureCause::Request(SendRequestFailure::Timeout,),
        ),
        RemoteControlPairingFailureStage::Timeout
    );
    assert_eq!(
        classify_pairing_request_failure(
            RemoteControlControllerPairingRequestFailureCause::Request(
                SendRequestFailure::WriteFailed,
            ),
        ),
        RemoteControlPairingFailureStage::Request
    );
    assert_eq!(
        classify_pairing_request_failure(
            RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
        ),
        RemoteControlPairingFailureStage::Request
    );
}

#[test]
fn approval_failures_preserve_actionable_failure_stages() {
    use personal_rns::engine::{
        ApproveRemoteControlControllerPairingFailure,
        RemoteControlControllerPairingRequestBuildError,
        RemoteControlControllerPairingRequestFailure,
        RemoteControlControllerPairingRequestFailureCause, SendRequestFailure,
    };
    use personal_rns::remote_control::{
        FailRemoteControlControllerPairingRequestOutcome, RemoteControlControllerPairingAborted,
    };
    use personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure;

    let (attempt_id, context) = pairing_attempt_fixture(0xA4);
    let aborted = RemoteControlControllerPairingAborted::AwaitingCompletion {
        attempt_id,
        context,
    };
    let cases = [
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::Request(
                            SendRequestFailure::Timeout,
                        ),
                        exchange: FailRemoteControlControllerPairingRequestOutcome::Aborted {
                            aborted,
                        },
                    },
                ),
                RemoteControlPairingFailureStage::Timeout,
                "The node did not finish pairing before the approval timed out.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::Request(
                            SendRequestFailure::LinkClosed,
                        ),
                        exchange: FailRemoteControlControllerPairingRequestOutcome::Aborted {
                            aborted,
                        },
                    },
                ),
                RemoteControlPairingFailureStage::Link,
                "The connection to the node closed before pairing finished.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
                        exchange: FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt,
                    },
                ),
                RemoteControlPairingFailureStage::Request,
                "The node could not complete the pairing approval request.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::Expired {
                        expired: aborted,
                        retired_link: context.link_id(),
                    },
                ),
                RemoteControlPairingFailureStage::Expired,
                "The pairing attempt expired before approval completed.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::PersistenceInProgress {
                        attempt_id,
                    },
                ),
                RemoteControlPairingFailureStage::Persistence,
                "Pairing could not advance while authorization was being saved.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::Request(
                            SendRequestFailure::WriteFailed,
                        ),
                        exchange:
                            FailRemoteControlControllerPairingRequestOutcome::PersistenceInProgress {
                                attempt_id,
                            },
                    },
                ),
                RemoteControlPairingFailureStage::Persistence,
                "Pairing could not advance while authorization was being saved.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::NoActiveAttempt,
                ),
                RemoteControlPairingFailureStage::Confirmation,
                "The active pairing confirmation is no longer available.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::RequestBuild {
                        failure: RemoteControlControllerPairingRequestBuildError::Capacity {
                            required: 2,
                            maximum: 1,
                        },
                        rollback: FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt,
                    },
                ),
                RemoteControlPairingFailureStage::Request,
                "The node could not complete the pairing approval request.",
            ),
        ];

    for (failure, expected_stage, expected_detail) in cases {
        let stage = classify_pairing_approval_failure(&failure);
        assert_eq!(stage, expected_stage);
        assert_eq!(pairing_approval_failure_detail(stage), expected_detail);
        assert!(!expected_detail.contains("RemoteControl"));
        assert!(!expected_detail.contains("upstream"));
    }
}

fn pairing_attempt_fixture(
    fill: u8,
) -> (
    personal_rns::remote_control::RemoteControlPairingAttemptId,
    personal_rns::remote_control::RemoteControlPairingContext,
) {
    use personal_rns::identity::in_memory::InMemoryNodeIdentity;
    use personal_rns::identity::vault::IdentitySecretKey;
    use personal_rns::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
    use personal_rns::remote_control::{
        RemoteControlControllerIdentity, RemoteControlPairingAttemptId,
        RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin, RemoteControlPairingContext,
        RemoteControlPairingIdentity, RemoteControlPairingInvitationCode,
        RemoteControlPairingPermissions, RemoteControlPairingPreparedOffer,
        RemoteControlRequestSet,
    };
    use personal_rns::routing::links::LinkId;
    use personal_rns::units::DurationMillis;

    let controller_signer = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
        [fill; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    ));
    let controller = RemoteControlControllerIdentity::new(IdentityPublicKeys {
        encryption: controller_signer.encryption_public_key(),
        signing: controller_signer.signing_public_key(),
    });
    let target_signer = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
        [fill.wrapping_add(1); personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    ));
    let endpoint = RemoteControlPairingIdentity::new(IdentityHash::new([fill; 16])).endpoint();
    let context =
        RemoteControlPairingContext::new(endpoint, LinkId::new([fill.wrapping_add(2); 16]));
    let begin = RemoteControlPairingBegin::new(
        controller,
        endpoint,
        RemoteControlPairingInvitationCode::from_value(u32::from(fill)),
    );
    let prepared = RemoteControlPairingPreparedOffer::new(
        &target_signer,
        context,
        &begin,
        RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::all())
            .expect("nonempty permissions"),
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(5_000))
            .expect("valid attempt timeout"),
    );
    (
        RemoteControlPairingAttemptId::from(prepared.transcript()),
        context,
    )
}

#[test]
fn approval_wait_covers_the_longest_protocol_attempt() {
    let longest_attempt = Duration::from_millis(
        personal_rns::remote_control::MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT.0,
    );

    assert!(PAIRING_APPROVAL_TIMEOUT > longest_attempt);
    assert!(PAIRING_APPROVAL_TIMEOUT > COMMAND_TIMEOUT);
}

#[test]
fn development_tcp_target_requires_an_explicit_nonzero_ip_port() {
    assert_eq!(validate_development_tcp_target(None), Ok(None));
    assert_eq!(
        validate_development_tcp_target(Some("192.0.2.1:4242")),
        Ok(Some("192.0.2.1:4242".to_owned()))
    );
    assert_eq!(
        validate_development_tcp_target(Some("[2001:db8::1]:4242")),
        Ok(Some("[2001:db8::1]:4242".to_owned()))
    );
    for invalid in ["example.com:4242", "192.0.2.1", "192.0.2.1:0", ""] {
        assert!(validate_development_tcp_target(Some(invalid)).is_err());
    }
}

#[test]
fn apple_central_restoration_identifier_is_nonempty() {
    assert!(AppleBluetoothPreparation::CentralOnlyRestoration {
        central: "rs.reticulum.prns.dev.bluetooth-auto.central.v1".to_owned(),
    }
    .validate()
    .is_ok());
    assert!(AppleBluetoothPreparation::CentralOnlyRestoration {
        central: String::new(),
    }
    .validate()
    .is_err());
}

fn apple_owner_key(root: &str, central: &str, identity: u8) -> AppleBluetoothOwnerKey {
    AppleBluetoothOwnerKey {
        storage_root: PathBuf::from(root),
        preparation: AppleBluetoothPreparation::CentralOnlyRestoration {
            central: central.to_owned(),
        },
        identity: BleIdentity::new([identity; 16]),
    }
}

#[test]
fn prepared_apple_bluetooth_handoff_is_exact_and_single_use() {
    let expected = apple_owner_key("/tmp/prns/development", "central", 0x41);
    let mut pending = Some(PreparedAppleBluetoothOwner {
        key: expected.clone(),
        prepared: "single owner",
    });

    for mismatch in [
        apple_owner_key("/different/prns/development", "central", 0x41),
        apple_owner_key("/tmp/prns/development", "different-central", 0x41),
        apple_owner_key("/tmp/prns/development", "central", 0x42),
    ] {
        assert!(take_matching_prepared_owner(&mut pending, &mismatch).is_err());
        assert!(
            pending.is_some(),
            "a mismatch must preserve the existing owner"
        );
    }

    assert_eq!(
        take_matching_prepared_owner(&mut pending, &expected),
        Ok(Some("single owner"))
    );
    assert!(pending.is_none());
    assert_eq!(
        take_matching_prepared_owner(&mut pending, &expected),
        Ok(None)
    );
}

#[test]
fn stop_discard_helper_drops_a_pending_apple_bluetooth_owner() {
    let mut pending = Some(PreparedAppleBluetoothOwner {
        key: apple_owner_key("/tmp/prns/development", "central", 0x41),
        prepared: "pending owner",
    });

    discard_prepared_owner(&mut pending);

    assert!(pending.is_none());
}

#[test]
fn invalid_early_restoration_configuration_fails_before_platform_work() {
    assert_eq!(
        prepare_apple_bluetooth_central_restoration(
            Path::new("/tmp/prns/development"),
            String::new(),
        ),
        AppleBluetoothRestorationPreparationOutcome::Failed {
            stage: AppleBluetoothRestorationPreparationFailureStage::Contract,
            detail: "central CoreBluetooth restoration identifier must not be empty".to_owned(),
        }
    );
}

#[test]
fn invalid_restoration_configuration_cannot_overwrite_a_running_generation() {
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    };
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    let (commands, _commands_rx) = mpsc::channel(1);
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    let (_done_tx, done) = std_mpsc::channel();
    supervisor.lock_state().worker = Some(test_worker(
        commands,
        ShutdownSignal {
            sender: shutdown_tx,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done,
        None,
        PathBuf::from("/tmp/prns/running"),
    ));

    let outcome = start_configured_with_supervisor(
        &supervisor,
        Path::new("/tmp/prns/running"),
        DevelopmentNodeStartInput {
            development_tcp_target: None,
        },
        AppleBluetoothPreparation::CentralOnlyRestoration {
            central: String::new(),
        },
    );

    assert!(matches!(
        outcome,
        DevelopmentNodeStartOutcome::AlreadyRunning { .. }
    ));
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Running
    );
    supervisor.lock_state().worker = None;
}

#[test]
fn tracked_lxmf_send_tasks_are_bounded() {
    assert!(lxmf_send_has_capacity(0));
    assert!(lxmf_send_has_capacity(LXMF_SEND_TASK_CAPACITY - 1));
    assert!(!lxmf_send_has_capacity(LXMF_SEND_TASK_CAPACITY));
    assert!(!lxmf_send_has_capacity(LXMF_SEND_TASK_CAPACITY + 1));
}

#[test]
fn admitted_send_waiter_requires_a_definitive_commit_outcome() {
    let supervisor = Arc::new(Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    });
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    let (commands, mut receiver) = mpsc::channel(1);
    let (shutdown_sender, _) = watch::channel(false);
    let (_done_tx, done) = std_mpsc::sync_channel(1);
    supervisor.lock_state().worker = Some(test_worker(
        commands,
        ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done,
        None,
        PathBuf::from("/unused"),
    ));
    let owner = Arc::clone(&supervisor);
    let waiter = std::thread::spawn(move || {
        foreign_block_on(admission::send_direct_text_with_supervisor(
            &owner,
            SendDirectTextInput {
                destination: [7; 16],
                title: String::new(),
                content: "Commit".to_owned(),
            },
        ))
    });
    let Some(Command::SendDirectText(_, response)) = receiver.blocking_recv() else {
        panic!("production admission must submit the send");
    };
    std::thread::sleep(Duration::from_millis(25));
    assert!(!waiter.is_finished());
    response
        .send(SendDirectTextOutcome::Accepted { local_record_id: 7 })
        .expect("publish definitive commit outcome");
    assert_eq!(
        waiter.join().expect("waiter joins"),
        SendDirectTextOutcome::Accepted { local_record_id: 7 }
    );
}

#[test]
fn durable_mailbox_bypasses_the_generation_only_when_no_worker_exists() {
    assert_eq!(
        durable_mailbox_access(DevelopmentNodeRuntime::Running, true),
        DurableMailboxAccess::RunningGeneration
    );
    for runtime in [
        DevelopmentNodeRuntime::Stopped,
        DevelopmentNodeRuntime::Failed,
    ] {
        assert_eq!(
            durable_mailbox_access(runtime, false),
            DurableMailboxAccess::Offline
        );
    }
    for runtime in [
        DevelopmentNodeRuntime::Stopped,
        DevelopmentNodeRuntime::Starting,
        DevelopmentNodeRuntime::Running,
        DevelopmentNodeRuntime::Stopping,
        DevelopmentNodeRuntime::Failed,
    ] {
        assert_eq!(
            durable_mailbox_access(runtime, runtime != DevelopmentNodeRuntime::Running),
            DurableMailboxAccess::GenerationTransition
        );
    }
}

#[test]
fn stopped_generation_lists_retries_and_cancels_through_the_store_owner() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    };
    let destination = [0x42; 16];
    seed_failed_outbound_message(&supervisor, &paths, destination);
    assert_offline_list_retry_cancel(&supervisor, destination);

    supervisor
        .lock_state()
        .application_owner
        .take()
        .expect("offline operations opened one application owner")
        .close()
        .expect("application owner closes");
}

#[test]
fn admitted_offline_mutations_wait_for_their_definitive_store_outcomes() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Arc::new(Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    });
    let destination = [0x42; 16];
    seed_failed_outbound_message(&supervisor, &paths, destination);

    let (retry_entered_tx, retry_entered_rx) = std_mpsc::sync_channel(1);
    let (retry_release_tx, retry_release_rx) = std_mpsc::sync_channel(1);
    supervisor
        .lock_state()
        .application_owner
        .as_ref()
        .expect("application owner")
        .admit_test_barrier(retry_entered_tx, retry_release_rx)
        .expect("retry barrier admission");
    retry_entered_rx
        .recv()
        .expect("owner entered retry barrier");
    let retry_supervisor = Arc::clone(&supervisor);

    let retry = std::thread::spawn(move || {
        foreign_block_on(admission::retry_lxmf_message_with_supervisor(
            &retry_supervisor,
            RetryLxmfMessageInput { local_record_id: 1 },
        ))
    });
    std::thread::sleep(LXMF_QUERY_TIMEOUT + Duration::from_millis(25));
    assert!(!retry.is_finished());
    retry_release_tx.send(()).expect("release retry");
    assert_eq!(
        retry.join().expect("retry waiter joins"),
        RetryLxmfMessageOutcome::Accepted { local_record_id: 1 }
    );

    let (cancel_entered_tx, cancel_entered_rx) = std_mpsc::sync_channel(1);
    let (cancel_release_tx, cancel_release_rx) = std_mpsc::sync_channel(1);
    supervisor
        .lock_state()
        .application_owner
        .as_ref()
        .expect("application owner")
        .admit_test_barrier(cancel_entered_tx, cancel_release_rx)
        .expect("cancel barrier admission");
    cancel_entered_rx
        .recv()
        .expect("owner entered cancel barrier");
    let cancel_supervisor = Arc::clone(&supervisor);

    let cancel = std::thread::spawn(move || {
        foreign_block_on(admission::cancel_lxmf_message_with_supervisor(
            &cancel_supervisor,
            CancelLxmfMessageInput { local_record_id: 1 },
        ))
    });
    std::thread::sleep(LXMF_QUERY_TIMEOUT + Duration::from_millis(25));
    assert!(!cancel.is_finished());
    cancel_release_tx.send(()).expect("release cancel");
    assert_eq!(
        cancel.join().expect("cancel waiter joins"),
        CancelLxmfMessageOutcome::Cancelled { local_record_id: 1 }
    );

    let LxmfMessageListOutcome::Listed { messages } =
        foreign_block_on(admission::list_lxmf_messages_with_supervisor(
            &supervisor,
            ListLxmfMessagesInput {
                peer: Some(destination),
                before: None,
                limit: 25,
            },
        ))
    else {
        panic!("the cancelled mailbox row was not listed");
    };
    assert!(matches!(
        messages[0].delivery_state,
        crate::contract::LxmfDeliveryState::Cancelled { .. }
    ));
    supervisor
        .lock_state()
        .application_owner
        .take()
        .expect("application owner")
        .close()
        .expect("application owner closes");
}

#[test]
fn terminal_failed_worker_keeps_native_reap_authority_during_offline_mailbox_access() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(true)),
        state: Mutex::new(SupervisorState::default()),
    };
    let destination = [0x42; 16];
    seed_failed_outbound_message(&supervisor, &paths, destination);

    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    supervisor.snapshots.set_local_host(running_host_state());
    settle_worker_result(
        &supervisor.operation_admitted,
        &supervisor.snapshots,
        &Err((
            DevelopmentNodeStopStage::Node,
            "node stopped unexpectedly".to_owned(),
        )),
    );
    let failure_snapshot = supervisor.snapshots.read();
    let list_input = ListLxmfMessagesInput {
        peer: Some(destination),
        before: None,
        limit: 25,
    };

    install_finished_failed_worker(&supervisor, &paths);
    let LxmfMessageListOutcome::Listed { messages } = foreign_block_on(
        admission::list_lxmf_messages_with_supervisor(&supervisor, list_input.clone()),
    ) else {
        panic!("the failed generation stranded its durable row");
    };
    assert!(matches!(
        messages[0].delivery_state,
        crate::contract::LxmfDeliveryState::Failed { .. }
    ));
    assert!(supervisor.lock_state().worker.is_some());
    assert_eq!(supervisor.snapshots.read(), failure_snapshot);
    assert_eq!(
        admission::prepare_native_storage_with_supervisor(&supervisor, &storage),
        NativeStoragePreparationOutcome::Prepared
    );
    assert!(supervisor.lock_state().worker.is_none());
    assert_eq!(supervisor.snapshots.read(), failure_snapshot);

    install_finished_failed_worker(&supervisor, &paths);
    assert_eq!(
        foreign_block_on(admission::retry_lxmf_message_with_supervisor(
            &supervisor,
            RetryLxmfMessageInput { local_record_id: 1 }
        )),
        RetryLxmfMessageOutcome::Accepted { local_record_id: 1 }
    );
    assert!(supervisor.lock_state().worker.is_some());
    assert_eq!(supervisor.snapshots.read(), failure_snapshot);
    assert_eq!(
        admission::prepare_native_storage_with_supervisor(&supervisor, &storage),
        NativeStoragePreparationOutcome::Prepared
    );
    assert!(supervisor.lock_state().worker.is_none());
    assert_eq!(supervisor.snapshots.read(), failure_snapshot);

    install_finished_failed_worker(&supervisor, &paths);
    assert_eq!(
        foreign_block_on(admission::cancel_lxmf_message_with_supervisor(
            &supervisor,
            CancelLxmfMessageInput { local_record_id: 1 }
        )),
        CancelLxmfMessageOutcome::Cancelled { local_record_id: 1 }
    );
    assert!(supervisor.lock_state().worker.is_some());
    assert_eq!(supervisor.snapshots.read(), failure_snapshot);
    assert_eq!(
        admission::prepare_native_storage_with_supervisor(&supervisor, &storage),
        NativeStoragePreparationOutcome::Prepared
    );
    assert!(supervisor.lock_state().worker.is_none());
    assert_eq!(supervisor.snapshots.read(), failure_snapshot);

    let LxmfMessageListOutcome::Listed { messages } = foreign_block_on(
        admission::list_lxmf_messages_with_supervisor(&supervisor, list_input),
    ) else {
        panic!("the cancelled mailbox row was not listed");
    };
    assert!(matches!(
        messages[0].delivery_state,
        crate::contract::LxmfDeliveryState::Cancelled { .. }
    ));
    supervisor
        .lock_state()
        .application_owner
        .take()
        .expect("offline operations retained one application owner")
        .close()
        .expect("application owner closes");
}

#[test]
fn terminal_failed_worker_does_not_block_a_direct_restart() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(true)),
        state: Mutex::new(SupervisorState::default()),
    };
    {
        let mut state = supervisor.lock_state();
        identity_vault(&mut state, &paths)
            .expect("identity vault")
            .store(
                &primary_label().expect("primary label"),
                &[0x42; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
            )
            .expect("primary identity");
    }
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    supervisor.snapshots.set_local_host(running_host_state());
    settle_worker_result(
        &supervisor.operation_admitted,
        &supervisor.snapshots,
        &Err((
            DevelopmentNodeStopStage::Node,
            "node stopped unexpectedly".to_owned(),
        )),
    );
    install_finished_failed_worker(&supervisor, &paths);

    assert!(matches!(
        start_configured_with_supervisor(
            &supervisor,
            &storage,
            DevelopmentNodeStartInput {
                development_tcp_target: None,
            },
            AppleBluetoothPreparation::WithoutRestoration,
        ),
        DevelopmentNodeStartOutcome::Started { .. }
    ));
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Running
    );
    let mut state = supervisor.lock_state();
    assert_eq!(
        stop_locked(&supervisor, &mut state),
        DevelopmentNodeStopOutcome::Stopped
    );
    state
        .application_owner
        .take()
        .expect("restart retained the application owner")
        .close()
        .expect("application owner closes");
}

#[test]
fn reset_preserves_the_owner_and_files_after_a_reported_stop_failure() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let owner =
        DevelopmentStoreOwner::open(&paths.root, &paths.application).expect("application owner");
    let (commands, _commands_rx) = mpsc::channel(1);
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let (done_tx, done) = std_mpsc::sync_channel(1);
    done_tx
        .send(Err((
            DevelopmentNodeStopStage::Worker,
            "reported worker failure".to_owned(),
        )))
        .expect("seed worker result");
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(test_supervisor_state(
            Some(test_worker(
                commands,
                ShutdownSignal {
                    sender: shutdown_sender,
                    requested: Arc::new(AtomicBool::new(false)),
                },
                done,
                Some(std::thread::spawn(|| {})),
                paths.root.clone(),
            )),
            Some(owner),
            None,
        )),
    };
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);

    assert!(matches!(
        reset_with_supervisor(&supervisor, &storage),
        DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Worker,
            ..
        }
    ));
    assert!(paths.application.exists());
    assert!(supervisor.lock_state().application_owner.is_some());
    assert!(supervisor
        .lock_state()
        .worker
        .as_ref()
        .is_some_and(|worker| worker.incomplete_stop.is_some() && worker.join.is_none()));
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Stopping
    );

    assert_eq!(
        reset_with_supervisor(&supervisor, &storage),
        DevelopmentNodeStopOutcome::Stopped
    );
    assert!(!storage.exists());
    assert!(supervisor.lock_state().application_owner.is_none());
}

#[tokio::test]
async fn durable_lxmf_send_accepts_and_queries_before_pending_proof() {
    let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
        &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    )
    .expect("the fixed LXMF destination name is valid");
    let root = tempfile::tempdir().expect("temporary application root");
    let database_path = root.path().join("application.redb");
    let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
        .expect("the application owner opens");
    let network = Arc::new(PendingProofNetwork::default());
    let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
    let service = pending
        .start(
            Arc::clone(&network) as Arc<dyn prns_lxmf::direct::DirectNetwork>,
            owner.mailbox_submitter(),
            Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
        )
        .await
        .expect("the test owns a Tokio runtime");
    let peer_destination = learn_pending_peer(&service).await;

    let (send_response, send_result) = oneshot::channel();
    let mut send_tasks = JoinSet::new();
    dispatch_lxmf_send(
        &service,
        &mut send_tasks,
        SendDirectTextInput {
            destination: peer_destination,
            title: "Proof gate".to_owned(),
            content: "Still sending".to_owned(),
        },
        send_response,
        1_700_000_000_000,
    );
    tokio::time::timeout(Duration::from_secs(1), network.send_entered.notified())
        .await
        .expect("the direct attempt reaches its pending proof gate");
    let accepted = tokio::time::timeout(Duration::from_secs(1), send_result)
        .await
        .expect("send response deadline")
        .expect("queue acceptance does not await proof");
    let SendDirectTextOutcome::Accepted { local_record_id } = accepted else {
        panic!("the durable send was not accepted: {accepted:?}");
    };
    assert!(send_tasks
        .join_next()
        .await
        .expect("the tracked response wrapper exists")
        .is_ok());

    let (list_response, mut list_result) = oneshot::channel();
    dispatch_lxmf_message_list(
        &service,
        prns_lxmf::mailbox::MailboxListRequest {
            peer: Some(peer_destination),
            direction: None,
            before: None,
            limit: 25,
        },
        list_response,
    )
    .await;
    let LxmfMessageListOutcome::Listed { messages } = list_result
        .try_recv()
        .expect("the query completes while proof remains pending")
    else {
        panic!("the pending message query did not return a list");
    };
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].delivery_state,
        crate::contract::LxmfDeliveryState::Sending { failed_attempts: 0 }
    );
    assert_eq!(messages[0].local_record_id, local_record_id);

    network.release_send.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = service
                .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                    peer: None,
                    direction: None,
                    before: None,
                    limit: 25,
                })
                .await
                .expect("the mailbox query succeeds");
            if matches!(
                snapshot.messages[0].delivery_state,
                prns_lxmf::mailbox::DurableLxmfDeliveryState::Delivered { .. }
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("proof settlement is persisted");
    service.stop().await.expect("the service stops promptly");
    owner.close().expect("the owner closes after the service");
}

#[tokio::test]
async fn admitted_insert_survives_stop_and_response_drain_boundaries() {
    let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
        &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    )
    .expect("the fixed LXMF destination name is valid");
    let root = tempfile::tempdir().expect("temporary application root");
    let database_path = root.path().join("application.redb");
    let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
        .expect("the application owner opens");
    let blocking_submitter = Arc::new(BlockingInsertSubmitter::new(owner.mailbox_submitter()));
    let network = Arc::new(PendingProofNetwork::default());
    let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
    let service = pending
        .start(
            Arc::clone(&network) as Arc<dyn prns_lxmf::direct::DirectNetwork>,
            Arc::clone(&blocking_submitter) as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
            Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
        )
        .await
        .expect("the test owns a Tokio runtime");
    let peer_destination = learn_pending_peer(&service).await;
    let (send_response, mut send_result) = oneshot::channel();
    let mut send_tasks = JoinSet::new();
    dispatch_lxmf_send(
        &service,
        &mut send_tasks,
        SendDirectTextInput {
            destination: peer_destination,
            title: "Retain response".to_owned(),
            content: "Commit exactly once".to_owned(),
        },
        send_response,
        1_700_000_000_500,
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        blocking_submitter.insert_entered.notified(),
    )
    .await
    .expect("the insert reaches its durable owner boundary");

    let mut shutdown = Box::pin(stop_lxmf_service_and_drain(&service, &mut send_tasks));
    let old_stop_and_drain_boundaries =
        prns_lxmf::direct::STOP_JOIN_TIMEOUT + LXMF_QUERY_TIMEOUT + Duration::from_millis(25);
    assert!(
        tokio::time::timeout(old_stop_and_drain_boundaries, &mut shutdown)
            .await
            .is_err()
    );
    assert!(matches!(
        send_result.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    blocking_submitter.release_insert.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), &mut shutdown)
            .await
            .expect("retained response wrapper and service complete"),
        Ok(())
    );
    drop(shutdown);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), send_result)
            .await
            .expect("send response deadline")
            .expect("the committed insert reports its definitive outcome"),
        SendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    assert_eq!(network.send_count.load(Ordering::Acquire), 0);

    let listed = owner
        .admit_mailbox_async(prns_lxmf::mailbox::MailboxRequest::List(
            prns_lxmf::mailbox::MailboxListRequest {
                peer: None,
                direction: None,
                before: None,
                limit: 25,
            },
        ))
        .expect("list admission")
        .await
        .expect("list response")
        .expect("list succeeds");
    let prns_lxmf::mailbox::MailboxReply::Listed { messages, .. } = listed else {
        panic!("unexpected mailbox reply");
    };
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].delivery_state,
        prns_lxmf::mailbox::DurableLxmfDeliveryState::Queued { .. }
    ));
    owner.close().expect("the owner closes after the service");
}

#[tokio::test]
async fn generated_send_caller_drop_preserves_the_owned_durable_insert() {
    let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
        &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    )
    .expect("local identity");
    let root = tempfile::tempdir().expect("isolated application storage");
    let owner = DevelopmentStoreOwner::open(root.path(), &root.path().join("application.redb"))
        .expect("database owner");
    let blocked = Arc::new(BlockingInsertSubmitter::new(owner.mailbox_submitter()));
    let network = Arc::new(PendingProofNetwork::default());
    let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
    let service = pending
        .start(
            Arc::clone(&network) as Arc<dyn prns_lxmf::direct::DirectNetwork>,
            Arc::clone(&blocked) as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
            Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
        )
        .await
        .expect("existing service runtime");
    let destination = learn_pending_peer(&service).await;
    let (response, receiver) = oneshot::channel();
    let mut responses = JoinSet::new();
    dispatch_lxmf_send(
        &service,
        &mut responses,
        SendDirectTextInput {
            destination,
            title: "Caller left".to_owned(),
            content: "Commit once".to_owned(),
        },
        response,
        1_700_000_000_500,
    );
    tokio::time::timeout(Duration::from_secs(1), blocked.insert_entered.notified())
        .await
        .expect("insert admitted");
    drop(receiver);
    let mut stopping = Box::pin(stop_lxmf_service_and_drain(&service, &mut responses));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut stopping)
            .await
            .is_err()
    );
    blocked.release_insert.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), &mut stopping)
            .await
            .expect("durable drain"),
        Ok(())
    );
    drop(stopping);
    assert!(responses.is_empty());
    assert_eq!(network.send_count.load(Ordering::Acquire), 0);
    let prns_lxmf::mailbox::MailboxReply::Listed { messages, .. } = owner
        .admit_mailbox_async(prns_lxmf::mailbox::MailboxRequest::List(
            prns_lxmf::mailbox::MailboxListRequest {
                peer: None,
                direction: None,
                before: None,
                limit: 25,
            },
        ))
        .expect("query admission")
        .await
        .expect("query response")
        .expect("query result")
    else {
        panic!("mailbox query");
    };
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].local_record_id, 1);
    owner.close().expect("stop closes the single owner");
}

#[tokio::test]
async fn async_mailbox_corruption_preserves_the_typed_reset_affordance() {
    let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
        &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    )
    .expect("the fixed LXMF destination name is valid");
    let root = tempfile::tempdir().expect("temporary application root");
    let database_path = root.path().join("application.redb");
    let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
        .expect("the application owner opens");
    let submitter = Arc::new(ResetCompletionSubmitter {
        inner: owner.mailbox_submitter(),
    });
    let network = Arc::new(PendingProofNetwork::default());
    let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
    let service = pending
        .start(
            Arc::clone(&network) as Arc<dyn prns_lxmf::direct::DirectNetwork>,
            submitter as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
            Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
        )
        .await
        .expect("the test owns a Tokio runtime");
    let peer_destination = learn_pending_peer(&service).await;

    assert_eq!(
        service
            .send_direct_text(
                peer_destination,
                1_700_000_000_700,
                b"Corruption",
                b"Preserve reset affordance",
            )
            .await,
        prns_lxmf::mailbox::DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
    );
    tokio::time::timeout(Duration::from_secs(1), network.send_entered.notified())
        .await
        .expect("the attempt reaches the proof boundary");
    network.release_send.notify_one();
    let failure = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = service
                .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                    peer: None,
                    direction: None,
                    before: None,
                    limit: 25,
                })
                .await
                .expect("the read remains available");
            if let prns_lxmf::mailbox::MailboxProjectionHealth::ResetRequired(reason) =
                snapshot.mailbox_health
            {
                break prns_lxmf::mailbox::MailboxFailure::ResetRequired(reason);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the asynchronous corruption becomes visible");
    assert_eq!(
        service
            .send_direct_text(
                peer_destination,
                1_700_000_000_701,
                b"Later write",
                b"Cannot clear corruption",
            )
            .await,
        prns_lxmf::mailbox::DurableSendDirectTextOutcome::Accepted { local_record_id: 2 }
    );
    assert!(matches!(
        service
            .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                peer: None,
                direction: None,
                before: None,
                limit: 25,
            })
            .await
            .expect("the read remains available")
            .mailbox_health,
        prns_lxmf::mailbox::MailboxProjectionHealth::ResetRequired(ref reason)
            if reason == "mailbox record became unreadable"
    ));

    let snapshots = SnapshotStore::new();
    snapshots.set_runtime(DevelopmentNodeRuntime::Running);
    snapshots.set_local_host(running_host_state());
    assert_eq!(
        refresh_lxmf_health(&service, &snapshots).await,
        Err(failure.clone())
    );
    assert_eq!(
        publish_lxmf_storage_failure(&snapshots, &failure),
        "mailbox record became unreadable"
    );
    let failed = snapshots.read();
    assert_eq!(failed.runtime, DevelopmentNodeRuntime::Failed);
    assert_eq!(
        failed.local_host,
        LocalHostState::DevelopmentResetRequired {
            reason: "mailbox record became unreadable".to_owned(),
        }
    );
    assert_eq!(
        failed.failure,
        Some(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Storage,
            detail: "mailbox record became unreadable".to_owned(),
        })
    );
    service.stop().await.expect("the service stops promptly");
    owner.close().expect("the owner closes after the service");
}

#[tokio::test]
async fn transient_mailbox_refresh_failures_degrade_and_retry_without_stopping() {
    let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
        &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    )
    .expect("the fixed LXMF destination name is valid");
    let root = tempfile::tempdir().expect("temporary application root");
    let database_path = root.path().join("application.redb");
    let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
        .expect("the application owner opens");
    let submitter = Arc::new(TransientListFailureSubmitter {
        inner: owner.mailbox_submitter(),
        remaining_failures: std::sync::atomic::AtomicUsize::new(2),
        list_submissions: std::sync::atomic::AtomicUsize::new(0),
    });
    let network = Arc::new(PendingProofNetwork::default());
    let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
    let service = pending
        .start(
            network,
            submitter as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
            Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
        )
        .await
        .expect("the test owns a Tokio runtime");
    let snapshots = SnapshotStore::new();
    snapshots.set_runtime(DevelopmentNodeRuntime::Running);
    snapshots.set_local_host(running_host_state());
    snapshots.refresh_lxmf(crate::contract::LxmfHealth {
        state: crate::contract::LxmfHealthState::Ready,
        inbound_overflow_count: 7,
    });

    assert_eq!(
        refresh_running_lxmf_health(&service, &snapshots).await,
        Ok(true)
    );
    let busy = snapshots.read();
    assert_eq!(busy.runtime, DevelopmentNodeRuntime::Running);
    assert_eq!(busy.lxmf.state, crate::contract::LxmfHealthState::Degraded);
    assert_eq!(busy.lxmf.inbound_overflow_count, 7);
    assert_eq!(busy.failure, None);

    assert_eq!(
        refresh_running_lxmf_health(&service, &snapshots).await,
        Ok(true)
    );
    let unavailable = snapshots.read();
    assert_eq!(unavailable.runtime, DevelopmentNodeRuntime::Running);
    assert_eq!(unavailable.revision, busy.revision);
    assert_eq!(unavailable.failure, None);

    assert_eq!(
        refresh_running_lxmf_health(&service, &snapshots).await,
        Ok(false)
    );
    let recovered = snapshots.read();
    assert_eq!(recovered.runtime, DevelopmentNodeRuntime::Running);
    assert_eq!(
        recovered.lxmf.state,
        crate::contract::LxmfHealthState::Ready
    );
    assert_eq!(recovered.lxmf.inbound_overflow_count, 0);
    assert_eq!(recovered.failure, None);

    service.stop().await.expect("the service stops promptly");
    owner.close().expect("the owner closes after the service");
}

#[tokio::test(start_paused = true)]
async fn delayed_mailbox_health_retry_coalesces_feedback_and_services_actor_input() {
    let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
        &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    )
    .expect("the fixed LXMF destination name is valid");
    let root = tempfile::tempdir().expect("temporary application root");
    let database_path = root.path().join("application.redb");
    let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
        .expect("the application owner opens");
    let submitter = Arc::new(TransientListFailureSubmitter {
        inner: owner.mailbox_submitter(),
        remaining_failures: std::sync::atomic::AtomicUsize::new(2),
        list_submissions: std::sync::atomic::AtomicUsize::new(0),
    });
    let network = Arc::new(PendingProofNetwork::default());
    let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
    let service = pending
        .start(
            network,
            Arc::clone(&submitter) as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
            Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
        )
        .await
        .expect("the test owns a Tokio runtime");
    let snapshots = SnapshotStore::new();
    snapshots.set_runtime(DevelopmentNodeRuntime::Running);
    snapshots.set_local_host(running_host_state());
    snapshots.refresh_lxmf(crate::contract::LxmfHealth {
        state: crate::contract::LxmfHealthState::Ready,
        inbound_overflow_count: 7,
    });
    let mut refresh = service.subscribe();
    let mut retry_timer = tokio::time::interval(LXMF_HEALTH_RETRY_DELAY);
    retry_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut retry_pending = false;

    refresh_running_lxmf_health_after_change(
        &service,
        &snapshots,
        &mut retry_pending,
        &mut retry_timer,
    )
    .await
    .expect("a transient failure does not stop the actor");
    assert!(retry_pending);
    assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 1);

    let (actor_input, mut actor_commands) = tokio::sync::mpsc::channel(1);
    actor_input
        .send(())
        .await
        .expect("the command lane is open");
    let mut command_processed = false;
    for _ in 0..2 {
        tokio::select! {
            biased;
            changed = refresh.changed() => {
                changed.expect("the service remains running");
                refresh_running_lxmf_health_after_change(
                    &service,
                    &snapshots,
                    &mut retry_pending,
                    &mut retry_timer,
                ).await.expect("feedback is coalesced");
            }
            command = actor_commands.recv() => {
                command_processed = command.is_some();
                break;
            }
        }
    }
    assert!(command_processed);
    assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 1);

    tokio::time::advance(LXMF_HEALTH_RETRY_DELAY).await;
    retry_timer.tick().await;
    retry_pending = refresh_running_lxmf_health(&service, &snapshots)
        .await
        .expect("the unavailable retry remains nonfatal");
    assert!(retry_pending);
    assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 2);
    refresh
        .changed()
        .await
        .expect("the service remains running");
    refresh_running_lxmf_health_after_change(
        &service,
        &snapshots,
        &mut retry_pending,
        &mut retry_timer,
    )
    .await
    .expect("retry feedback is coalesced");
    assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 2);

    tokio::time::advance(LXMF_HEALTH_RETRY_DELAY).await;
    retry_timer.tick().await;
    retry_pending = refresh_running_lxmf_health(&service, &snapshots)
        .await
        .expect("the delayed retry recovers");
    assert!(!retry_pending);
    assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 3);
    let recovered = snapshots.read();
    assert_eq!(recovered.runtime, DevelopmentNodeRuntime::Running);
    assert_eq!(
        recovered.lxmf.state,
        crate::contract::LxmfHealthState::Ready
    );
    assert_eq!(recovered.failure, None);

    service.stop().await.expect("the service stops promptly");
    owner.close().expect("the owner closes after the service");
}

#[test]
fn fatal_pairing_commands_remove_only_the_selected_discovery_candidate() {
    let endpoint = personal_rns::remote_control::RemoteControlPairingIdentity::new(
        personal_rns::identity::IdentityHash::new([0x42; 16]),
    )
    .endpoint();
    let other_endpoint = personal_rns::remote_control::RemoteControlPairingIdentity::new(
        personal_rns::identity::IdentityHash::new([0x43; 16]),
    )
    .endpoint();
    let mut controls = PairingControls {
        candidates: vec![
            crate::pairing::PairingCandidateControl {
                candidate_id: "candidate".to_owned(),
                endpoint,
                display_name: Some("Candidate".to_owned()),
                observed_at: personal_rns::units::InstantMillis(1),
                expires_at: personal_rns::units::InstantMillis(10),
            },
            crate::pairing::PairingCandidateControl {
                candidate_id: "other".to_owned(),
                endpoint: other_endpoint,
                display_name: Some("Other".to_owned()),
                observed_at: personal_rns::units::InstantMillis(2),
                expires_at: personal_rns::units::InstantMillis(10),
            },
        ],
        initiated_candidate_id: Some("candidate".to_owned()),
        active_attempt_id: Some("attempt".to_owned()),
        ..PairingControls::default()
    };
    let snapshots = SnapshotStore::new();
    seed_active_pairing(
        &snapshots,
        RemoteControlPairingState::InvitationSubmitted {
            candidate_id: "candidate".to_owned(),
        },
    );

    let outcome = pairing_failed_visible(
        &mut controls,
        &snapshots,
        RemoteControlPairingFailureStage::Link,
        "link failed",
    );

    assert_eq!(
        outcome,
        RemoteControlPairingCommandOutcome::Failed {
            stage: RemoteControlPairingFailureStage::Link,
            detail: "link failed".to_owned(),
        }
    );
    assert_eq!(controls.candidates.len(), 1);
    assert_eq!(controls.candidates[0].candidate_id, "other");
    assert!(controls.confirmation.is_none());
    assert!(controls.initiated_candidate_id.is_none());
    assert!(controls.active_attempt_id.is_none());
    let snapshot = snapshots.read();
    assert_eq!(
        snapshot.pairing,
        RemoteControlPairingState::Failed {
            stage: RemoteControlPairingFailureStage::Link,
            detail: "link failed".to_owned(),
        }
    );
    assert!(snapshot.active_operation.is_none());
}

#[test]
fn persistence_terminal_failure_projects_pairing_and_worker_failure() {
    let snapshots = SnapshotStore::new();
    seed_active_pairing(
        &snapshots,
        RemoteControlPairingState::Persisting {
            attempt_id: "attempt".to_owned(),
        },
    );
    let admitted = AtomicBool::new(true);
    let result = Err((
        DevelopmentNodeStopStage::Persistence,
        "authorization flush failed".to_owned(),
    ));

    settle_worker_result(&admitted, &snapshots, &result);

    assert!(!admitted.load(Ordering::Acquire));
    let snapshot = snapshots.read();
    assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
    assert_eq!(
        snapshot.pairing,
        RemoteControlPairingState::Failed {
            stage: RemoteControlPairingFailureStage::Persistence,
            detail: "The paired target authorization could not be persisted.".to_owned(),
        }
    );
    assert_eq!(
        snapshot.failure,
        Some(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::PersistenceRestore,
            detail: "authorization flush failed".to_owned(),
        })
    );
    assert!(snapshot.active_operation.is_none());
}

#[test]
fn non_persistence_worker_failure_is_terminal_without_rewriting_pairing() {
    let snapshots = SnapshotStore::new();
    let pairing = RemoteControlPairingState::ConfirmationRequired {
        attempt_id: "attempt".to_owned(),
        confirmation_code: "123456".to_owned(),
        target_identity_fingerprint: vec![0x42; 16],
        permissions: vec![crate::contract::RemoteControlRequestKind::Describe],
    };
    seed_active_pairing(&snapshots, pairing.clone());
    snapshots.set_local_host(running_host_state());
    let admitted = AtomicBool::new(true);
    let result = Err((DevelopmentNodeStopStage::Node, "node stopped".to_owned()));

    settle_worker_result(&admitted, &snapshots, &result);

    assert!(!admitted.load(Ordering::Acquire));
    let snapshot = snapshots.read();
    assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
    assert_eq!(
        snapshot.local_host,
        LocalHostState::Stopped {
            last_start_failure: Some("node stopped".to_owned()),
        }
    );
    assert_eq!(snapshot.pairing, pairing);
    assert_eq!(
        snapshot.failure,
        Some(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Node,
            detail: "node stopped".to_owned(),
        })
    );
    assert!(snapshot.active_operation.is_none());
}

#[test]
fn worker_failure_during_explicit_stop_retains_the_stopping_transition() {
    let snapshots = SnapshotStore::new();
    snapshots.set_runtime(DevelopmentNodeRuntime::Running);
    let local_host = running_host_state();
    snapshots.set_local_host(local_host.clone());
    snapshots.begin_stop(42);
    let admitted = AtomicBool::new(true);

    settle_worker_result(
        &admitted,
        &snapshots,
        &Err((
            DevelopmentNodeStopStage::Worker,
            "shutdown did not settle".to_owned(),
        )),
    );

    assert!(!admitted.load(Ordering::Acquire));
    let snapshot = snapshots.read();
    assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Stopping);
    assert_eq!(snapshot.local_host, local_host);
    assert_eq!(
        snapshot.failure,
        Some(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Runtime,
            detail: "shutdown did not settle".to_owned(),
        })
    );
    assert_eq!(
        snapshot.active_operation,
        Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Shutdown,
            started_at_millis: 42,
        })
    );
}

#[test]
fn terminal_failure_preserves_a_reset_required_host() {
    let snapshots = SnapshotStore::new();
    snapshots.set_local_host(LocalHostState::DevelopmentResetRequired {
        reason: "malformed persisted Bluetooth identity".to_owned(),
    });

    settle_worker_result(
        &AtomicBool::new(true),
        &snapshots,
        &Err((DevelopmentNodeStopStage::Node, "node stopped".to_owned())),
    );

    assert!(matches!(
        snapshots.read().local_host,
        LocalHostState::DevelopmentResetRequired { .. }
    ));
}

#[test]
fn failed_start_cleanup_joins_and_clears_a_shutdown_responsive_worker() {
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(true)),
        state: Mutex::new(SupervisorState::default()),
    };
    let (commands, _command_rx) = mpsc::channel(1);
    let (shutdown_sender, mut shutdown_rx) = watch::channel(false);
    let shutdown = ShutdownSignal {
        sender: shutdown_sender,
        requested: Arc::new(AtomicBool::new(false)),
    };
    let (done_tx, done) = std_mpsc::sync_channel(1);
    let join = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            shutdown_rx.changed().await.expect("shutdown signal");
            assert!(*shutdown_rx.borrow());
        });
        let _ = done_tx.send(Ok(()));
    });
    let mut state = test_supervisor_state(
        Some(test_worker(
            commands,
            shutdown,
            done,
            Some(join),
            PathBuf::from("/tmp/prns/development"),
        )),
        None,
        None,
    );

    finish_failed_start(
        &supervisor,
        &mut state,
        DevelopmentNodeFailureStage::PersistenceRestore,
        "readiness stalled".to_owned(),
    );

    assert!(state.worker.is_none());
    assert!(!supervisor.operation_admitted.load(Ordering::Acquire));
    assert_eq!(
        supervisor.snapshots.read().failure,
        Some(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::PersistenceRestore,
            detail: "readiness stalled".to_owned(),
        })
    );
}

fn async_snapshot_test_owner() -> (Arc<Supervisor>, mpsc::Receiver<Command>) {
    let supervisor = Arc::new(Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    });
    supervisor
        .snapshots
        .begin_generation(PrimaryIdentityState::Missing);
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    supervisor.snapshots.set_local_host(running_host_state());
    let (commands, receiver) = mpsc::channel(COMMAND_LANE_CAPACITY);
    let (shutdown, _) = watch::channel(false);
    let (_, done) = std_mpsc::sync_channel(1);
    let worker = test_worker(
        commands,
        ShutdownSignal {
            sender: shutdown,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done,
        None,
        PathBuf::from("/unused"),
    );
    worker
        .runtime
        .set(tokio::runtime::Handle::current())
        .expect("one runtime");
    supervisor.lock_state().worker = Some(worker);
    (supervisor, receiver)
}

#[tokio::test]
async fn generated_snapshot_yields_and_uses_existing_actor() {
    let (supervisor, mut commands) = async_snapshot_test_owner();
    let owner = Arc::clone(&supervisor);
    let query = tokio::spawn(async move { snapshot_async_with_supervisor(&owner).await });
    let Some(Command::Snapshot(response)) = commands.recv().await else {
        panic!("async read")
    };
    assert!(!query.is_finished());
    tokio::task::yield_now().await;
    supervisor
        .snapshots
        .update(|snapshot| snapshot.revision = 42);
    let expected = supervisor.snapshots.read();
    response.send(expected.clone()).expect("live query");
    assert_eq!(query.await.expect("query completion"), expected);
}

#[tokio::test]
async fn generated_snapshot_cancellation_retires_waiter_without_stopping_node() {
    let (supervisor, mut commands) = async_snapshot_test_owner();
    let owner = Arc::clone(&supervisor);
    let query = tokio::spawn(async move { snapshot_async_with_supervisor(&owner).await });
    let Some(Command::Snapshot(mut response)) = commands.recv().await else {
        panic!("async read")
    };
    query.abort();
    assert!(query.await.expect_err("cancelled query").is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), response.closed())
        .await
        .expect("waiter retired");
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Running
    );
    assert!(supervisor.lock_state().worker.is_some());
}

#[tokio::test(start_paused = true)]
async fn generated_snapshot_timeout_does_not_mutate_a_replacement_generation() {
    let (supervisor, mut commands) = async_snapshot_test_owner();
    let owner = Arc::clone(&supervisor);
    let query = tokio::spawn(async move { snapshot_async_with_supervisor(&owner).await });
    let Some(Command::Snapshot(_response)) = commands.recv().await else {
        panic!("async read")
    };
    supervisor
        .snapshots
        .begin_generation(PrimaryIdentityState::Missing);
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    supervisor.snapshots.set_local_host(running_host_state());
    let replacement = supervisor.snapshots.read();
    tokio::time::advance(SNAPSHOT_TIMEOUT + Duration::from_secs(1)).await;
    assert_eq!(query.await.expect("timeout settled"), replacement);
    assert_eq!(supervisor.snapshots.read(), replacement);
}

#[tokio::test]
async fn generated_snapshot_never_waits_for_native_lifecycle_lock() {
    let (supervisor, _commands) = async_snapshot_test_owner();
    let held = supervisor.lock_state();
    let expected = supervisor.snapshots.read();
    let mut query = std::pin::pin!(snapshot_async_with_supervisor(&supervisor));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert_eq!(
        std::future::Future::poll(query.as_mut(), &mut context),
        std::task::Poll::Ready(expected)
    );
    drop(held);
}
#[tokio::test]
async fn snapshot_failure_after_concurrent_reset_keeps_the_reset_snapshot() {
    let (supervisor, mut commands) = async_snapshot_test_owner();
    let owner = Arc::clone(&supervisor);
    let query = tokio::spawn(async move { snapshot_async_with_supervisor(&owner).await });
    let Some(Command::Snapshot(response)) = commands.recv().await else {
        panic!("snapshot inspection was not admitted");
    };
    // An admitted async query never holds native lifecycle admission. Reset
    // can replace its generation while the old actor's reply remains held.
    {
        let mut state = supervisor.lock_state();
        state.worker = None;
        supervisor.snapshots.reset();
    }
    let reset_snapshot = supervisor.snapshots.read();
    drop(response);
    assert_eq!(
        query.await.expect("query settles after its actor closes"),
        reset_snapshot
    );
    assert_eq!(supervisor.snapshots.read(), reset_snapshot);
    assert_eq!(reset_snapshot.runtime, DevelopmentNodeRuntime::Stopped);
    assert!(matches!(
        reset_snapshot.local_host,
        LocalHostState::Stopped { .. }
    ));
}

#[test]
fn observed_save_rechecks_generation_after_waiting_outside_the_supervisor_lock() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Arc::new(Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    });
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);

    assert_eq!(
        admission::prepare_native_storage_with_supervisor(&supervisor, &storage),
        NativeStoragePreparationOutcome::Prepared
    );

    let (old_commands, mut old_command_rx) = mpsc::channel(1);
    let (query_started_tx, query_started_rx) = std_mpsc::sync_channel(1);
    let (release_query_tx, release_query_rx) = std_mpsc::sync_channel(1);
    let actor = std::thread::spawn(move || {
        let Some(Command::ObservedIdentity(destination, response)) = old_command_rx.blocking_recv()
        else {
            panic!("observed identity command was not admitted");
        };
        assert_eq!(destination, [4; 16]);
        query_started_tx.send(()).expect("publish query start");
        release_query_rx.recv().expect("release identity response");
        response
            .send(Ok(Some([5; 16])))
            .expect("publish identity response");
    });
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let (_done_tx, done) = std_mpsc::sync_channel(1);
    supervisor.lock_state().worker = Some(test_worker(
        old_commands,
        ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done,
        None,
        paths.root.clone(),
    ));

    let (result_tx, result_rx) = std_mpsc::sync_channel(1);
    let save_supervisor = Arc::clone(&supervisor);

    let save = std::thread::spawn(move || {
        result_tx
            .send(foreign_block_on(
                admission::save_observed_destination_with_supervisor(
                    &save_supervisor,
                    ContactDestinationInput {
                        destination: [4; 16],
                    },
                ),
            ))
            .expect("publish save result");
    });
    query_started_rx.recv().expect("identity query started");

    let (new_commands, _new_command_rx) = mpsc::channel(1);
    let (new_shutdown_sender, _new_shutdown_rx) = watch::channel(false);
    let (_new_done_tx, new_done) = std_mpsc::sync_channel(1);
    supervisor.lock_state().worker = Some(test_worker(
        new_commands,
        ShutdownSignal {
            sender: new_shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        },
        new_done,
        None,
        paths.root,
    ));
    release_query_tx.send(()).expect("release identity query");

    assert!(matches!(
        result_rx.recv().expect("save result"),
        ContactMutationOutcome::DevelopmentUnavailable { .. }
    ));
    assert!(supervisor.lock_state().application_owner.is_some());
    assert_eq!(
        foreign_block_on(admission::list_contacts_with_supervisor(&supervisor)),
        ContactListOutcome::Listed { contacts: vec![] }
    );
    save.join().expect("save thread");
    actor.join().expect("actor thread");
}

#[test]
fn observed_save_persists_the_generation_identity_and_reopens() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    };
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);

    assert_eq!(
        admission::prepare_native_storage_with_supervisor(&supervisor, &storage),
        NativeStoragePreparationOutcome::Prepared
    );

    let destination = [0x31; 16];
    let identity = [0x42; 16];
    let (commands, mut command_rx) = mpsc::channel(1);
    let actor = std::thread::spawn(move || {
        let Some(Command::ObservedIdentity(observed_destination, response)) =
            command_rx.blocking_recv()
        else {
            panic!("observed identity command was not admitted");
        };
        assert_eq!(observed_destination, destination);
        response
            .send(Ok(Some(identity)))
            .expect("publish identity response");
    });
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let (_done_tx, done) = std_mpsc::sync_channel(1);
    supervisor.lock_state().worker = Some(test_worker(
        commands,
        ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        },
        done,
        None,
        paths.root.clone(),
    ));

    assert!(matches!(
        foreign_block_on(admission::save_observed_destination_with_supervisor(&supervisor, ContactDestinationInput { destination })),
        ContactMutationOutcome::Saved { contact }
            if contact.destination == destination
                && contact.identity == Some(identity)
                && contact.alias.is_none()
                && !contact.pinned
    ));
    actor.join().expect("actor thread");

    let owner = supervisor
        .lock_state()
        .application_owner
        .take()
        .expect("save opened the application database");
    owner.close().expect("close application database");
    let reopened = DevelopmentStoreOwner::open(&paths.root, &paths.application)
        .expect("reopen application database");
    let reply = reopened
        .admit_directory_async(DirectoryRequest::Get { destination })
        .expect("admit contact lookup")
        .blocking_recv()
        .expect("receive contact lookup")
        .expect("contact lookup succeeds");
    assert!(matches!(
        reply,
        DirectoryResponse::Lookup(ContactLookupOutcome::Found { contact })
            if contact.destination == destination && contact.identity == Some(identity)
    ));
    reopened.close().expect("close reopened database");
}

#[test]
fn stopped_root_transition_closes_the_old_application_owner() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let first_storage = temporary
        .path()
        .join("first")
        .join("prns")
        .join("development");
    let second_storage = temporary
        .path()
        .join("second")
        .join("prns")
        .join("development");
    let first_paths = prepare_storage(&first_storage).expect("first private storage");
    let second_paths = prepare_storage(&second_storage).expect("second private storage");
    let mut state = test_supervisor_state(
        None,
        Some(
            DevelopmentStoreOwner::open(&first_paths.root, &first_paths.application)
                .expect("first application owner"),
        ),
        None,
    );

    assert_eq!(
        inspect_identity_locked(&mut state, &second_storage),
        PrimaryIdentityState::Missing
    );
    assert!(state.application_owner.is_none());
    assert_eq!(
        state.identity_owner.as_ref().map(|owner| &owner.root),
        Some(&second_paths.root)
    );
    redb::Database::open(&first_paths.application)
        .expect("the old application owner was closed before the root changed");
}

#[test]
fn running_root_transition_rejects_without_dropping_owned_state() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let first_storage = temporary
        .path()
        .join("first")
        .join("prns")
        .join("development");
    let second_storage = temporary
        .path()
        .join("second")
        .join("prns")
        .join("development");
    let first_paths = prepare_storage(&first_storage).expect("first private storage");
    let second_paths = prepare_storage(&second_storage).expect("second private storage");
    let (commands, _command_rx) = mpsc::channel(1);
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let (_done_tx, done) = std_mpsc::sync_channel(1);
    let mut state = test_supervisor_state(
        Some(test_worker(
            commands,
            ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            None,
            first_paths.root.clone(),
        )),
        Some(
            DevelopmentStoreOwner::open(&first_paths.root, &first_paths.application)
                .expect("first application owner"),
        ),
        Some(IdentityOwner {
            root: first_paths.root.clone(),
            vault: FileVault::new(&first_paths.identities),
        }),
    );

    assert!(matches!(
        ensure_application_owner_locked(&mut state, &second_paths),
        Err(DevelopmentStoreFailure::Unavailable(_))
    ));
    assert!(matches!(
        inspect_identity_locked(&mut state, &second_storage),
        PrimaryIdentityState::Unavailable { .. }
    ));
    assert_eq!(
        state.worker.as_ref().map(|worker| &worker.storage_root),
        Some(&first_paths.root)
    );
    assert_eq!(
        state.application_owner.as_ref().map(|owner| &owner.root),
        Some(&first_paths.root)
    );
    assert_eq!(
        state.identity_owner.as_ref().map(|owner| &owner.root),
        Some(&first_paths.root)
    );
    let response = state
        .application_owner
        .as_ref()
        .expect("application owner was retained")
        .admit_directory_async(DirectoryRequest::List)
        .expect("retained owner accepts work");
    assert!(matches!(
        response.blocking_recv().expect("receive retained owner response"),
        Ok(DirectoryResponse::List(ContactListOutcome::Listed { contacts }))
            if contacts.is_empty()
    ));

    state.worker = None;
    state
        .application_owner
        .take()
        .expect("application owner remains present")
        .close()
        .expect("close retained owner");
}

#[test]
fn stop_uses_priority_shutdown_when_the_snapshot_lane_is_saturated() {
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    };
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    supervisor.snapshots.set_local_host(running_host_state());

    let (commands, mut command_rx) = mpsc::channel(1);
    let (shutdown_sender, mut shutdown_rx) = watch::channel(false);
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let shutdown_requested_for_assertion = Arc::clone(&shutdown_requested);
    let shutdown = ShutdownSignal {
        sender: shutdown_sender,
        requested: shutdown_requested,
    };
    let (done_tx, done) = std_mpsc::sync_channel(1);
    let (inspection_started_tx, inspection_started_rx) = std_mpsc::sync_channel(1);
    let join = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let stalled_snapshot = command_rx.recv().await.expect("snapshot command");
            inspection_started_tx
                .send(())
                .expect("publish inspection start");
            shutdown_rx.changed().await.expect("priority shutdown");
            assert!(*shutdown_rx.borrow());
            drop(stalled_snapshot);
        });
        let _ = done_tx.send(Ok(()));
    });
    let (first_response, mut first_result) = oneshot::channel();
    assert!(commands.try_send(Command::Snapshot(first_response)).is_ok());
    inspection_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("snapshot inspection started");
    let (queued_response, mut queued_result) = oneshot::channel();
    assert!(commands
        .try_send(Command::Snapshot(queued_response))
        .is_ok());
    let mut state = test_supervisor_state(
        Some(test_worker(
            commands,
            shutdown,
            done,
            Some(join),
            PathBuf::from("/tmp/prns/development"),
        )),
        None,
        None,
    );

    assert_eq!(
        stop_locked(&supervisor, &mut state),
        DevelopmentNodeStopOutcome::Stopped
    );

    assert!(state.worker.is_none());
    assert!(shutdown_requested_for_assertion.load(Ordering::Acquire));
    assert_eq!(
        first_result.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    );
    assert_eq!(
        queued_result.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    );
    supervisor
        .snapshots
        .set_local_host_unavailable_if_running("late snapshot failure".to_owned());
    assert!(matches!(
        supervisor.snapshots.read().local_host,
        LocalHostState::Stopped { .. }
    ));
}

#[test]
fn incomplete_stop_remains_stopping_and_blocks_start_until_reset() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(true)),
        state: Mutex::new(SupervisorState::default()),
    };
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    let local_host = running_host_state();
    supervisor.snapshots.set_local_host(local_host.clone());

    let (commands, _command_rx) = mpsc::channel(1);
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let shutdown = ShutdownSignal {
        sender: shutdown_sender,
        requested: Arc::new(AtomicBool::new(false)),
    };
    let (done_tx, done) = std_mpsc::sync_channel(1);
    done_tx
        .send(Err((
            DevelopmentNodeStopStage::Worker,
            "shutdown did not settle".to_owned(),
        )))
        .expect("terminal worker result");
    let join = std::thread::spawn(|| {});
    supervisor.lock_state().worker = Some(test_worker(
        commands,
        shutdown,
        done,
        Some(join),
        paths.root,
    ));

    {
        let mut state = supervisor.lock_state();
        assert_eq!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                detail: "shutdown did not settle".to_owned(),
            }
        );
        assert!(state
            .worker
            .as_ref()
            .is_some_and(|worker| worker.incomplete_stop.is_some() && worker.join.is_none()));
    }
    assert!(!supervisor.operation_admitted.load(Ordering::Acquire));
    let incomplete = foreign_block_on(snapshot_async_with_supervisor(&supervisor));
    assert_eq!(incomplete.runtime, DevelopmentNodeRuntime::Stopping);
    assert_eq!(incomplete.local_host, local_host);
    assert_eq!(
        incomplete.failure,
        Some(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Runtime,
            detail: "shutdown did not settle".to_owned(),
        })
    );
    assert!(matches!(
        incomplete.active_operation,
        Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Shutdown,
            ..
        })
    ));

    assert!(matches!(
        start_configured_with_supervisor(
            &supervisor,
            &storage,
            DevelopmentNodeStartInput {
                development_tcp_target: None,
            },
            AppleBluetoothPreparation::WithoutRestoration,
        ),
        DevelopmentNodeStartOutcome::Failed {
            stage: DevelopmentNodeFailureStage::Runtime,
            ..
        }
    ));
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Stopping
    );

    let mut state = supervisor.lock_state();
    assert_eq!(
        stop_locked(&supervisor, &mut state),
        DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Worker,
            detail: "shutdown did not settle".to_owned(),
        }
    );
    assert!(state.worker.is_some());
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Stopping
    );
    drop(state);

    assert_eq!(
        reset_with_supervisor(&supervisor, &storage),
        DevelopmentNodeStopOutcome::Stopped
    );
    assert!(supervisor.lock_state().worker.is_none());
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Stopped
    );
}

#[test]
fn timed_out_stop_can_settle_successfully_when_retried() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    };
    let detail = "The native worker did not finish bounded shutdown.".to_owned();
    supervisor.snapshots.incomplete_stop(
        DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Runtime,
            detail: detail.clone(),
        },
        42,
    );

    let (commands, _command_rx) = mpsc::channel(1);
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let (done_tx, done) = std_mpsc::sync_channel(1);
    done_tx.send(Ok(())).expect("terminal worker result");
    let mut worker = test_worker(
        commands,
        ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(true)),
        },
        done,
        Some(std::thread::spawn(|| {})),
        paths.root,
    );
    worker.incomplete_stop = Some((DevelopmentNodeStopStage::Worker, detail));
    supervisor.lock_state().worker = Some(worker);

    assert!(matches!(
        start_configured_with_supervisor(
            &supervisor,
            &storage,
            DevelopmentNodeStartInput {
                development_tcp_target: None,
            },
            AppleBluetoothPreparation::WithoutRestoration,
        ),
        DevelopmentNodeStartOutcome::Failed {
            stage: DevelopmentNodeFailureStage::Runtime,
            ..
        }
    ));

    let mut state = supervisor.lock_state();
    assert_eq!(
        stop_locked(&supervisor, &mut state),
        DevelopmentNodeStopOutcome::Stopped
    );
    assert!(state.worker.is_none());
    assert_eq!(
        supervisor.snapshots.read().runtime,
        DevelopmentNodeRuntime::Stopped
    );
}

#[test]
fn stop_reaps_an_already_terminal_worker_and_preserves_failure_state() {
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(true)),
        state: Mutex::new(SupervisorState::default()),
    };
    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Running);
    supervisor.snapshots.set_local_host(running_host_state());
    let terminal = Err((
        DevelopmentNodeStopStage::Node,
        "node stopped unexpectedly".to_owned(),
    ));
    settle_worker_result(
        &supervisor.operation_admitted,
        &supervisor.snapshots,
        &terminal,
    );

    let (commands, _command_rx) = mpsc::channel(1);
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let shutdown = ShutdownSignal {
        sender: shutdown_sender,
        requested: Arc::new(AtomicBool::new(false)),
    };
    let (done_tx, done) = std_mpsc::sync_channel(1);
    done_tx.send(terminal).expect("terminal worker result");
    let join = std::thread::spawn(|| {});
    let mut state = test_supervisor_state(
        Some(test_worker(
            commands,
            shutdown,
            done,
            Some(join),
            PathBuf::from("/tmp/prns/development"),
        )),
        None,
        None,
    );

    assert_eq!(
        stop_locked(&supervisor, &mut state),
        DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Node,
            detail: "node stopped unexpectedly".to_owned(),
        }
    );
    assert!(state.worker.is_none());
    let snapshot = supervisor.snapshots.read();
    assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
    assert!(snapshot.active_operation.is_none());
    assert_eq!(
        snapshot.local_host,
        LocalHostState::Stopped {
            last_start_failure: Some("node stopped unexpectedly".to_owned()),
        }
    );
}

#[test]
fn stop_reaps_a_disconnected_terminal_worker_and_preserves_its_failure() {
    let supervisor = Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    };
    let prior_failure = DevelopmentNodeFailure {
        stage: DevelopmentNodeFailureStage::Node,
        detail: "node stopped unexpectedly".to_owned(),
    };
    supervisor.snapshots.terminal_fail(prior_failure.clone());

    let (commands, _command_rx) = mpsc::channel(1);
    let (shutdown_sender, _shutdown_rx) = watch::channel(false);
    let (_done_tx, done) = std_mpsc::sync_channel::<WorkerResult>(1);
    drop(_done_tx);
    let join = std::thread::spawn(|| {});
    let mut state = test_supervisor_state(
        Some(test_worker(
            commands,
            ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            Some(join),
            PathBuf::from("/tmp/prns/development"),
        )),
        None,
        None,
    );

    assert!(matches!(
        stop_locked(&supervisor, &mut state),
        DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Worker,
            ..
        }
    ));
    assert!(state.worker.is_none());
    let snapshot = supervisor.snapshots.read();
    assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
    assert_eq!(snapshot.failure, Some(prior_failure));
    assert!(snapshot.active_operation.is_none());
}

#[test]
fn primary_vault_malformed_and_operational_failures_are_distinct() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let paths = prepare_storage(&storage).expect("private storage");
    std::fs::write(paths.identities.join("primary"), [0_u8; 63])
        .expect("malformed primary identity");
    let mut state = SupervisorState::default();

    assert!(matches!(
        inspect_identity_locked(&mut state, &storage),
        PrimaryIdentityState::DevelopmentResetRequired { .. }
    ));
    assert!(matches!(
        primary_vault_failure(FileVaultError::Io(std::io::Error::other(
            "identity store unavailable"
        ))),
        PrimaryIdentityState::Unavailable { detail }
            if detail.contains("identity store unavailable")
    ));

    #[cfg(unix)]
    {
        let unavailable_storage = temporary
            .path()
            .join("unavailable")
            .join("prns")
            .join("development");
        let unavailable_paths = prepare_storage(&unavailable_storage).expect("private storage");
        std::os::unix::fs::symlink("primary", unavailable_paths.identities.join("primary"))
            .expect("identity symlink loop");
        let mut unavailable_state = SupervisorState::default();
        assert!(matches!(
            inspect_identity_locked(&mut unavailable_state, &unavailable_storage),
            PrimaryIdentityState::Unavailable { .. }
        ));
    }
}

#[test]
fn identity_and_node_reopen_reset_and_recreate_in_one_process() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let storage = temporary.path().join("prns").join("development");
    let imported = [0x42; 64];

    assert_eq!(
        preview_identity_import(&imported[..63]),
        IdentityImportPreviewOutcome::InvalidLength
    );
    let preview = preview_identity_import(&imported);
    let identity_hash = match preview {
        IdentityImportPreviewOutcome::Valid { identity_hash } => identity_hash,
        IdentityImportPreviewOutcome::InvalidLength => Vec::new(),
    };
    assert_eq!(identity_hash.len(), 16);
    assert_eq!(inspect_identity(&storage), PrimaryIdentityState::Missing);
    let (creation_tx, creation_rx) = std_mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let sender = creation_tx.clone();
            let storage = &storage;
            scope.spawn(move || {
                let _ = sender.send(create_imported_identity(storage, &imported));
            });
        }
    });
    drop(creation_tx);
    let creations = creation_rx.into_iter().collect::<Vec<_>>();
    assert_eq!(creations.len(), 2);
    assert_eq!(
        creations
            .iter()
            .filter(|outcome| matches!(outcome, IdentityCreationOutcome::Created { .. }))
            .count(),
        1
    );
    assert_eq!(
        creations
            .iter()
            .filter(|outcome| matches!(outcome, IdentityCreationOutcome::AlreadyExists))
            .count(),
        1
    );
    assert_eq!(
        create_generated_identity(&storage),
        IdentityCreationOutcome::AlreadyExists
    );
    assert_eq!(
        inspect_identity(&storage),
        PrimaryIdentityState::Present { identity_hash }
    );
    assert_eq!(
        admission::prepare_native_storage(&storage),
        NativeStoragePreparationOutcome::Prepared
    );
    let contact_destination = [0x24; 16];
    assert_eq!(
        foreign_block_on(admission::save_observed_destination(
            ContactDestinationInput {
                destination: contact_destination,
            }
        )),
        ContactMutationOutcome::LocalNodeStopped
    );
    assert!(matches!(
        foreign_block_on(admission::create_manual_contact(CreateManualContactInput {
            destination: contact_destination,
            identity: None,
            alias: Some(" Offline contact ".to_owned()),
        })),
        ContactMutationOutcome::Saved { .. }
    ));
    let wrong_storage = temporary
        .path()
        .join("wrong")
        .join("prns")
        .join("development");
    assert!(matches!(
        reset(&wrong_storage),
        DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Persistence,
            ..
        }
    ));
    assert!(storage.exists());
    assert!(matches!(
        foreign_block_on(admission::list_contacts()),
        ContactListOutcome::Listed { contacts }
            if contacts.len() == 1 && contacts[0].destination == contact_destination
    ));

    assert!(matches!(
        start(&storage),
        DevelopmentNodeStartOutcome::Started { .. }
    ));
    let running = foreign_block_on(snapshot_async());
    assert_eq!(running.runtime, DevelopmentNodeRuntime::Running);
    assert_eq!(running.lxmf.state, crate::contract::LxmfHealthState::Ready);
    assert_eq!(
        foreign_block_on(admission::list_lxmf_peers()),
        LxmfPeerListOutcome::Listed { peers: vec![] }
    );
    assert_eq!(
        foreign_block_on(admission::list_lxmf_messages(ListLxmfMessagesInput {
            peer: None,
            before: None,
            limit: 25,
        })),
        LxmfMessageListOutcome::Listed { messages: vec![] }
    );
    assert_eq!(
        foreign_block_on(admission::measure_lxmf_text(MeasureLxmfTextInput {
            title: String::new(),
            content: "x".repeat(319),
        })),
        MeasureLxmfTextOutcome::Measured {
            wire_bytes: 431,
            remaining_bytes: 0,
        }
    );
    assert_eq!(
        foreign_block_on(admission::send_direct_text(SendDirectTextInput {
            destination: [0x99; 16],
            title: "Unknown peer".to_owned(),
            content: "No message should be sent".to_owned(),
        })),
        SendDirectTextOutcome::PeerIdentityUnavailable
    );
    let LocalHostState::Running { host } = running.local_host else {
        panic!("the running generation did not publish its canonical Host snapshot");
    };
    assert_eq!(host.backend.backend(), BackendKind::Native);
    assert!(host.backend.supports(Capability::Bluetooth));
    assert_eq!(host.backend.capabilities().len(), 1);
    assert!(host.persistence.persistent);
    assert!(host.persistence.restored);
    assert!(host.revision >= 1);
    let captured_revision = host.revision;
    std::thread::sleep(Duration::from_millis(550));
    assert_eq!(
        host_revision(&supervisor().snapshots.read()),
        captured_revision
    );
    assert!(host_revision(&foreign_block_on(snapshot_async())) > captured_revision);
    let bluetooth_path = storage.join("identities").join("bluetooth-auto.identity");
    let bluetooth_record = std::fs::read(&bluetooth_path).unwrap_or_default();
    assert_eq!(bluetooth_record.len(), 40);
    assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
    #[cfg(any(feature = "apple", feature = "android", feature = "host-test"))]
    {
        assert!(matches!(
            start_configured(
                &storage,
                DevelopmentNodeStartInput {
                    development_tcp_target: Some("127.0.0.1:9".to_owned()),
                },
            ),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        let tcp_snapshot = foreign_block_on(snapshot_async());
        let LocalHostState::Running { host } = tcp_snapshot.local_host else {
            panic!("the TCP generation did not publish its Host snapshot");
        };
        assert!(host.backend.supports(Capability::TcpClient));
        assert!(host.backend.supports_interface(InterfaceKind::TcpClient));
        assert_eq!(
            std::fs::read(&bluetooth_path).unwrap_or_default(),
            bluetooth_record
        );
        assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
    }
    #[cfg(not(any(feature = "apple", feature = "android", feature = "host-test")))]
    {
        assert!(matches!(
            start_configured(
                &storage,
                DevelopmentNodeStartInput {
                    development_tcp_target: Some("127.0.0.1:9".to_owned()),
                },
            ),
            DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Contract,
                ..
            }
        ));
        assert!(matches!(
            start(&storage),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        assert_eq!(
            std::fs::read(&bluetooth_path).unwrap_or_default(),
            bluetooth_record
        );
        assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
    }
    std::fs::write(&bluetooth_path, [0_u8; 40]).expect("malformed Bluetooth record");
    assert!(matches!(
        start(&storage),
        DevelopmentNodeStartOutcome::Failed {
            stage: DevelopmentNodeFailureStage::Identity,
            ..
        }
    ));
    assert!(matches!(
        supervisor().snapshots.read().local_host,
        LocalHostState::DevelopmentResetRequired { .. }
    ));
    assert!(matches!(
        foreign_block_on(admission::list_contacts()),
        ContactListOutcome::Listed { contacts } if contacts.len() == 1
    ));
    let reset_guard = supervisor().lock_state();
    let (reset_tx, reset_rx) = std_mpsc::sync_channel(1);
    let reset_storage = storage.clone();
    let reset_thread = std::thread::spawn(move || {
        let _ = reset_tx.send(reset(&reset_storage));
    });
    assert!(matches!(
        reset_rx.recv_timeout(Duration::from_millis(25)),
        Err(std_mpsc::RecvTimeoutError::Timeout)
    ));
    drop(reset_guard);
    let reset_outcome =
        reset_rx
            .recv_timeout(STOP_TIMEOUT)
            .unwrap_or(DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                detail: "reset thread did not settle".to_owned(),
            });
    assert!(matches!(
        reset_outcome,
        DevelopmentNodeStopOutcome::AlreadyStopped
    ));
    let _ = reset_thread.join();
    assert!(!storage.exists());
    assert_eq!(inspect_identity(&storage), PrimaryIdentityState::Missing);
    assert_eq!(
        admission::prepare_native_storage(&storage),
        NativeStoragePreparationOutcome::Prepared
    );
    assert_eq!(
        foreign_block_on(admission::list_contacts()),
        ContactListOutcome::Listed { contacts: vec![] }
    );
    let generated_hash = match create_generated_identity(&storage) {
        IdentityCreationOutcome::Created { identity_hash } => identity_hash,
        _ => Vec::new(),
    };
    assert_eq!(generated_hash.len(), 16);
    supervisor().lock_state().identity_owner = None;
    assert_eq!(
        inspect_identity(&storage),
        PrimaryIdentityState::Present {
            identity_hash: generated_hash,
        }
    );
    assert!(matches!(
        start(&storage),
        DevelopmentNodeStartOutcome::Started { .. }
    ));
    assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
    assert!(matches!(
        reset(&storage),
        DevelopmentNodeStopOutcome::AlreadyStopped
    ));
}
