use super::*;
use crate::owner::OwnedSession;
use crate::{HostAttachmentCleanup, HostEmbedding, HostPreparedAttachment};
use prns_host::{Bitrate, HostConfig, HostRole, IdentityConfig, PersistenceConfig, PrnsLimits};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Wake;
use std::time::{Duration, Instant};

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
// The public futures do not require an ambient Tokio executor.
fn run<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "native future exceeded test deadline"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
}
fn config() -> HostConfig {
    HostConfig {
        identity: IdentityConfig::GenerateEphemeral,
        persistence: PersistenceConfig::Ephemeral,
        role: HostRole::Endpoint,
        destinations: vec![],
        required_capabilities: vec![],
        limits: PrnsLimits::balanced(),
    }
}
fn tcp(port: u16) -> InterfaceConfig {
    InterfaceConfig::TcpClient {
        target: format!("127.0.0.1:{port}"),
        bitrate: Bitrate::Auto,
    }
}
fn auto() -> InterfaceConfig {
    InterfaceConfig::AutoLan {
        group_id: None,
        discovery_scope: None,
        discovery_port: None,
        data_port: None,
        devices: vec![],
        ignored_devices: vec![],
        multicast_address_type: None,
    }
}
fn command(config: InterfaceConfig) -> HostCommand {
    HostCommand::AttachInterface {
        config,
        routing: None,
    }
}

#[test]
fn claims_protect_managed_resources_and_keep_manual_interfaces_usable() {
    let owner = run(OwnedSession::open(config())).unwrap();
    let raw = owner.client();
    let scope = owner.attachment_scope().unwrap();
    let other = owner.attachment_scope().unwrap();
    let managed = run(scope.attach(tcp(9), None)).unwrap();
    assert_eq!(
        run(other.attach(tcp(9), None)),
        Err(AttachmentError::Conflict)
    );
    assert_eq!(
        run(raw.execute(command(tcp(9)))),
        Ok(Err(CommandFailure::Busy))
    );
    assert_eq!(
        run(raw.execute(HostCommand::AttachTcpClient {
            target: "127.0.0.1:9".into(),
            bitrate: Bitrate::Auto
        })),
        Ok(Err(CommandFailure::Busy))
    );
    assert!(matches!(
        run(raw.execute(HostCommand::DetachInterface { interface: managed })),
        Ok(Err(CommandFailure::PermissionDenied { .. }))
    ));
    assert_eq!(run(other.detach(managed)), Err(AttachmentError::NotOwned));
    let manual = match run(raw.execute(command(tcp(10)))).unwrap().unwrap() {
        CommandOutcome::InterfaceAttached { interface } => interface,
        _ => panic!("attach outcome"),
    };
    assert_eq!(
        run(scope.attach(tcp(10), None)),
        Err(AttachmentError::Conflict)
    );
    assert_eq!(run(scope.detach(manual)), Err(AttachmentError::NotOwned));
    scope.fence();
    assert_eq!(
        run(scope.attach(tcp(12), None)),
        Err(AttachmentError::Closed)
    );
    assert_eq!(run(raw.inspection()).unwrap().interfaces.len(), 2);
    run(scope.close()).unwrap();
    assert!(matches!(
        run(raw.execute(HostCommand::DetachInterface { interface: manual })),
        Ok(Ok(CommandOutcome::InterfaceDetached { .. }))
    ));
    let replacement = run(other.attach(tcp(9), None)).unwrap();
    run(other.detach(replacement)).unwrap();
    run(owner.stop()).unwrap();
}

