//! Standalone lifetime and filesystem behavior without an async runtime.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::panic)]
use prns_groups::{
    enrollment::EnrollmentError,
    fixture,
    owned::{LocalIdentity, LocalOptions, LocalOwner, OpenError},
    Error, ExecutionContext, Intent, Outcome, Request, RequestId, RequestToken,
};
use prns_groups_core::Timestamp;
use prns_host::IdentityHash;
use redb::{ReadableDatabase, TableDefinition};
use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    num::NonZeroUsize,
    os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt},
    path::Path,
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
};

const QUERY_PAGE: prns_groups::PageLimits = prns_groups::PageLimits {
    max_rows: 64,
    max_bytes: 64 * 1024,
};

const ID: IdentityHash = IdentityHash::new([7; 16]);
const NOW: u64 = 1_800_000_000;
const CATALOG: TableDefinition<u8, &[u8]> = TableDefinition::new("prns_group_enrollment_v1");

struct WakeThread(thread::Thread);
impl Wake for WakeThread {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
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
fn options() -> LocalOptions {
    LocalOptions::new(NonZeroUsize::new(4).unwrap(), 1024 * 1024)
        .with_validation_cache_entries(NonZeroUsize::MIN)
}
fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}
fn private_file(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
}
fn catalog_bytes(path: &Path) -> Vec<u8> {
    let db = redb::Database::open(path).unwrap();
    let tx = db.begin_read().unwrap();
    let table = tx.open_table(CATALOG).unwrap();
    let value = table.get(0).unwrap().unwrap().value().to_vec();
    value
}

#[test]
fn dropped_owner_and_waiter_drain_accepted_creation_before_unlocking() {
    let directory = private_directory();
    let path = directory.path().join("groups.redb");
    let owner = LocalOwner::create_new(&path, ID, options()).unwrap();
    let client = owner.client();
    assert_eq!(wait(client.identity()).unwrap(), ID);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let request = Request {
        id: RequestId(RequestToken::new(NOW, [4; 20]).unwrap().into_bytes()),
        intent: Intent::Create {
            profile: Default::default(),
            title: "weather station".into(),
        },
        original_scope: None,
    };
    let context = ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
        ),
    };
    let admitted = client.admit_execute(request.clone(), context).unwrap();
    let dropped_close = owner.close();
    drop(dropped_close);
    assert!(matches!(
        client.admit_execute(request.clone(), context),
        Err(prns_groups_core::ApiError::Unavailable)
    ));
    let completion = owner.close();
    let repeated = owner.close();
    drop(admitted);
    drop(owner);
    wait(completion).unwrap();
    assert!(matches!(repeated.status(), Some(Ok(()))));
    wait(repeated).unwrap();
    assert!(matches!(
        wait(client.identity()),
        Err(prns_groups_core::ApiError::Unavailable)
    ));

    // Cleanup observation guarantees the database owner and its exclusive lock
    // have gone, although no OS thread join is performed on this test thread.
    let reopened = LocalOwner::open_existing(&path, ID, options()).unwrap();
    let client = reopened.client();
    let first = wait(client.admit_execute(request.clone(), context).unwrap()).unwrap();
    let Outcome::Created(group) = first else {
        panic!("creation result")
    };
    let second = wait(client.admit_execute(request, context).unwrap()).unwrap();
    assert!(matches!(second, Outcome::Created(recovered) if recovered == group));
    let page = wait(client.conversations(None, QUERY_PAGE)).unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_none());
    wait(reopened.close()).unwrap();
}

