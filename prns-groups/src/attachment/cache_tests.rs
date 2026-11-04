#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{
    client::{Command, Reply},
    GroupManager, Job,
};
use prns_groups_core::{ClockInterval, ClockReading, Timestamp};
use tokio::sync::{oneshot, Notify};

const IDENTITY: IdentityHash = IdentityHash::new([41; 16]);
const NOW: u64 = 1_800_000_000;

fn database(path: &std::path::Path) -> Arc<Database> {
    let database = Arc::new(Database::create(path).unwrap());
    let tx = database.begin_write().unwrap();
    Enrollment::initialize_in(&tx, IDENTITY).unwrap();
    prns_groups_store_redb::change::initialize_in(&tx, StoreIncarnation::new([42; 24]).unwrap())
        .unwrap();
    tx.commit().unwrap();
    database
}

fn execute(database: &Database, attachment: &mut Attachment, command: Command) -> Reply {
    dispatch(database, attachment, command).unwrap()
}

fn dispatch(
    database: &Database,
    attachment: &mut Attachment,
    command: Command,
) -> Result<Reply, Error> {
    let mut manager = GroupManager::open_attached(database, attachment).unwrap();
    dispatch_on(&mut manager, command)
}

fn dispatch_on(manager: &mut GroupManager<'_>, command: Command) -> Result<Reply, Error> {
    let (reply, result) = oneshot::channel();
    manager.execute(Job {
        command,
        group: None,
        control: None,
        reply,
        wake: Arc::new(Notify::new()),
        _input: None,
    });
    result.blocking_recv().unwrap()
}

#[test]
fn selected_stamp_is_checked_again_at_the_catalog_commit_boundary() {
    let directory = tempfile::tempdir().unwrap();
    for foreign in [false, true] {
        let database = database(&directory.path().join(format!("stamp-{foreign}.redb")));
        let mut attachment =
            Attachment::open_cached(database.clone(), IDENTITY, NonZeroUsize::MIN).unwrap();
        let mut manager = GroupManager::open_attached(&database, &mut attachment).unwrap();
        // Model out-of-contract damage after readiness. An optional commit
        // would silently skip a missing stamp; this selected lane must abort.
        let write = database.begin_write().unwrap();
        write
            .delete_table(redb::TableDefinition::<u8, &[u8]>::new(
                "prns_group_change_stamp_v1",
            ))
            .unwrap();
        if foreign {
            prns_groups_store_redb::change::initialize_in(
                &write,
                StoreIncarnation::new([99; 24]).unwrap(),
            )
            .unwrap();
        }
        write.commit().unwrap();
        let before = prns_groups_store_redb::change::read(&database).unwrap();
        assert!(matches!(
            dispatch_on(&mut manager, create_command(1, Default::default())),
            Err(Error::Enrollment(
                crate::enrollment::EnrollmentError::Storage(
                    prns_groups_store_redb::RedbError::Corrupt
                        | prns_groups_store_redb::RedbError::ForeignIncarnation,
                )
            ))
        ));
        let Reply::Conversations(rows) = dispatch_on(
            &mut manager,
            Command::Conversations {
                after: None,
                limit: crate::PageLimits {
                    max_rows: 8,
                    max_bytes: 65536,
                },
            },
        )
        .unwrap() else {
            panic!("expected conversations")
        };
        assert!(rows.items.is_empty() && rows.next.is_none());
        assert_eq!(
            prns_groups_store_redb::change::read(&database).unwrap(),
            before
        );
    }
}

fn create_command(id: u8, profile: crate::GroupProfile) -> Command {
    Command::Execute {
        request: crate::Request {
            id: crate::RequestToken::new(NOW, [id; 20]).unwrap().into(),
            original_scope: None,
            intent: crate::Intent::Create {
                title: "supported group".into(),
                profile,
            },
        },
        context: crate::ExecutionContext {
            clock: ClockReading::Trusted(
                ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
            ),
        },
    }
}