#[test]
fn two_claims_racing_for_one_endpoint_cannot_replace_each_other() {
    let owner = run(OwnedSession::open(config())).unwrap();
    let left = owner.attachment_scope().unwrap();
    let right = owner.attachment_scope().unwrap();
    let barrier = std::sync::Barrier::new(2);
    let (a, b) = std::thread::scope(|threads| {
        let a = threads.spawn(|| {
            barrier.wait();
            run(left.attach(tcp(11), None))
        });
        let b = threads.spawn(|| {
            barrier.wait();
            run(right.attach(tcp(11), None))
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(a == Err(AttachmentError::Conflict) || b == Err(AttachmentError::Conflict));
    run(left.close()).unwrap();
    run(right.close()).unwrap();
    run(owner.stop()).unwrap();
}

struct Supervisor;
impl personal_rns::interfaces::ReportsStatus for Supervisor {}
impl personal_rns::runtime::InterfaceSupervisor for Supervisor {
    const KIND: personal_rns::interfaces::InterfaceKind =
        personal_rns::interfaces::InterfaceKind::AutoWifi;
    fn channel_tag(&self) -> &[u8] {
        b"attachment-claim-test"
    }
    fn policy(&self) -> personal_rns::interfaces::EffectiveInterfacePolicy {
        personal_rns::interfaces::tcp::policy_for_bitrate(
            personal_rns::interfaces::tcp::TCP_BITRATE_ESTIMATE,
        )
    }
    async fn run(self, _fleet: personal_rns::Fleet) {
        std::future::pending::<()>().await;
    }
}
fn embedding(cleanup: HostAttachmentCleanup, effects: Arc<AtomicUsize>) -> HostEmbedding {
    HostEmbedding {
        auto_lan_factory: Some(Arc::new(move |handle, _, _| {
            effects.fetch_add(1, Ordering::SeqCst);
            Ok(HostPreparedAttachment::ManagedSupervisor {
                attachment: handle.supervise(Supervisor),
                kind: InterfaceKind::AutoLan,
                cleanup: Arc::clone(&cleanup),
            })
        })),
        ..HostEmbedding::default()
    }
}

#[test]
fn failed_cleanup_retains_claim_until_explicit_retry_actually_finishes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let effects = Arc::new(AtomicUsize::new(0));
    let cleanup_calls = Arc::clone(&calls);
    let cleanup: HostAttachmentCleanup = Arc::new(move || {
        let calls = Arc::clone(&cleanup_calls);
        Box::pin(async move {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("injected unfinished retirement".into())
            } else {
                Ok(())
            }
        })
    });
    let owner = run(OwnedSession::open_with_embedding(
        config(),
        embedding(cleanup, Arc::clone(&effects)),
    ))
    .unwrap();
    let scope = owner.attachment_scope().unwrap();
    run(scope.attach(auto(), None)).unwrap();
    assert!(matches!(
        run(scope.close()),
        Err(AttachmentError::Command(
            CommandFailure::BackendFailed { .. }
        ))
    ));
    assert_eq!(
        run(owner.client().execute(command(auto()))),
        Ok(Err(CommandFailure::Busy))
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "conflict rejected before the factory has effects"
    );
    run(scope.close()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    run(owner.stop()).unwrap();
}

#[test]
fn cleanup_is_retained_when_waiters_vanish_and_ordinary_queue_is_full() {
    let (release, gate) = tokio::sync::oneshot::channel();
    let gate = Arc::new(Mutex::new(Some(gate)));
    let entered = Arc::new(AtomicBool::new(false));
    let entered_cleanup = Arc::clone(&entered);
    let cleanup: HostAttachmentCleanup = Arc::new(move || {
        let gate = lock(&gate).take();
        entered_cleanup.store(true, Ordering::Release);
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(())
        })
    });
    let owner = run(OwnedSession::open_with_embedding(
        config(),
        embedding(cleanup, Arc::new(AtomicUsize::new(0))),
    ))
    .unwrap();
    let scope = owner.attachment_scope().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let id = run(scope.attach(auto(), None)).unwrap();
    let detach = scope
        .host
        .submit_native(
            RuntimeCommand::Contract {
                command: HostCommand::DetachInterface { interface: id },
                claim: Some(Arc::clone(&scope.claim)),
                retirement: None,
            },
            None,
        )
        .unwrap();
    while !entered.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let mut accepted = Vec::new();
    loop {
        match scope.host.submit(HostCommand::CloseLink {
            link_id: prns_host::LinkId::new([0; 16]),
        }) {
            Ok(job) => accepted.push(job),
            Err(HostSubmitError::Busy) => break,
            Err(error) => panic!("unexpected admission {error:?}"),
        }
    }
    let close = scope.close();
    assert_eq!(
        run(scope.attach(tcp(12), None)),
        Err(AttachmentError::Closed)
    );
    drop(detach);
    drop(accepted);
    drop(close); // neither queue nor waiters own cleanup
    release.send(()).unwrap();
    run(scope.close()).unwrap();
    assert!(run(owner.client().inspection())
        .unwrap()
        .interfaces
        .is_empty());
    run(owner.stop()).unwrap();
}

#[test]
fn stopped_observer_does_not_request_stop_and_waits_for_provider_retirement() {
    let (release, gate) = tokio::sync::oneshot::channel();
    let gate = Arc::new(Mutex::new(Some(gate)));
    let entered = Arc::new(AtomicBool::new(false));
    let entered_cleanup = Arc::clone(&entered);
    let cleanup: HostAttachmentCleanup = Arc::new(move || {
        let gate = lock(&gate).take();
        entered_cleanup.store(true, Ordering::Release);
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            Ok(())
        })
    });
    let owner = run(OwnedSession::open_with_embedding(
        config(),
        embedding(cleanup, Arc::new(AtomicUsize::new(0))),
    ))
    .unwrap();
    let scope = owner.attachment_scope().unwrap();
    run(scope.attach(auto(), None)).unwrap();
    let mut stopped = Box::pin(scope.stopped());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(stopped.as_mut().poll(&mut cx).is_pending());
    assert!(owner.client().identity_hash().is_ok());
    drop(owner);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !entered.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(stopped.as_mut().poll(&mut cx).is_pending());
    release.send(()).unwrap();
    run(stopped).unwrap();
    run(scope.close()).unwrap();
}

