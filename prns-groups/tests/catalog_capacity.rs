//! Deliberate local capacity selection through the ordinary standalone owner.
//! Frames use the canonical enrollment API directly; real Link qualification is
//! in group_only_onboarding. No LXMF, text, archive or network feature is needed.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use prns_groups::{
    create_request_id,
    enrollment::{CatalogLimits, Enrollment, EnrollmentError, InvitationState},
    owned::{LocalOptions, LocalOwner, OpenError},
    Client, Clock, ExecutionContext, IdentityHash, Intent, InvitationId, Outcome, Request,
    SystemClock,
};
use prns_groups_core::{ApiError, GroupId};
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use std::{num::NonZeroUsize, os::unix::fs::PermissionsExt, path::Path, time::Instant};

#[path = "support/catalog_scale.rs"]
mod scale;

const QUERY_PAGE: prns_groups::PageLimits = prns_groups::PageLimits {
    max_rows: 64,
    max_bytes: 64 * 1024,
};

const CATALOG: TableDefinition<u8, &[u8]> = TableDefinition::new("prns_group_enrollment_v1");
fn large() -> CatalogLimits {
    CatalogLimits {
        // An explicit small-fixture quota, not an implementation ceiling.
        max_groups: Some(32),
        max_catalog_bytes: 8 * 1024 * 1024,
        ..CatalogLimits::default()
    }
}
fn options() -> LocalOptions {
    LocalOptions::new(NonZeroUsize::new(16).unwrap(), 1024 * 1024)
}
fn directory() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
fn now() -> u64 {
    let prns_groups_core::ClockReading::Trusted(clock) = SystemClock.now() else {
        panic!("trusted test clock")
    };
    clock.lower().0
}
async fn execute(client: &Client, intent: Intent) -> Result<Outcome, ApiError> {
    let scope = match intent.group() {
        Some(group) => Some(client.group_scope(group).await?),
        None => None,
    };
    client
        .execute(
            Request {
                id: create_request_id().unwrap(),
                intent,
                original_scope: scope,
            },
            ExecutionContext {
                clock: SystemClock.now(),
            },
        )
        .await
}
async fn create(client: &Client, title: &str) -> GroupId {
    let Outcome::Created(group) = execute(
        client,
        Intent::Create {
            profile: Default::default(),
            title: title.into(),
        },
    )
    .await
    .unwrap() else {
        panic!("group")
    };
    group
}
async fn conversation_count(client: &Client) -> usize {
    let mut after = None;
    let mut count = 0;
    loop {
        let page = client
            .conversations(
                after,
                prns_groups::PageLimits {
                    max_rows: 7,
                    ..QUERY_PAGE
                },
            )
            .await
            .unwrap();
        assert!(page.items.len() <= 7);
        assert!(page
            .items
            .windows(2)
            .all(|rows| rows[0].group < rows[1].group));
        assert!(page
            .items
            .iter()
            .all(|row| after.is_none_or(|after| row.group > after)));
        count += page.items.len();
        match page.next {
            Some(next) => {
                assert!(after.is_none_or(|after| next > after));
                after = Some(next);
            }
            None => return count,
        }
    }
}
async fn exchange(sender: &Client, recipient: &Client) {
    sender.maintain(SystemClock.now()).await.unwrap();
    let identity = sender.identity().await.unwrap();
    let mut after = None;
    loop {
        let page = sender
            .enrollment_work(now(), after, QUERY_PAGE)
            .await
            .unwrap();
        for work in page.items {
            assert_eq!(work.recipient, recipient.identity().await.unwrap());
            let reply = recipient
                .enrollment_handle(identity, work.frame, now())
                .await
                .unwrap();
            sender
                .enrollment_complete(work.id, reply, now())
                .await
                .unwrap();
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
async fn join(owner: &Client, member: &Client, group: GroupId) -> InvitationId {
    let Outcome::Invited(invitation) = execute(
        owner,
        Intent::Invite {
            group,
            recipient: member.identity().await.unwrap(),
        },
    )
    .await
    .unwrap() else {
        panic!("invitation")
    };
    exchange(owner, member).await;
    let page = member.invitations(now(), None, QUERY_PAGE).await.unwrap();
    assert!(
        page.next.is_none(),
        "the fixture invitation catalog fits this page"
    );
    assert!(page
        .items
        .iter()
        .any(|i| i.id == invitation && i.state == InvitationState::Pending));
    execute(
        member,
        Intent::Respond {
            invitation,
            accept: true,
        },
    )
    .await
    .unwrap();
    for _ in 0..8 {
        exchange(member, owner).await;
        exchange(owner, member).await;
        let joined = |items: Vec<prns_groups::enrollment::Invitation>| {
            items
                .iter()
                .any(|i| i.id == invitation && i.state == InvitationState::Joined)
        };
        let outgoing = owner.invitations(now(), None, QUERY_PAGE).await.unwrap();
        let incoming = member.invitations(now(), None, QUERY_PAGE).await.unwrap();
        assert!(outgoing.next.is_none() && incoming.next.is_none());
        if joined(outgoing.items) && joined(incoming.items) {
            for client in [owner, member] {
                assert_eq!(
                    client
                        .for_group(group)
                        .network_config()
                        .await
                        .unwrap()
                        .readers
                        .len(),
                    2
                );
            }
            return invitation;
        }
    }
    panic!("canonical enrollment converges within bounded protocol exchanges")
}
fn catalog(path: &Path) -> Vec<u8> {
    let db = redb::Database::open(path).unwrap();
    let read = db.begin_read().unwrap();
    let table = read.open_table(CATALOG).unwrap();
    let bytes = table.get(0).unwrap().unwrap().value().to_vec();
    bytes
}
fn rewrite(path: &Path, update: impl FnOnce(&mut serde_json::Value)) {
    let db = redb::Database::open(path).unwrap();
    let tx = db.begin_write().unwrap();
    {
        let mut table = tx.open_table(CATALOG).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(table.get(0).unwrap().unwrap().value()).unwrap();
        update(&mut value);
        let bytes = serde_json::to_vec(&value).unwrap();
        table.insert(0, bytes.as_slice()).unwrap();
    }
    tx.commit().unwrap();
}

#[tokio::test]
async fn managed_configuration_requires_exact_scope_even_for_one_group() {
    let dir = directory();
    let owner = LocalOwner::create_new(
        dir.path().join("scope.redb"),
        IdentityHash::new([71; 16]),
        options(),
    )
    .unwrap();
    let client = owner.client();
    let group = create(&client, "Explicit selected group").await;
    assert!(matches!(
        client.network_config().await,
        Err(prns_groups::Error::NotProvisioned)
    ));
    let scope = client.group_scope(group).await.unwrap();
    let prns_groups::OriginalScope::Control { control, .. } = scope else {
        panic!("provisioned scope")
    };
    assert_eq!(
        client
            .for_group(group)
            .at_control(control)
            .network_config()
            .await
            .unwrap()
            .group,
        group
    );
    let mut stale = control;
    stale.head.0[0] ^= 1;
    assert!(matches!(
        client
            .for_group(group)
            .at_control(stale)
            .network_config()
            .await,
        Err(prns_groups::Error::Configuration)
    ));
    owner.close().await.unwrap();
}

#[tokio::test]
async fn enrollment_work_page_charges_actual_frame_and_rejects_zero_progress() {
    use prns_groups::{
        enrollment::{Work, WorkPage},
        PageLimits,
    };
    let dir = directory();
    let owner = LocalOwner::create_new(
        dir.path().join("work.redb"),
        IdentityHash::new([72; 16]),
        options(),
    )
    .unwrap();
    let client = owner.client();
    let minimum = PageLimits {
        max_rows: 1,
        max_bytes: (std::mem::size_of::<WorkPage>() + std::mem::size_of::<Work>()) as u64,
    };
    let empty = client.enrollment_work(now(), None, minimum).await.unwrap();
    assert!(empty.items.is_empty() && empty.next.is_none());
    let group = create(&client, "Small real enrollment frame").await;
    execute(
        &client,
        Intent::Invite {
            group,
            recipient: IdentityHash::new([73; 16]),
        },
    )
    .await
    .unwrap();
    let mut after = None;
    let mut visited = 0;
    loop {
        visited += 1;
        assert!(visited < 16, "small real catalog traversal terminates");
        let page = client
            .enrollment_work(
                now(),
                after,
                PageLimits {
                    max_bytes: 2 * 1024 * 1024,
                    ..minimum
                },
            )
            .await
            .unwrap();
        if let Some(work) = page.items.first() {
            let actual = minimum.max_bytes + work.frame.capacity() as u64;
            assert!(actual < u64::from(CatalogLimits::default().max_enrollment_frame_bytes));
            let compact = client
                .enrollment_work(
                    now(),
                    after,
                    PageLimits {
                        max_bytes: actual,
                        ..minimum
                    },
                )
                .await
                .unwrap();
            assert_eq!(compact.items.len(), 1);
            assert_eq!(compact.items[0].id, work.id);
            assert!(matches!(
                client.enrollment_work(now(), after, minimum).await,
                Err(prns_groups::Error::Enrollment(EnrollmentError::Capacity))
            ));
            break;
        }
        let next = page
            .next
            .expect("pending invitation has one active work row");
        assert!(after.is_none_or(|after| next > after));
        after = Some(next);
    }
    owner.close().await.unwrap();
}

#[tokio::test]
async fn thirty_two_joined_groups_reopen_with_selected_count_quota_and_cleanup() {
    let dir = directory();
    let paths = [dir.path().join("a.redb"), dir.path().join("b.redb")];
    let identities = [IdentityHash::new([1; 16]), IdentityHash::new([2; 16])];
    let startup = Instant::now();
    let owners: Vec<_> = paths
        .iter()
        .zip(identities)
        .map(|(path, id)| {
            LocalOwner::create_new(path, id, options().with_catalog_limits(large())).unwrap()
        })
        .collect();
    let startup_ms = startup.elapsed().as_millis();
    let clients: Vec<_> = owners.iter().map(LocalOwner::client).collect();
    let (alice, bob) = (&clients[0], &clients[1]);
    let creation = Instant::now();
    let mut groups = Vec::new();
    for index in 0..32 {
        let group = create(alice, &format!("Retained group {index}")).await;
        join(alice, bob, group).await;
        groups.push(group);
    }
    let creation_ms = creation.elapsed().as_millis();
    for client in &clients {
        assert_eq!(conversation_count(client).await, 32);
        for group in &groups {
            assert_eq!(
                client
                    .for_group(*group)
                    .network_config()
                    .await
                    .unwrap()
                    .group,
                *group
            );
        }
    }
    assert!(execute(
        alice,
        Intent::Create {
            profile: Default::default(),
            title: "Over selected group count".into()
        }
    )
    .await
    .unwrap_err()
    .is_storage_full());
    assert_eq!(conversation_count(alice).await, 32, "no eviction");
    for owner in &owners {
        owner.close().await.unwrap();
    }
    drop(owners);
    let catalog_bytes: Vec<_> = paths
        .iter()
        .map(|path| {
            let header: serde_json::Value = serde_json::from_slice(&catalog(path)).unwrap();
            header["bytes"].as_u64().unwrap() + header["reserved"].as_u64().unwrap()
        })
        .collect();
    let database_bytes: Vec<_> = paths
        .iter()
        .map(|path| std::fs::metadata(path).unwrap().len())
        .collect();
    for (path, identity) in paths.iter().zip(identities) {
        let db = redb::Database::open(path).unwrap();
        assert_eq!(
            Enrollment::open_existing(&db, identity)
                .unwrap()
                .limits()
                .unwrap(),
            large()
        );
    }
    // Caller defaults do not shrink, rewrite or migrate the persisted selection.
    let reopen = Instant::now();
    let owners: Vec<_> = paths
        .iter()
        .zip(identities)
        .map(|(path, id)| LocalOwner::open_existing(path, id, options()).unwrap())
        .collect();
    let reopen_ms = reopen.elapsed().as_millis();
    let alice = owners[0].client();
    let bob = owners[1].client();
    let inspect = Instant::now();
    assert_eq!(conversation_count(&alice).await, 32);
    let inspect_ms = inspect.elapsed().as_millis();
    // The selected membership quota is full. Cleanup remains available without
    // inventing the former count-derived receipt limit: v5 admits by actual
    // encoded bytes and funds each admitted cleanup obligation separately.
    let cleanup = Instant::now();
    for group in groups {
        execute(&alice, Intent::Close { group }).await.unwrap();
        exchange(&alice, &bob).await;
        execute(&alice, Intent::DeleteLocal { group })
            .await
            .unwrap();
        execute(&bob, Intent::DeleteLocal { group }).await.unwrap();
    }
    let page = alice.conversations(None, QUERY_PAGE).await.unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    let page = bob.conversations(None, QUERY_PAGE).await.unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    let cleanup_ms = cleanup.elapsed().as_millis();
    for owner in owners {
        owner.close().await.unwrap();
    }
    eprintln!("32 joined groups: startup_ms={startup_ms}, create_and_join_ms={creation_ms}, reopen_ms={reopen_ms}, inspect32_ms={inspect_ms}, cleanup32_at_selected_count_ms={cleanup_ms}, catalog_bytes={catalog_bytes:?}, database_bytes={database_bytes:?}");
    assert!(
        catalog_bytes
            .iter()
            .all(|bytes| *bytes < large().max_catalog_bytes),
        "actual catalog rows and funded cleanup fit the selected byte quota"
    );
}

#[tokio::test]
async fn capacity_selection_is_atomic_strict_and_rejects_older_catalogs() {
    let dir = directory();
    let identity = IdentityHash::new([4; 16]);
    let invalid_path = dir.path().join("invalid.redb");
    for limits in [
        CatalogLimits {
            max_groups: Some(0),
            ..large()
        },
        CatalogLimits {
            max_catalog_bytes: 1024,
            ..large()
        },
    ] {
        assert!(matches!(
            LocalOwner::create_new(
                &invalid_path,
                identity,
                options().with_catalog_limits(limits)
            ),
            Err(OpenError::InvalidOptions)
        ));
        assert!(
            !invalid_path.exists(),
            "invalid selection has no filesystem effects"
        );
    }
    // Minimal selectable capacity frees an entry only through explicit cleanup.
    let path = dir.path().join("one.redb");
    let owner = LocalOwner::create_new(
        &path,
        identity,
        options().with_catalog_limits(CatalogLimits {
            max_groups: Some(1),
            ..CatalogLimits::default()
        }),
    )
    .unwrap();
    let client = owner.client();
    let group = create(&client, "one").await;
    assert!(execute(
        &client,
        Intent::Create {
            profile: Default::default(),
            title: "full".into()
        }
    )
    .await
    .unwrap_err()
    .is_storage_full());
    execute(&client, Intent::Close { group }).await.unwrap();
    assert!(execute(
        &client,
        Intent::Create {
            profile: Default::default(),
            title: "closed still retained".into()
        }
    )
    .await
    .unwrap_err()
    .is_storage_full());
    execute(&client, Intent::DeleteLocal { group })
        .await
        .unwrap();
    create(&client, "explicit deletion frees one entry").await;
    owner.close().await.unwrap();
    drop(owner);

    let strict = dir.path().join("strict.redb");
    let owner = LocalOwner::create_new(&strict, identity, options()).unwrap();
    owner.close().await.unwrap();
    drop(owner);
    let original = catalog(&strict);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&original).unwrap()["version"],
        5
    );
    for update in 0..3 {
        rewrite(&strict, |value| {
            *value = serde_json::from_slice(&original).unwrap();
            match update {
                0 => {
                    value.as_object_mut().unwrap().remove("limits");
                }
                1 => {
                    value["limits"]["max_groups"] = 0.into();
                }
                _ => {
                    value["version"] = 6.into();
                }
            }
        });
        let before = catalog(&strict);
        assert!(LocalOwner::open_existing(&strict, identity, options()).is_err());
        assert_eq!(catalog(&strict), before, "unsupported state is untouched");
    }
    // Earlier catalog formats do not have this indexed schema. Reopen
    // rejects them without migration, even when the caller supplies new limits.
    for version in [2, 3, 4] {
        rewrite(&strict, |value| {
            *value = serde_json::from_slice(&original).unwrap();
            value["version"] = version.into();
            if version == 2 {
                value.as_object_mut().unwrap().remove("limits");
            }
        });
        let before = catalog(&strict);
        assert!(LocalOwner::open_existing(
            &strict,
            identity,
            options().with_catalog_limits(large())
        )
        .is_err());
        assert_eq!(catalog(&strict), before);
    }
    // Selected initialization participates in its caller's transaction: abort
    // creates neither an empty catalog nor a separate policy table.
    let empty = redb::Database::create(dir.path().join("abort.redb")).unwrap();
    let tx = empty.begin_write().unwrap();
    Enrollment::initialize_with_limits_in(&tx, identity, large()).unwrap();
    tx.abort().unwrap();
    assert!(matches!(
        Enrollment::open_existing(&empty, identity),
        Err(EnrollmentError::MissingCatalog)
    ));
}
