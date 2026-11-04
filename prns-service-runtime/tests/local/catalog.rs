use super::*;
use group::enrollment::{CatalogLimits, Enrollment};
use prns_identities::scoped_store::{self, ScopedDatabase};

#[test]
fn explicit_service_initialization_persists_catalog_limits_across_changed_defaults() {
    let root = Root::new();
    let selected = CatalogLimits {
        max_groups: Some(1000),
        max_catalog_bytes: 8 * 1024 * 1024,
        ..CatalogLimits::default()
    };
    let configured = options().with_catalog_limits(selected);
    let owner = groups::create_new::<4, 32, 4, 16>(&root.0, configured).unwrap();
    let identities = owner.identities();
    let identity = create(&identities, 13, core::ServiceSet::empty());
    let reference = identity.reference();
    assert!(matches!(
        transition(
            &identities,
            &identity,
            core::TransitionTarget::AddService(groups::SERVICE)
        ),
        core::OperationResult::ServiceAdded { .. }
    ));
    wait(owner.close()).unwrap();
    drop(owner);
    let path = scoped_store::path_under(reference, &root.0.join("domains")).unwrap();
    let inspect = || {
        let db =
            ScopedDatabase::open(&path, reference, configured.local.stores.per_identity).unwrap();
        let enrollment = Enrollment::open_existing(
            db.database(),
            group::IdentityHash::new(reference.public_hash.0),
        )
        .unwrap();
        assert_eq!(enrollment.limits().unwrap(), selected);
    };
    inspect();

    // Smaller construction defaults apply only to later explicitly added
    // services. They cannot rewrite a configured identity's existing policy.
    let changed = options().with_catalog_limits(CatalogLimits {
        max_groups: Some(1),
        ..CatalogLimits::default()
    });
    let owner = groups::open_existing::<4, 32, 4, 16>(&root.0, changed).unwrap();
    let groups = groups::client(owner.submission(), reference, |job| job);
    let page = wait(groups.conversations(None, QUERY_PAGE)).unwrap();
    assert!(page.items.is_empty() && page.next.is_none());
    wait(owner.close()).unwrap();
    drop(owner);
    inspect();
}

#[test]
fn invalid_catalog_policy_is_rejected_before_creating_the_composed_owner() {
    for limits in [
        CatalogLimits {
            max_groups: Some(0),
            ..CatalogLimits::default()
        },
        CatalogLimits {
            max_maintenance_rows: 0,
            ..CatalogLimits::default()
        },
        CatalogLimits {
            max_catalog_bytes: 1024,
            ..CatalogLimits::default()
        },
    ] {
        let root = Root::new();
        assert!(
            groups::create_new::<4, 32, 4, 16>(&root.0, options().with_catalog_limits(limits),)
                .is_err()
        );
        assert!(!root.0.exists());
    }
}