#[test]
fn effective_resource_comparison_ignores_routing_and_serial_settings() {
    let original = signature(&tcp(9), None).unwrap();
    let changed = signature(
        &InterfaceConfig::TcpClient {
            target: "127.0.0.1:9".into(),
            bitrate: Bitrate::BitsPerSecond(9600),
        },
        Some(InterfaceRoutingPolicy {
            mode: Some(prns_host::InterfaceMode::Boundary),
            ..Default::default()
        }),
    )
    .unwrap();
    assert!(original.overlaps(&changed));
    assert!(!original.overlaps(&signature(&tcp(10), None).unwrap()));
    let line = prns_host::SerialLineConfig {
        baud: 9600,
        data_bits: prns_host::SerialDataBits::Eight,
        parity: prns_host::SerialParity::None,
        stop_bits: prns_host::SerialStopBits::One,
    };
    let serial_a = signature(
        &InterfaceConfig::Serial {
            port: "/dev/claimed-port".into(),
            line,
        },
        None,
    )
    .unwrap();
    let serial_b = signature(
        &InterfaceConfig::Serial {
            port: "/dev/claimed-port".into(),
            line: prns_host::SerialLineConfig {
                baud: 115200,
                ..line
            },
        },
        None,
    )
    .unwrap();
    assert!(serial_a.overlaps(&serial_b));
    let mut explicit = auto();
    if let InterfaceConfig::AutoLan {
        discovery_port,
        data_port,
        ..
    } = &mut explicit
    {
        *discovery_port = Some(29716);
        *data_port = Some(42671);
    }
    assert!(signature(&auto(), None)
        .unwrap()
        .overlaps(&signature(&explicit, None).unwrap()));
}

#[test]
fn lost_attach_waiter_keeps_exact_claim_and_scope_drop_cleans_it() {
    let effects = Arc::new(AtomicUsize::new(0));
    let cleaned = Arc::new(AtomicBool::new(false));
    let cleaned_callback = Arc::clone(&cleaned);
    let cleanup: HostAttachmentCleanup = Arc::new(move || {
        let cleaned = Arc::clone(&cleaned_callback);
        Box::pin(async move {
            cleaned.store(true, Ordering::Release);
            Ok(())
        })
    });
    let owner = run(OwnedSession::open_with_embedding(
        config(),
        embedding(cleanup, Arc::clone(&effects)),
    ))
    .unwrap();
    let scope = owner.attachment_scope().unwrap();
    let pending = scope
        .host
        .submit_native(
            RuntimeCommand::Contract {
                command: command(auto()),
                claim: Some(Arc::clone(&scope.claim)),
                retirement: None,
            },
            None,
        )
        .unwrap();
    drop(pending);
    let deadline = Instant::now() + Duration::from_secs(3);
    while effects.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    // The factory has started, so this is accepted work rather than a queued cancellation.
    drop(scope);
    while !cleaned.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(run(owner.client().inspection())
        .unwrap()
        .interfaces
        .is_empty());
    run(owner.stop()).unwrap();
}

#[test]
fn one_failing_cleanup_observer_does_not_strand_another_waiter() {
    struct PanicWake;
    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            panic!("test observer failure");
        }
    }
    let completion = Arc::new(Completion::default());
    let mut failed = Box::pin(Wait {
        completion: Arc::clone(&completion),
        registration: None,
    });
    let panic_waker = Waker::from(Arc::new(PanicWake));
    assert!(failed
        .as_mut()
        .poll(&mut Context::from_waker(&panic_waker))
        .is_pending());
    struct CountingWake(Arc<AtomicUsize>);
    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let wakes = Arc::new(AtomicUsize::new(0));
    let counting = Waker::from(Arc::new(CountingWake(Arc::clone(&wakes))));
    let mut waiting = Box::pin(Wait {
        completion: Arc::clone(&completion),
        registration: None,
    });
    assert!(waiting
        .as_mut()
        .poll(&mut Context::from_waker(&counting))
        .is_pending());
    completion.finish(Ok(()));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert_eq!(run(waiting), Ok(()));
    assert_eq!(run(failed), Ok(()));
}

