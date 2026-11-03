use super::*;
use prns_identities::scoped_store::{self, ScopedDatabase};
use redb::ReadableDatabase;

fn limits() -> group::archive::Limits {
    group::archive::Limits {
        max_records: 64,
        max_bytes: 1024 * 1024,
    }
}
fn identity_hash(reference: core::IdentityReference) -> group::IdentityHash {
    group::IdentityHash::new(reference.public_hash.0)
}

#[test]
fn selected_archive_initializes_with_groups_and_reopens_from_persisted_policy() {
    let root = Root::new();
    let configured = options().with_archive(limits());
    let owner = groups::create_new::<4, 32, 4, 16>(&root.0, configured).unwrap();
    let client = owner.identities();
    let identity = create(&client, 11, core::ServiceSet::empty());
    let reference = identity.reference();
    assert!(matches!(
        transition(
            &client,
            &identity,
            core::TransitionTarget::AddService(groups::SERVICE)
        ),
        core::OperationResult::ServiceAdded { .. }
    ));
    wait(owner.close()).unwrap();
    drop(owner);
    let path = scoped_store::path_under(reference, &root.0.join("domains")).unwrap();
    {
        let db =
            ScopedDatabase::open(&path, reference, configured.local.stores.per_identity).unwrap();
        let read = db.database().begin_read().unwrap();
        let usage = group::archive::inspect_in(&read, identity_hash(reference))
            .unwrap()
            .unwrap();
        assert_eq!(usage.limits, limits());
        assert_eq!((usage.records, usage.bytes), (0, 0));
    }
    // No archive creation option is supplied on reopen. Durable selection, not
    // today's defaults, controls the actual ordinary attached group client.
    let owner = groups::open_existing::<4, 32, 4, 16>(&root.0, options()).unwrap();
    let history = groups::client(owner.submission(), reference, |job| job);
    let page = wait(history.conversations(None, QUERY_PAGE)).unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    wait(owner.close()).unwrap();
}

#[test]
fn orphan_archive_is_not_adopted_by_fresh_group_initialization() {
    let root = Root::new();
    let owner = groups::create_new::<4, 32, 4, 16>(&root.0, options()).unwrap();
    let identity = create(&owner.identities(), 12, core::ServiceSet::empty());
    let reference = identity.reference();
    wait(owner.close()).unwrap();
    drop(owner);
    let path = scoped_store::path_under(reference, &root.0.join("domains")).unwrap();
    {
        let db = ScopedDatabase::create_initialized(
            &path,
            reference,
            options().local.stores.per_identity,
            |tx| {
                group::archive::initialize_in(tx, identity_hash(reference), limits())
                    .map_err(|error| groups::SchemaError::Group(group::Error::Storage(error)))
            },
        )
        .unwrap();
        drop(db);
    }
    let owner = groups::open_existing::<4, 32, 4, 16>(&root.0, options()).unwrap();
    let client = owner.identities();
    let identity = client.identity(reference).unwrap();
    let (token, prepared) = begin(
        &client,
        &identity,
        core::TransitionTarget::AddService(groups::SERVICE),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while wait(identity.snapshot()).unwrap().last_error.is_none() {
        assert!(
            Instant::now() < deadline,
            "partial archive was not rejected"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        wait(client.operation(token, api::Command::Transition(prepared))).unwrap(),
        core::OperationState::Transitioning
    );
    wait(owner.close()).unwrap();
    drop(owner);
    let db = ScopedDatabase::open(&path, reference, options().local.stores.per_identity).unwrap();
    let read = db.database().begin_read().unwrap();
    assert!(matches!(
        group::enrollment::Enrollment::validate_in(&read, identity_hash(reference)),
        Err(group::enrollment::EnrollmentError::MissingCatalog)
    ));
    let usage = group::archive::inspect_in(&read, identity_hash(reference))
        .unwrap()
        .unwrap();
    assert_eq!(usage.limits, limits());
    assert_eq!(usage.records, 0);
}