#[test]
fn runner_support_is_atomic_persists_across_views_and_cannot_be_widened() {
    let directory = tempfile::tempdir().unwrap();
    let database = database(&directory.path().join("groups.redb"));
    let mut attachment = Attachment::open(&database, IDENTITY).unwrap();
    let profile = crate::GroupProfile::default();
    let support = crate::ProfileSupport::new(
        profile.max_wire_bytes as usize,
        profile.max_enrollment_frame_bytes as usize,
    )
    .unwrap();
    let wide = crate::ProfileSupport::new(usize::MAX, usize::MAX).unwrap();
    let tight = crate::ProfileSupport::new(
        profile.max_wire_bytes as usize - 1,
        profile.max_enrollment_frame_bytes as usize,
    )
    .unwrap();
    execute(&database, &mut attachment, create_command(1, profile));
    let before = prns_groups_store_redb::change::read(&database).unwrap();
    // A stored incompatible profile prevents readiness without installing a
    // partial selection. The exact owner lane is also used by run_multi.
    assert!(matches!(
        dispatch(&database, &mut attachment, Command::ProfileSupport(tight)),
        Err(Error::Enrollment(
            crate::enrollment::EnrollmentError::UnsupportedProfile
        ))
    ));
    assert!(attachment.profile_support.is_none());
    for selected in [support, support, wide] {
        execute(
            &database,
            &mut attachment,
            Command::ProfileSupport(selected),
        );
        assert_eq!(attachment.profile_support, Some(support));
    }
    assert!(attachment.set_profile_support(&database, tight).is_err());
    assert_eq!(attachment.profile_support, Some(support));
    assert_eq!(
        prns_groups_store_redb::change::read(&database).unwrap(),
        before
    );
    let larger = crate::GroupProfile {
        max_wire_bytes: profile.max_wire_bytes + 1,
        ..profile
    };
    larger.validate().unwrap();
    assert!(matches!(
        dispatch(&database, &mut attachment, create_command(2, larger)),
        Err(Error::Enrollment(
            crate::enrollment::EnrollmentError::UnsupportedProfile
        ))
    ));
    execute(&database, &mut attachment, create_command(3, profile));
    // An attachment reopen revalidates persisted profiles under the new carrier;
    // it does not persist local support as group authority.
    let mut reopened = Attachment::open(&database, IDENTITY).unwrap();
    assert!(reopened.set_profile_support(&database, tight).is_err());
    assert!(reopened.profile_support.is_none());
    reopened.set_profile_support(&database, support).unwrap();
}

#[test]
fn sequential_manager_views_reuse_validation_without_retaining_read_transactions() {
    let directory = tempfile::tempdir().unwrap();
    let database = database(&directory.path().join("groups.redb"));
    let mut attachment =
        Attachment::open_cached(database.clone(), IDENTITY, NonZeroUsize::new(2).unwrap()).unwrap();
    let result = execute(
        &database,
        &mut attachment,
        Command::Execute {
            request: crate::Request {
                id: crate::RequestToken::new(NOW, [1; 20]).unwrap().into(),
                original_scope: None,
                intent: crate::Intent::Create {
                    title: "cached group".into(),
                    profile: Default::default(),
                },
            },
            context: crate::ExecutionContext {
                clock: ClockReading::Trusted(
                    ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
                ),
            },
        },
    );
    let Reply::Outcome(crate::Outcome::Created(group)) = result else {
        panic!("expected group")
    };
    execute(
        &database,
        &mut attachment,
        Command::Conversation {
            group,
            limit: crate::PageLimits {
                max_rows: 1,
                max_bytes: 65536,
            },
        },
    );
    let before = attachment.validation_stats().unwrap();
    for _ in 0..20 {
        let Reply::Conversation(conversation) = execute(
            &database,
            &mut attachment,
            Command::Conversation {
                group,
                limit: crate::PageLimits {
                    max_rows: 1,
                    max_bytes: 65536,
                },
            },
        ) else {
            panic!("expected conversations")
        };
        assert_eq!(conversation.group, group);
    }
    let after = attachment.validation_stats().unwrap();
    assert_eq!(after.full_validations, before.full_validations);
    assert_eq!(after.values_inspected, before.values_inspected);
    assert_eq!(after.receipts_inspected, before.receipts_inspected);
    assert!(after.cache_hits > before.cache_hits);
    // A writer is available between views: the retained cache pins allocation
    // identity, not a redb read snapshot or pages.
    database.begin_write().unwrap().abort().unwrap();
}

#[test]
fn identical_copied_store_cannot_retarget_a_cached_attachment() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original.redb");
    let copied = directory.path().join("copied.redb");
    let database = database(&original);
    let mut attachment =
        Attachment::open_cached(database.clone(), IDENTITY, NonZeroUsize::MIN).unwrap();
    std::fs::copy(&original, &copied).unwrap();
    let copied = Database::open(copied).unwrap();
    assert_eq!(
        prns_groups_store_redb::change::read(&database).unwrap(),
        prns_groups_store_redb::change::read(&copied).unwrap()
    );
    assert!(matches!(
        GroupManager::open_attached(&copied, &mut attachment),
        Err(Error::Configuration)
    ));
    assert!(GroupManager::open_attached(&database, &mut attachment).is_ok());
}