#[test]
fn prepared_resources_are_not_silently_adopted_or_replaced() {
    let effects = Arc::new(AtomicUsize::new(0));
    let cleanup: HostAttachmentCleanup = Arc::new(|| Box::pin(async { Ok(()) }));
    let mut options = embedding(cleanup, Arc::clone(&effects));
    let prepared_id = Arc::new(Mutex::new(None));
    let id_out = Arc::clone(&prepared_id);
    options.prepare_interfaces = Some(Box::new(move |client| {
        let attachment = client.protocols().supervise(Supervisor);
        *lock(&id_out) = Some(crate::host_interface(attachment.id()));
        Ok(vec![HostPreparedAttachment::Supervisor {
            attachment,
            kind: InterfaceKind::AutoLan,
        }])
    }));
    let owner = run(OwnedSession::open_with_embedding(config(), options)).unwrap();
    let scope = owner.attachment_scope().unwrap();
    assert_eq!(
        run(scope.attach(auto(), None)),
        Err(AttachmentError::Conflict)
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    run(scope.close()).unwrap();
    let interface = lock(&prepared_id).expect("prepared ID");
    assert!(matches!(
        run(owner
            .client()
            .execute(HostCommand::DetachInterface { interface })),
        Ok(Ok(CommandOutcome::InterfaceDetached { .. }))
    ));
    run(owner.stop()).unwrap();
}

#[test]
fn partial_attach_failure_retains_exact_retirement_evidence_until_retry() {
    let owner = run(OwnedSession::open(config())).unwrap();
    let scope = owner.attachment_scope().unwrap();
    let claim = Arc::clone(&scope.claim);
    let result = run(owner.client().protocol_operation(move |handle| async move {
        let calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::clone(&calls);
        let cleanup: HostAttachmentCleanup = Arc::new(move || {
            let calls = Arc::clone(&cleanup_calls);
            Box::pin(async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("partial-plan cleanup failed".into())
                } else {
                    Ok(())
                }
            })
        });
        let resource = AttachmentResource::ManagedSupervisor {
            attachment: handle.supervise(Supervisor),
            kind: InterfaceKind::AutoLan,
            cleanup,
        };
        let expected = crate::host_interface(resource.interfaces()[0]);
        let mut entries = BTreeMap::new();
        let evidence: RetirementEvidence = Arc::new(Mutex::new(None));
        let result = complete_attach(
            &handle,
            &mut entries,
            resource,
            signature(&auto(), None).unwrap(),
            Some(&claim),
            Some(&evidence),
            Some(CommandFailure::ConnectFailed {
                detail: "later member failed".into(),
            }),
        )
        .await;
        assert!(matches!(result, Err(CommandFailure::BackendFailed { .. })));
        assert_eq!(lock(&evidence).as_ref().map(|(id, _)| *id), Some(expected));
        assert_eq!(entries.len(), 1);
        assert!(require_owner(entries.get(&expected).unwrap(), None).is_err());
        assert!(preflight(&entries, &auto(), None, None).is_err());
        let completion = Arc::new(Completion::default());
        Cleanup {
            claim,
            completion: Arc::clone(&completion),
        }
        .execute(&handle, &mut entries)
        .await;
        assert_eq!(completion.result(), Some(Ok(())));
        assert!(entries.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }));
    result.unwrap();
    run(owner.stop()).unwrap();
}

#[test]
fn equivalent_raw_target_spellings_cannot_bypass_claims() {
    let owner = run(OwnedSession::open(config())).unwrap();
    let scope = owner.attachment_scope().unwrap();
    run(scope.attach(
        InterfaceConfig::TcpClient {
            target: "localhost:9".into(),
            bitrate: Bitrate::Auto,
        },
        None,
    ))
    .unwrap();
    assert_eq!(
        run(owner.client().execute(HostCommand::AttachTcpClient {
            target: "LOCALHOST.:9".into(),
            bitrate: Bitrate::BitsPerSecond(9600)
        })),
        Ok(Err(CommandFailure::Busy))
    );
    let a = signature(
        &InterfaceConfig::TcpClient {
            target: "[0:0:0:0:0:0:0:1]:9".into(),
            bitrate: Bitrate::Auto,
        },
        None,
    )
    .unwrap();
    let b = signature(
        &InterfaceConfig::TcpClient {
            target: "[::1]:9".into(),
            bitrate: Bitrate::Auto,
        },
        None,
    )
    .unwrap();
    assert!(a.overlaps(&b));
    run(scope.close()).unwrap();
    run(owner.stop()).unwrap();
}
