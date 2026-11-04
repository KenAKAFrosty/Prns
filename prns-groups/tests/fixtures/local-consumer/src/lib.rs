//! Independent consumer fixture: no text, native host, or Tokio executor.
#![forbid(unsafe_code)]
#[cfg(all(test, unix))]
mod tests {
    use prns_groups_core::{ClockInterval, ClockReading, GroupId, NamespaceId, SchemaId, Timestamp};
    use prns_groups::{
        fixture,
        owned::{LocalOptions, LocalOwner},
        ApiError, Client, ExecutionContext, Intent, OriginalScope, Outcome, Provisioned,
        PublicationIntent, RecordCursor, RejectionReason, Request, RequestId, RequestToken,
        Targets,
    };
    use prns_host::IdentityHash;
    use redb::ReadableDatabase;
    use std::{
        fs::{self, OpenOptions},
        future::Future,
        num::NonZeroUsize,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
        pin::pin,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
        thread,
    };

    const QUERY_PAGE: prns_groups::PageLimits = prns_groups::PageLimits {
        max_rows: 64,
        max_bytes: 64 * 1024,
    };

    const NOW: u64 = 1_800_000_000;
    const IDS: [IdentityHash; 2] = [IdentityHash::new([1; 16]), IdentityHash::new([2; 16])];
    struct WakeThread(thread::Thread);
    impl Wake for WakeThread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    // Poll ordinary Rust futures using only std; this deliberately has no runtime.
    fn wait<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(WakeThread(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => thread::park(),
            }
        }
    }
    fn token(value: u8) -> [u8; 32] {
        RequestToken::new(NOW, [value; 20]).unwrap().into_bytes()
    }
    fn clock() -> ClockReading {
        ClockReading::Trusted(ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap())
    }
    struct Owner {
        client: Client,
        local: LocalOwner,
    }
    fn options() -> LocalOptions {
        LocalOptions::new(NonZeroUsize::new(8).unwrap(), 1024 * 1024)
    }
    fn owner(local: LocalOwner) -> Owner {
        Owner {
            client: local.client(),
            local,
        }
    }
    fn create(path: PathBuf, identity: IdentityHash) -> Owner {
        owner(LocalOwner::create_new(path, identity, options()).unwrap())
    }
    fn reopen(path: PathBuf, identity: IdentityHash) -> Owner {
        owner(LocalOwner::open_existing(path, identity, options()).unwrap())
    }
    fn private_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }
    // Explicit test provisioning happens before standalone attachment. The
    // production owner receives installed configuration and never imports fixtures.
    fn install_fixed(path: &Path, fixture: &[u8], identity: IdentityHash) -> Provisioned {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        let database = redb::Database::builder().create_file(file).unwrap();
        fixture::load_and_install(&database, fixture, identity).unwrap()
    }
    fn context() -> ExecutionContext {
        ExecutionContext { clock: clock() }
    }
    fn mutation(client: &Client, id: RequestId, intent: Intent) -> Outcome {
        let original_scope = intent
            .group()
            .map(|group| wait(client.group_scope(group)).unwrap());
        wait(
            client
                .admit_execute(
                    Request {
                        id,
                        intent,
                        original_scope,
                    },
                    context(),
                )
                .unwrap(),
        )
        .unwrap()
    }
    fn publish(client: &Client, group: GroupId, nonce: u8) -> prns_groups_core::RecordId {
        let config = wait(client.for_group(group).network_config()).unwrap();
        let request = Request {
            id: RequestId(token(nonce)),
            original_scope: Some(OriginalScope::Control {
                control: config.control,
                incarnation: config.incarnation,
            }),
            intent: Intent::Publish(PublicationIntent {
                group,
                namespace: NamespaceId(0x5445_4c45),
                schema: SchemaId(19),
                targets: Targets::Explicit(vec![config.peers[0].endpoint]),
                body: vec![0, 255, 13, 7],
            }),
        };
        let Outcome::Published(id) = wait(client.execute(request.clone(), context())).unwrap()
        else {
            panic!("published record")
        };
        assert_eq!(
            wait(client.execute(request, context())).unwrap(),
            Outcome::Published(id)
        );
        let page = wait(client.records(group, None, 10, clock())).unwrap();
        assert_eq!(page.records[0].record.body.as_slice(), [0, 255, 13, 7]);
        assert!(!wait(client.changes(group, None, 10))
            .unwrap()
            .changes
            .is_empty());
        id
    }
    fn absent_catalog(path: &Path) {
        let db = redb::Database::open(path).unwrap();
        let read = db.begin_read().unwrap();
        let catalog = redb::TableDefinition::<u8, &[u8]>::new("prns_group_enrollment_v1");
        assert!(matches!(
            read.open_table(catalog),
            Err(redb::TableError::TableDoesNotExist(_))
        ));
    }

    #[test]
    fn fixed_opaque_records_reopen_without_creating_an_invitation_catalog() {
        let root = private_directory();
        let path = root.path().join("fixed.redb");
        let fixtures = fixture::generate("sensor fleet", &IDS, Timestamp(NOW)).unwrap();
        let configured = install_fixed(&path, &fixtures[0], IDS[0]);
        let local =
            owner(LocalOwner::open_fixed(&path, configured.clone(), IDS[0], options()).unwrap());
        let page = wait(local.client.conversations(None, QUERY_PAGE)).unwrap();
        assert!(page.next.is_none());
        assert_eq!(page.items.len(), 1);
        let group = page.items[0].group;
        assert_eq!(group, configured.group);
        assert!(wait(local.client.conversation(group, QUERY_PAGE))
            .unwrap()
            .profile
            .is_none());
        let id = publish(&local.client, group, 10);
        assert!(matches!(
            wait(
                local
                    .client
                    .admit_execute(
                        Request {
                            id: RequestId(token(11)),
                            intent: Intent::Create {
                                profile: Default::default(),
                                title: "wrong mode".into()
                            },
                            original_scope: None,
                        },
                        context()
                    )
                    .unwrap()
            ),
            Err(ApiError::NotProvisioned)
        ));
        wait(local.local.close()).unwrap();
        absent_catalog(&path);
        let reopened = owner(LocalOwner::open_fixed(&path, configured, IDS[0], options()).unwrap());
        assert_eq!(publish(&reopened.client, group, 10), id);
        wait(reopened.local.close()).unwrap();
        absent_catalog(&path);
    }
    fn transfer(
        from: &Client,
        from_identity: IdentityHash,
        to: &Client,
        to_identity: IdentityHash,
    ) {
        // Recovery/cleanup belongs to the owner turn, never each work page.
        wait(from.maintain(clock())).unwrap();
        let mut after = None;
        loop {
            let page = wait(from.enrollment_work(NOW, after, QUERY_PAGE)).unwrap();
            for work in page.items {
                assert_eq!(work.recipient, to_identity);
                let reply = wait(to.enrollment_handle(from_identity, work.frame, NOW)).unwrap();
                wait(from.enrollment_complete(work.id, reply, NOW)).unwrap();
            }
            match page.next {
                Some(next) => {
                    assert!(after.is_none_or(|after| next > after));
                    after = Some(next);
                }
                None => break,
            }
        }
    }
    #[test]
    fn managed_enrollment_and_generic_records_work_offline_without_a_runtime() {
        let root = private_directory();
        let path = root.path().join("owner.redb");
        let alice = create(path.clone(), IDS[0]);
        let bob = create(root.path().join("member.redb"), IDS[1]);
        let Outcome::Created(group) = mutation(
            &alice.client,
            RequestId(token(1)),
            Intent::Create {
                profile: Default::default(),
                title: "device telemetry".into(),
            },
        ) else {
            panic!("created group")
        };
        let Outcome::Invited(invitation) = mutation(
            &alice.client,
            RequestId(token(2)),
            Intent::Invite {
                group,
                recipient: IDS[1],
            },
        ) else {
            panic!("invitation")
        };
        transfer(&alice.client, IDS[0], &bob.client, IDS[1]);
        let page = wait(bob.client.invitations(NOW, None, QUERY_PAGE)).unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.next.is_none());
        mutation(
            &bob.client,
            RequestId(token(3)),
            Intent::Respond {
                invitation,
                accept: true,
            },
        );
        for _ in 0..4 {
            transfer(&bob.client, IDS[1], &alice.client, IDS[0]);
            transfer(&alice.client, IDS[0], &bob.client, IDS[1]);
        }
        assert_eq!(
            wait(bob.client.conversation(group, QUERY_PAGE))
                .unwrap()
                .member_count,
            2
        );
        // Multiple configured groups require generic queries to use their explicit
        // group argument even when there is no text feature or client scope.
        mutation(
            &alice.client,
            RequestId(token(7)),
            Intent::Create {
                profile: Default::default(),
                title: "separate telemetry".into(),
            },
        );
        let id = publish(&alice.client, group, 4);
        wait(alice.local.close()).unwrap();
        let reopened = reopen(path, IDS[0]);
        assert_eq!(publish(&reopened.client, group, 4), id);
        mutation(
            &reopened.client,
            RequestId(token(5)),
            Intent::Close { group },
        );
        mutation(
            &reopened.client,
            RequestId(token(6)),
            Intent::DeleteLocal { group },
        );
        let page = wait(reopened.client.conversations(None, QUERY_PAGE)).unwrap();
        assert!(page.next.is_none());
        assert!(page.items.iter().all(|entry| entry.group != group));
        wait(reopened.local.close()).unwrap();
        wait(bob.local.close()).unwrap();
    }
    fn pump(alice: &Client, bob: &Client) {
        for _ in 0..4 {
            transfer(bob, IDS[1], alice, IDS[0]);
            transfer(alice, IDS[0], bob, IDS[1]);
        }
    }
    fn join(alice: &Client, bob: &Client, group: GroupId, nonce: u8) {
        let Outcome::Invited(invitation) = mutation(
            alice,
            RequestId(token(nonce)),
            Intent::Invite {
                group,
                recipient: IDS[1],
            },
        ) else {
            panic!("invitation")
        };
        transfer(alice, IDS[0], bob, IDS[1]);
        mutation(
            bob,
            RequestId(token(nonce + 1)),
            Intent::Respond {
                invitation,
                accept: true,
            },
        );
        pump(alice, bob);
        assert_eq!(
            wait(bob.conversation(group, QUERY_PAGE))
                .unwrap()
                .member_count,
            2
        );
    }
    fn readmit(alice: &Client, bob: &Client, group: GroupId, nonce: u8) {
        mutation(bob, RequestId(token(nonce)), Intent::Leave { group });
        pump(alice, bob);
        assert_eq!(
            wait(bob.conversation(group, QUERY_PAGE)).unwrap().lifecycle,
            prns_groups::GroupLifecycle::Removed
        );
        join(alice, bob, group, nonce + 1);
    }
    #[test]
    fn opaque_retained_history_is_scoped_bounded_and_recoverable_after_readmission() {
        let root = private_directory();
        let bob_path = root.path().join("history-member.redb");
        let alice = create(root.path().join("history-owner.redb"), IDS[0]);
        let mut bob = create(bob_path.clone(), IDS[1]);
        let Outcome::Created(group) = mutation(
            &alice.client,
            RequestId(token(1)),
            Intent::Create {
                profile: Default::default(),
                title: "retained telemetry".into(),
            },
        ) else {
            panic!("group")
        };
        join(&alice.client, &bob.client, group, 10);
        let prior = wait(bob.client.for_group(group).network_config()).unwrap();
        let request = Request {
            id: RequestId(token(200)),
            original_scope: Some(OriginalScope::Control {
                control: prior.control,
                incarnation: prior.incarnation,
            }),
            intent: Intent::Publish(PublicationIntent {
                group,
                namespace: NamespaceId(0x5445_4c45),
                schema: SchemaId(19),
                targets: Targets::Explicit(vec![prior.peers[0].endpoint]),
                body: vec![0, 255, 13, 7],
            }),
        };
        let Outcome::Published(original) =
            wait(bob.client.execute(request.clone(), context())).unwrap()
        else {
            panic!("published record")
        };
        let initial = wait(bob.client.records(group, None, 1, clock())).unwrap();
        let position = initial.records[0].position;
        assert_eq!(initial.records[0].author_identity, IDS[1]);
        assert_eq!(position.incarnation, prior.incarnation);
        let old_head = initial.visited_heads[0];
        for round in 0..5 {
            readmit(&alice.client, &bob.client, group, 20 + round * 4);
        }
        wait(bob.local.close()).unwrap();
        bob = reopen(bob_path, IDS[1]);
        let first = wait(bob.client.records(group, None, 10, clock())).unwrap();
        assert!(first.records.is_empty());
        assert_eq!(
            first.visited_heads.len(),
            prns_groups::MAX_HISTORY_NAMESPACES_PER_PAGE
        );
        assert!(
            wait(
                bob.client
                    .record(group, first.current_incarnation, original, clock())
            )
            .unwrap()
            .is_none(),
            "record IDs do not search a different retained incarnation implicitly"
        );
        let continuation = first.next_before.unwrap();
        assert_eq!(
            RecordCursor::from_bytes(&continuation.to_bytes()).unwrap(),
            continuation
        );
        assert_ne!(continuation.incarnation, first.current_incarnation);
        // Readmission during paging cannot silently reinterpret the continuation.
        readmit(&alice.client, &bob.client, group, 60);
        let tail = wait(bob.client.records(group, Some(continuation), 10, clock())).unwrap();
        assert_ne!(tail.current_incarnation, first.current_incarnation);
        assert_eq!(tail.records.len(), 1);
        assert_eq!(tail.records[0].position, position);
        assert!(tail.records[0].progress.membership_changed);
        assert!(tail.next_before.is_none());
        assert_eq!(
            wait(
                bob.client
                    .record(group, prior.incarnation, original, clock())
            )
            .unwrap()
            .unwrap()
            .position,
            position
        );
        assert!(
            wait(
                bob.client
                    .delivery_progress(group, prior.incarnation, original, clock())
            )
            .unwrap()
            .unwrap()
            .membership_changed
        );
        assert_eq!(
            wait(bob.client.changes(group, Some(old_head), 10))
                .unwrap()
                .head
                .incarnation,
            prior.incarnation
        );
        // Exact old work is recovered before its stale original-control fence.
        let original_client = bob.client.for_group(group);
        assert_eq!(
            wait(original_client.execute(request.clone(), context())).unwrap(),
            Outcome::Published(original)
        );
        let mut conflict = request.clone();
        let Intent::Publish(publication) = &mut conflict.intent else {
            panic!("publication")
        };
        publication.body.push(9);
        assert!(matches!(
            wait(original_client.execute(conflict, context())),
            Err(ApiError::Rejected {
                reason: RejectionReason::RequestConflict
            })
        ));
        let mut unknown = request;
        unknown.id = RequestId(token(201));
        assert!(matches!(
            wait(original_client.execute(unknown, context())),
            Err(ApiError::Rejected {
                reason: RejectionReason::StaleMembership
            })
        ));
        let start = RecordCursor {
            group,
            incarnation: prior.incarnation,
            before: None,
        };
        assert_eq!(
            wait(bob.client.records(group, Some(start), 10, clock()))
                .unwrap()
                .records
                .len(),
            1
        );
        let wrong = RecordCursor {
            group: GroupId([99; 32]),
            ..start
        };
        assert!(wait(bob.client.records(group, Some(wrong), 10, clock())).is_err());
        let wrong = RecordCursor {
            incarnation: prns_groups_core::StoreIncarnation::new([99; 24]).unwrap(),
            ..start
        };
        assert!(wait(bob.client.records(group, Some(wrong), 10, clock())).is_err());
        let future = RecordCursor {
            before: Some(u64::MAX),
            ..start
        };
        assert!(wait(bob.client.records(group, Some(future), 10, clock())).is_err());
        mutation(
            &bob.client,
            RequestId(token(220)),
            Intent::DeleteLocal { group },
        );
        assert!(wait(bob.client.records(group, Some(start), 10, clock())).is_err());
        assert!(wait(bob.client.changes(group, Some(old_head), 10)).is_err());
        wait(bob.local.close()).unwrap();
        wait(alice.local.close()).unwrap();
    }
}