#[test]
fn dropped_attachment_releases_the_exact_database_allocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("groups.redb");
    let database = database(&path);
    let weak = Arc::downgrade(&database);
    let attachment =
        Attachment::open_cached(database.clone(), IDENTITY, NonZeroUsize::MIN).unwrap();
    drop(database);
    assert!(weak.upgrade().is_some());
    drop(attachment);
    assert!(weak.upgrade().is_none());
    let reopened = Database::open(path).unwrap();
    Attachment::open(&reopened, IDENTITY).unwrap();
}

#[test]
fn standalone_manager_cache_reuses_two_namespaces_after_confirmed_writes() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(&dir.path().join("standalone-cache"));
    let mut cache =
        prns_groups_store_redb::ValidationCache::new(db.clone(), NonZeroUsize::new(2).unwrap())
            .unwrap();
    let mut manager = GroupManager::open_managed(&db, IDENTITY)
        .unwrap()
        .with_validation_cache(&mut cache)
        .unwrap();
    let mut groups = Vec::new();
    for id in [1, 2] {
        let Reply::Outcome(crate::Outcome::Created(group)) =
            dispatch_on(&mut manager, create_command(id, Default::default())).unwrap()
        else {
            panic!("created group")
        };
        groups.push(group);
    }
    for _ in 0..50 {
        for group in &groups {
            let Reply::Conversation(row) = dispatch_on(
                &mut manager,
                Command::Conversation {
                    group: *group,
                    limit: crate::PageLimits {
                        max_rows: 1,
                        max_bytes: 65536,
                    },
                },
            )
            .unwrap() else {
                panic!("detail")
            };
            assert_eq!(row.group, *group);
        }
    }
    drop(manager);
    let before = cache.stats();
    assert_eq!(
        before.full_validations, 2,
        "first namespace opens only; confirmed writes reseed"
    );
    assert!(before.cache_hits >= 100);
    let mut manager = GroupManager::open_managed(&db, IDENTITY)
        .unwrap()
        .with_validation_cache(&mut cache)
        .unwrap();
    for group in groups {
        dispatch_on(
            &mut manager,
            Command::Conversation {
                group,
                limit: crate::PageLimits {
                    max_rows: 1,
                    max_bytes: 65536,
                },
            },
        )
        .unwrap();
    }
    drop(manager);
    let after = cache.stats();
    assert_eq!(before.values_inspected, after.values_inspected);
    assert_eq!(before.receipts_inspected, after.receipts_inspected);
    assert_eq!(before.full_validations, after.full_validations);
    assert!(after.cache_hits > before.cache_hits);
    let foreign = database(&dir.path().join("foreign"));
    assert!(matches!(
        GroupManager::open_managed(&foreign, IDENTITY)
            .unwrap()
            .with_validation_cache(&mut cache),
        Err(Error::Configuration)
    ));
}

#[test]
fn standalone_fixed_manager_uses_the_same_bounded_validation_cache() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::create(dir.path().join("fixed-cache")).unwrap());
    let fixtures = crate::fixture::generate(
        "fixed",
        &[IDENTITY, IdentityHash::new([43; 16])],
        Timestamp(NOW),
    )
    .unwrap();
    let cfg = crate::fixture::load_and_install(&db, &fixtures[0], IDENTITY).unwrap();
    let mut cache =
        prns_groups_store_redb::ValidationCache::new(db.clone(), NonZeroUsize::MIN).unwrap();
    let mut manager = GroupManager::open_fixed(&db, cfg.clone())
        .unwrap()
        .with_validation_cache(&mut cache)
        .unwrap();
    for _ in 0..50 {
        dispatch_on(
            &mut manager,
            Command::Conversation {
                group: cfg.group,
                limit: crate::PageLimits {
                    max_rows: 1,
                    max_bytes: 65536,
                },
            },
        )
        .unwrap();
    }
    drop(manager);
    let stats = cache.stats();
    assert_eq!(stats.full_validations, 1);
    assert!(stats.cache_hits >= 50);
    assert!(stats.values_inspected > 0);
}