#[test]
fn exclusive_owner_create_existing_and_wrong_identity_preserve_catalog() {
    let directory = private_directory();
    let path = directory.path().join("groups.redb");
    let owner = LocalOwner::create_new(&path, ID, options()).unwrap();
    assert!(matches!(
        LocalOwner::open_existing(&path, ID, options()),
        Err(OpenError::Database(redb::Error::DatabaseAlreadyOpen))
    ));
    assert!(
        matches!(LocalOwner::create_new(&path, ID, options()), Err(OpenError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
    );
    wait(owner.close()).unwrap();
    let original = catalog_bytes(&path);
    assert!(matches!(
        LocalOwner::open_existing(&path, IdentityHash::new([8; 16]), options()),
        Err(OpenError::Domain(Error::Enrollment(
            EnrollmentError::Authentication
        )))
    ));
    assert_eq!(catalog_bytes(&path), original);
    wait(
        LocalOwner::open_existing(&path, ID, options())
            .unwrap()
            .close(),
    )
    .unwrap();
}

#[test]
fn absent_partial_and_corrupt_state_never_become_a_new_catalog() {
    let directory = private_directory();
    let path = directory.path().join("groups.redb");
    assert!(
        matches!(LocalOwner::open_existing(&path, ID, options()), Err(OpenError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound)
    );
    assert!(!path.exists());
    drop(private_file(&path));
    assert!(matches!(
        LocalOwner::open_existing(&path, ID, options()),
        Err(OpenError::Database(_))
    ));
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    fs::write(&path, b"partial database bytes").unwrap();
    assert!(matches!(
        LocalOwner::open_existing(&path, ID, options()),
        Err(OpenError::Database(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), b"partial database bytes");

    let empty = directory.path().join("no-schema.redb");
    drop(
        redb::Database::builder()
            .create_file(private_file(&empty))
            .unwrap(),
    );
    assert!(matches!(
        LocalOwner::open_existing(&empty, ID, options()),
        Err(OpenError::Domain(Error::Enrollment(
            EnrollmentError::MissingCatalog
        )))
    ));
    let database = redb::Database::open(empty).unwrap();
    assert_eq!(
        database
            .begin_read()
            .unwrap()
            .list_tables()
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn unsupported_or_corrupt_domain_catalog_is_preserved() {
    let directory = private_directory();
    let path = directory.path().join("groups.redb");
    wait(
        LocalOwner::create_new(&path, ID, options())
            .unwrap()
            .close(),
    )
    .unwrap();
    let original = catalog_bytes(&path);
    let mut unsupported: serde_json::Value = serde_json::from_slice(&original).unwrap();
    unsupported["version"] = 999.into();
    for bytes in [
        b"corrupt catalog".to_vec(),
        serde_json::to_vec(&unsupported).unwrap(),
    ] {
        let database = redb::Database::open(&path).unwrap();
        let transaction = database.begin_write().unwrap();
        transaction
            .open_table(CATALOG)
            .unwrap()
            .insert(0, bytes.as_slice())
            .unwrap();
        transaction.commit().unwrap();
        drop(database);
        assert!(matches!(
            LocalOwner::open_existing(&path, ID, options()),
            Err(OpenError::Domain(Error::Enrollment(
                EnrollmentError::Corrupt | EnrollmentError::UnsupportedVersion
            )))
        ));
        assert_eq!(catalog_bytes(&path), bytes);
    }
}

#[test]
fn fixed_attachment_validates_installed_state_without_enrollment_initialization() {
    let directory = private_directory();
    let path = directory.path().join("fixed.redb");
    let fixtures = fixture::generate(
        "telemetry",
        &[ID, IdentityHash::new([9; 16])],
        Timestamp(NOW),
    )
    .unwrap();
    let database = redb::Database::builder()
        .create_file(private_file(&path))
        .unwrap();
    let provisioned = fixture::load_and_install(&database, &fixtures[0], ID).unwrap();
    drop(database);
    assert!(matches!(
        LocalOwner::open_fixed(
            "invalid\0path",
            provisioned.clone(),
            IdentityHash::new([99; 16]),
            options()
        ),
        Err(OpenError::IdentityMismatch)
    ));
    let owner = LocalOwner::open_fixed(&path, provisioned.clone(), ID, options()).unwrap();
    let page = wait(owner.client().conversations(None, QUERY_PAGE)).unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next.is_none());
    wait(owner.close()).unwrap();
    let database = redb::Database::open(&path).unwrap();
    assert!(matches!(
        database.begin_read().unwrap().open_table(CATALOG),
        Err(redb::TableError::TableDoesNotExist(_))
    ));
    drop(database);
    wait(
        LocalOwner::open_fixed(&path, provisioned.clone(), ID, options())
            .unwrap()
            .close(),
    )
    .unwrap();

    let empty = directory.path().join("not-installed.redb");
    drop(
        redb::Database::builder()
            .create_file(private_file(&empty))
            .unwrap(),
    );
    assert!(LocalOwner::open_fixed(&empty, provisioned, ID, options()).is_err());
    let database = redb::Database::open(empty).unwrap();
    assert_eq!(
        database
            .begin_read()
            .unwrap()
            .list_tables()
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn unsafe_paths_and_invalid_budgets_fail_without_rewriting_them() {
    let directory = private_directory();
    let path = directory.path().join("groups.redb");
    for cache_bytes in [0, 1, 1024 * 1024 - 1, usize::MAX] {
        assert!(matches!(
            LocalOwner::create_new(
                &path,
                ID,
                LocalOptions::new(options().queue_capacity, cache_bytes)
            ),
            Err(OpenError::InvalidOptions)
        ));
        assert!(!path.exists());
    }
    assert!(matches!(
        LocalOwner::create_new(
            &path,
            ID,
            LocalOptions::new(NonZeroUsize::new(1025).unwrap(), options().cache_bytes)
        ),
        Err(OpenError::InvalidOptions)
    ));
    wait(
        LocalOwner::create_new(&path, ID, options())
            .unwrap()
            .close(),
    )
    .unwrap();
    let link = directory.path().join("link.redb");
    symlink(&path, &link).unwrap();
    assert!(matches!(
        LocalOwner::open_existing(&link, ID, options()),
        Err(OpenError::UnsafePath)
    ));
    assert!(matches!(
        LocalOwner::create_new(&link, ID, options()),
        Err(OpenError::Io(_))
    ));
    let hard = directory.path().join("hard.redb");
    fs::hard_link(&path, &hard).unwrap();
    assert!(matches!(
        LocalOwner::open_existing(&path, ID, options()),
        Err(OpenError::UnsafePermissions)
    ));
    fs::remove_file(hard).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        LocalOwner::open_existing(&path, ID, options()),
        Err(OpenError::UnsafePermissions)
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let parent_link = directory.path().join("parent-link");
    symlink(directory.path(), &parent_link).unwrap();
    assert!(matches!(
        LocalOwner::open_existing(parent_link.join("groups.redb"), ID, options()),
        Err(OpenError::UnsafePath)
    ));
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        LocalOwner::open_existing(&path, ID, options()),
        Err(OpenError::UnsafePermissions)
    ));
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    wait(
        LocalOwner::open_existing(path, ID, options())
            .unwrap()
            .close(),
    )
    .unwrap();
}

#[test]
fn retained_identity_outlives_the_database_and_failed_construction_releases_it() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Retained {
        path: std::path::PathBuf,
        observed: Arc<AtomicUsize>,
    }
    impl Drop for Retained {
        fn drop(&mut self) {
            // The group database must be released before its key capability.
            self.observed.store(
                if redb::Database::open(&self.path).is_ok() {
                    1
                } else {
                    2
                },
                Ordering::SeqCst,
            );
        }
    }
    let directory = private_directory();
    let path = directory.path().join("groups.redb");
    let observed = Arc::new(AtomicUsize::new(0));
    let retained = || {
        LocalIdentity::retain(
            ID,
            Retained {
                path: path.clone(),
                observed: observed.clone(),
            },
        )
    };
    let owner = LocalOwner::create_new(&path, retained(), options()).unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 0);
    let closed = owner.close();
    drop(owner);
    wait(closed).unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    observed.store(0, Ordering::SeqCst);
    assert!(LocalOwner::create_new(&path, retained(), options()).is_err());
    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "archive")]
#[test]
fn selected_archive_is_atomic_at_creation_and_reopen_uses_persisted_limits() {
    let directory = private_directory();
    let path = directory.path().join("archived.redb");
    let limits = prns_groups::archive::Limits {
        max_records: 17,
        max_bytes: 1024 * 1024,
    };
    let owner = LocalOwner::create_new(&path, ID, options().with_archive(limits)).unwrap();
    wait(owner.close()).unwrap();
    drop(owner);
    let database = redb::Database::open(&path).unwrap();
    assert_eq!(
        prns_groups::archive::inspect_in(&database.begin_read().unwrap(), ID)
            .unwrap()
            .unwrap()
            .limits,
        limits
    );
    drop(database);
    // Different fresh-creation preferences never mutate an existing schema.
    let owner = LocalOwner::open_existing(
        &path,
        ID,
        options().with_archive(prns_groups::archive::Limits {
            max_records: 1,
            max_bytes: 1024,
        }),
    )
    .unwrap();
    wait(owner.close()).unwrap();
    drop(owner);
    let database = redb::Database::open(&path).unwrap();
    assert_eq!(
        prns_groups::archive::usage_in(&database.begin_read().unwrap(), ID)
            .unwrap()
            .unwrap()
            .limits,
        limits
    );
    drop(database);
    let absent = directory.path().join("invalid-archive.redb");
    assert!(matches!(
        LocalOwner::create_new(
            &absent,
            ID,
            options().with_archive(prns_groups::archive::Limits {
                max_records: 0,
                max_bytes: 1024
            })
        ),
        Err(OpenError::InvalidOptions)
    ));
    assert!(!absent.exists());
}

#[test]
fn exact_invitation_client_query_enforces_group_and_control_scope() {
    let directory = private_directory();
    let owner =
        LocalOwner::create_new(directory.path().join("invitations.redb"), ID, options()).unwrap();
    let client = owner.client();
    let context = ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
        ),
    };
    let execute = |id: u8, intent: Intent| {
        let original_scope = intent
            .group()
            .map(|group| wait(client.group_scope(group)).unwrap());
        wait(client.execute(
            Request {
                id: RequestId(RequestToken::new(NOW, [id; 20]).unwrap().into_bytes()),
                intent,
                original_scope,
            },
            context,
        ))
        .unwrap()
    };
    let Outcome::Created(group) = execute(
        1,
        Intent::Create {
            title: "query".into(),
            profile: Default::default(),
        },
    ) else {
        panic!("created")
    };
    let Outcome::Invited(invitation) = execute(
        2,
        Intent::Invite {
            group,
            recipient: IdentityHash::new([9; 16]),
        },
    ) else {
        panic!("invited")
    };
    let detail = wait(client.conversation(group, QUERY_PAGE)).unwrap();
    let selected = client.for_group(group).at_control(detail.control);
    assert_eq!(
        wait(selected.invitation(invitation, NOW))
            .unwrap()
            .unwrap()
            .group,
        group
    );
    assert!(wait(
        client
            .for_group(prns_groups_core::GroupId([99; 32]))
            .invitation(invitation, NOW)
    )
    .unwrap()
    .is_none());
    let mut stale = detail.control;
    stale.head.0[0] ^= 1;
    assert!(matches!(
        wait(
            client
                .for_group(group)
                .at_control(stale)
                .invitation(invitation, NOW)
        ),
        Err(prns_groups::ApiError::Configuration)
    ));
    execute(3, Intent::CancelInvitation { invitation });
    assert_eq!(
        wait(client.invitation(invitation, NOW))
            .unwrap()
            .unwrap()
            .state,
        prns_groups::enrollment::InvitationState::Cancelled
    );
    execute(4, Intent::DismissInvitation { invitation });
    assert!(wait(client.invitation(invitation, NOW)).unwrap().is_none());
    wait(owner.close()).unwrap();
}
