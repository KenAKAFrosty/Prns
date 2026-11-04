//! Real-file crash cuts exercise the complete service batch, independently of
//! native receipt-floor integration and physical media power-loss guarantees.
#![allow(clippy::unwrap_used, clippy::panic)]
#[path = "../../security/mls-rs/tests/support/durable.rs"]
mod durable;
use durable::{Cut, Directory, FileBackend};
use prns_groups_core::{AuthorizationEra, ControlId};
use prns_groups_mls::{Adapter, LocalIdentity, Policy};
use prns_groups_service::*;
use prns_groups_store::{Coordinator, RecoveryOutcome};

const GROUP: GroupId = GroupId([80; 32]);
const NOW: u64 = 1_792_000_000;
fn clock(now: u64) -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(now), Timestamp(now)).unwrap())
}
fn operation(value: u64) -> OperationId {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&value.to_be_bytes());
    OperationId(bytes)
}
fn endpoint() -> Endpoint {
    Endpoint {
        id: EndpointId([1; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}
fn limits() -> Limits {
    Limits {
        max_records: 4,
        max_targets: 1,
        max_body_bytes: 64,
        max_changes: 8,
        max_envelopes: 8,
        max_retained_wire_bytes: 8192,
        maximum_window: Seconds(100),
        future_skew: Seconds(0),
    }
}
fn mls_limits() -> prns_groups_mls::Limits {
    prns_groups_mls::Limits {
        max_members: 1,
        max_groups: 1,
        max_wire_bytes: 65536,
        max_plaintext_bytes: 4096,
        max_snapshot_bytes: 65536,
        max_key_packages: 1,
        max_retained_epochs: 1,
        max_pending_proposals: 1,
        max_pending_outputs: 12,
    }
}
fn store_limits() -> prns_groups_store::Limits {
    prns_groups_store::Limits {
        max_changes: 128,
        max_key_bytes: 256,
        max_value_bytes: 65536,
        max_staged_bytes: 1_048_576,
    }
}
fn adapter(
    directory: &Directory,
    identity: &LocalIdentity,
    max_changes: usize,
) -> Adapter<FileBackend> {
    let mut bounds = store_limits();
    bounds.max_changes = max_changes;
    Adapter::new(
        Coordinator::new(FileBackend::open(directory.path()).unwrap(), bounds).unwrap(),
        identity.clone(),
        Policy::new(identity.binding(), vec![identity.binding()]).unwrap(),
        mls_limits(),
    )
    .unwrap()
}
fn shared_adapter(directory: &Directory, identity: &LocalIdentity) -> Adapter<FileBackend> {
    let mut bounds = mls_limits();
    bounds.max_groups = 2;
    bounds.max_pending_outputs = 9;
    Adapter::new(
        Coordinator::new(FileBackend::open(directory.path()).unwrap(), store_limits()).unwrap(),
        identity.clone(),
        Policy::new(identity.binding(), vec![identity.binding()]).unwrap(),
        bounds,
    )
    .unwrap()
}
fn service<'a>(
    adapter: Adapter<FileBackend>,
    readers: &'a [Endpoint],
) -> GroupService<'a, FileBackend> {
    service_for(adapter, readers, GROUP)
}
fn service_for<'a>(
    adapter: Adapter<FileBackend>,
    readers: &'a [Endpoint],
    group: GroupId,
) -> GroupService<'a, FileBackend> {
    GroupService::new(
        adapter,
        Descriptor {
            group,
            control: ControlReference {
                head: ControlId([2; 32]),
                era: AuthorizationEra(ControlId([3; 32])),
            },
            readers,
        },
        limits(),
    )
    .unwrap()
}
fn request<'a>(nonce: u64, created: u64, targets: &'a [Endpoint]) -> Publication<'a> {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&nonce.to_be_bytes());
    Publication {
        nonce: bytes,
        namespace: NamespaceId(1),
        schema: SchemaId(1),
        validity: PublicationValidity::new(
            Timestamp(created),
            Timestamp(created + 10),
            Seconds(100),
        )
        .unwrap(),
        targets,
        body: b"retained until expiry",
    }
}

#[test]
fn service_maintenance_is_atomic_at_every_crash_cut_and_exact_after_lost_result() {
    for cut in [
        Cut::BeforeSnapshot,
        Cut::DuringSnapshot,
        Cut::AfterSnapshot,
        Cut::AfterPublish,
    ] {
        let directory = Directory::new().unwrap();
        let identity = LocalIdentity::generate([1; 32], 1).unwrap();
        let backend = FileBackend::open(directory.path()).unwrap();
        let faults = backend.faults();
        let initial = Adapter::new(
            Coordinator::new(backend, store_limits()).unwrap(),
            identity.clone(),
            Policy::new(identity.binding(), vec![identity.binding()]).unwrap(),
            mls_limits(),
        )
        .unwrap();
        initial.create(operation(1), GROUP).unwrap();
        let readers = [endpoint()];
        let mut svc = service(initial, &readers);
        let record = svc
            .publish(operation(2), &request(1, NOW, &readers), clock(NOW))
            .unwrap()
            .record;
        let wire = svc.outgoing(operation(2)).unwrap().unwrap();
        svc.emit_receipt(operation(3), record).unwrap();
        let snapshot = svc.snapshot().unwrap().unwrap();
        // A caught-up consumer must observe removal without another publication.
        let caught_up = svc.changes(ChangeCursor(0), 8).unwrap().head;
        let plan = svc.maintenance_plan(clock(NOW + 11), 1, 4).unwrap();
        let before = FileBackend::open(directory.path())
            .unwrap()
            .published_bytes()
            .unwrap();
        faults.arm(cut);
        assert!(svc.maintain(operation(4), &plan).is_err());
        assert!(svc.record(record).is_err()); // uncertain commit fences reads
        drop(svc);
        let mut svc = service(adapter(&directory, &identity, 128), &readers);
        let committed = matches!(cut, Cut::AfterPublish);
        assert_eq!(
            matches!(
                svc.coordinator().recover(operation(4)).unwrap(),
                RecoveryOutcome::Committed(_)
            ),
            committed
        );
        assert_eq!(svc.record(record).unwrap().is_none(), committed);
        let feed = svc.changes(caught_up, 8).unwrap();
        assert!(feed.changes.is_empty());
        assert_eq!(feed.head.0, caught_up.0 + u64::from(committed));
        assert_eq!(feed.next, feed.head);
        assert_eq!(
            svc.pending_operations().unwrap().contains(&operation(2)),
            !committed
        );
        if !committed {
            assert_eq!(
                FileBackend::open(directory.path())
                    .unwrap()
                    .published_bytes()
                    .unwrap(),
                before
            );
            assert_eq!(svc.outgoing(operation(2)).unwrap().unwrap(), wire);
            assert_eq!(svc.progress(record, clock(NOW)).unwrap().received, 1);
            assert_eq!(svc.snapshot().unwrap().unwrap(), snapshot);
        }
        let result = svc.maintain(operation(4), &plan).unwrap();
        assert_eq!(result.records_removed, 1);
        assert_eq!(result.operations_retired, 2);
        assert_eq!(svc.maintain(operation(4), &plan).unwrap(), result);
        let invalidated = svc.changes(caught_up, 8).unwrap();
        assert!(invalidated.changes.is_empty());
        assert_eq!(invalidated.head.0, caught_up.0 + 1);
        assert_eq!(invalidated.next, invalidated.head);
        assert_eq!(
            svc.changes(invalidated.next, 8).unwrap().head,
            invalidated.head,
            "an exact maintenance retry must not invalidate twice"
        );
        assert!(svc.record(record).unwrap().is_none());
        assert!(svc
            .publish(operation(2), &request(1, NOW, &readers), clock(NOW))
            .is_err());
        assert!(svc
            .publish(operation(5), &request(1, NOW, &readers), clock(NOW))
            .is_err()); // persisted expiry survives clock rollback
        assert!(svc.outgoing(operation(2)).unwrap().is_none());
    }
}

#[test]
fn bounded_batches_reuse_live_capacity_without_resetting_cursors_or_active_publications() {
    let directory = Directory::new().unwrap();
    let identity = LocalIdentity::generate([1; 32], 1).unwrap();
    let a = adapter(&directory, &identity, 128);
    a.create(operation(1), GROUP).unwrap();
    let readers = [endpoint()];
    let mut svc = service(a, &readers);
    let mut ticket = 2;
    let mut previous_head = ChangeCursor(0);
    // Cross the four-record/eight-envelope limits many times in one namespace.
    for batch in 0..256u64 {
        let now = NOW + batch * 20;
        for item in 0..4 {
            let record = svc
                .publish(
                    operation(ticket),
                    &request(batch * 4 + item, now, &readers),
                    clock(now),
                )
                .unwrap()
                .record;
            ticket += 1;
            svc.emit_receipt(operation(ticket), record).unwrap();
            ticket += 1;
        }
        let page = svc.changes(ChangeCursor(0), 8).unwrap();
        assert_eq!(page.changes.len(), 8);
        assert!(page.head > previous_head);
        previous_head = page.head;
        // Oldest first, one record and its two operations per atomic batch.
        for _ in 0..4 {
            let plan = svc.maintenance_plan(clock(now + 11), 1, 2).unwrap();
            assert_eq!(plan.records().len(), 1);
            svc.maintain(operation(ticket), &plan).unwrap();
            ticket += 1;
        }
        assert!(svc
            .maintenance_plan(clock(now + 12), 1, 2)
            .unwrap()
            .is_empty());
        assert_eq!(
            svc.changes(ChangeCursor(0), 8).unwrap().next,
            ChangeCursor(previous_head.0 + 4)
        );
        assert_eq!(svc.pending_operations().unwrap(), vec![operation(1)]);
    }
    assert_eq!(previous_head, ChangeCursor(3068));
    previous_head = ChangeCursor(previous_head.0 + 4);
    drop(svc);
    let svc = service(adapter(&directory, &identity, 128), &readers);
    assert_eq!(svc.changes(ChangeCursor(0), 8).unwrap().head, previous_head);
    assert!(svc.changes(ChangeCursor(0), 8).unwrap().changes.is_empty());
}

#[test]
fn failed_staging_and_stale_plan_leave_pending_records_and_outputs_unchanged() {
    let directory = Directory::new().unwrap();
    let identity = LocalIdentity::generate([1; 32], 1).unwrap();
    let a = adapter(&directory, &identity, 128);
    a.create(operation(1), GROUP).unwrap();
    let readers = [endpoint()];
    let mut svc = service(a, &readers);
    let id = svc
        .publish(operation(2), &request(1, NOW, &readers), clock(NOW))
        .unwrap()
        .record;
    let plan = svc.maintenance_plan(clock(NOW + 11), 1, 2).unwrap();
    drop(svc);
    let before = FileBackend::open(directory.path())
        .unwrap()
        .published_bytes()
        .unwrap();
    let mut svc = service(adapter(&directory, &identity, 8), &readers);
    assert!(svc.maintain(operation(3), &plan).is_err());
    assert_eq!(
        FileBackend::open(directory.path())
            .unwrap()
            .published_bytes()
            .unwrap(),
        before
    );
    assert!(svc.record(id).unwrap().is_some());
    assert!(svc.has_pending_receipt(id).unwrap());
    drop(svc);
    let mut svc = service(adapter(&directory, &identity, 128), &readers);
    svc.emit_receipt(operation(3), id).unwrap();
    assert!(matches!(
        svc.maintain(operation(4), &plan),
        Err(Error::Fault(Fault::ConfigurationChanged))
    ));
    assert_eq!(svc.progress(id, clock(NOW)).unwrap().received, 1);
    assert!(svc.outgoing(operation(2)).unwrap().is_some());
}

#[test]
fn expiry_plan_prioritizes_earliest_output_over_older_creation_time() {
    let directory = Directory::new().unwrap();
    let identity = LocalIdentity::generate([1; 32], 1).unwrap();
    let a = adapter(&directory, &identity, 128);
    a.create(operation(1), GROUP).unwrap();
    let readers = [endpoint()];
    let mut svc = service(a, &readers);
    let first = svc
        .publish(operation(2), &request(1, NOW + 5, &readers), clock(NOW + 5))
        .unwrap()
        .record;
    let older = svc
        .publish(operation(3), &request(2, NOW, &readers), clock(NOW + 5))
        .unwrap()
        .record;
    let plan = svc.maintenance_plan(clock(NOW + 20), 1, 1).unwrap();
    assert_eq!(plan.records(), &[first]);
    assert_eq!(plan.operations(), &[operation(2)]);
    svc.maintain(operation(4), &plan).unwrap();
    assert!(svc.record(older).unwrap().is_some());
    assert_eq!(svc.progress(older, clock(NOW + 20)).unwrap().received, 0);
}

#[test]
fn maintenance_preserves_other_groups_facts_outputs_and_records_in_shared_adapter() {
    const OTHER: GroupId = GroupId([81; 32]);
    let directory = Directory::new().unwrap();
    let identity = LocalIdentity::generate([1; 32], 1).unwrap();
    let open = || shared_adapter(&directory, &identity);
    let initial = open();
    initial.create(operation(1), GROUP).unwrap();
    initial.create(operation(2), OTHER).unwrap();
    let readers = [endpoint()];
    // The other group's output is earlier and also expired. Neither output
    // ordering nor a missing local body may make it eligible for this group.
    let mut other = service_for(initial, &readers, OTHER);
    let other_record = other
        .publish(operation(3), &request(1, NOW, &readers), clock(NOW))
        .unwrap()
        .record;
    let other_wire = other.outgoing(operation(3)).unwrap().unwrap();
    let other_snapshot = other.snapshot().unwrap().unwrap();
    drop(other);
    let mut own = service(open(), &readers);
    let own_record = own
        .publish(operation(4), &request(2, NOW, &readers), clock(NOW))
        .unwrap()
        .record;
    let plan = own.maintenance_plan(clock(NOW + 11), 1, 4).unwrap();
    assert_eq!(plan.records(), &[own_record]);
    assert_eq!(plan.operations(), &[operation(4)]);
    own.maintain(operation(5), &plan).unwrap();
    assert!(own.record(own_record).unwrap().is_none());
    assert!(own.outgoing(operation(4)).unwrap().is_none());
    assert!(own
        .maintenance_plan(clock(NOW + 11), 1, 4)
        .unwrap()
        .is_empty());
    drop(own);
    let mut other = service_for(open(), &readers, OTHER);
    assert!(other.record(other_record).unwrap().is_some());
    assert!(other.has_pending_receipt(other_record).unwrap());
    assert_eq!(other.outgoing(operation(3)).unwrap().unwrap(), other_wire);
    assert_eq!(other.snapshot().unwrap().unwrap(), other_snapshot);
    // The original operation fact remains replayable, without fresh encryption.
    let head = other.coordinator().head().unwrap();
    assert_eq!(
        other
            .publish(operation(3), &request(1, NOW, &readers), clock(NOW))
            .unwrap()
            .record,
        other_record
    );
    assert_eq!(other.coordinator().head().unwrap(), head);
    let plan = other.maintenance_plan(clock(NOW + 11), 1, 4).unwrap();
    assert_eq!(plan.operations(), &[operation(3)]);
    other.maintain(operation(6), &plan).unwrap();
    assert_eq!(
        other.pending_operations().unwrap(),
        vec![operation(1), operation(2)]
    );
}

#[test]
fn shared_adapter_reserves_output_capacity_for_other_groups_receipts() {
    const OTHER: GroupId = GroupId([81; 32]);
    let directory = Directory::new().unwrap();
    let identity = LocalIdentity::generate([1; 32], 1).unwrap();
    let initial = shared_adapter(&directory, &identity);
    initial.create(operation(1), GROUP).unwrap();
    initial.create(operation(2), OTHER).unwrap();
    let readers = [endpoint()];
    let mut own = service(initial, &readers);
    let record = own
        .publish(operation(3), &request(1, NOW, &readers), clock(NOW))
        .unwrap()
        .record;
    drop(own);
    let mut other = service_for(shared_adapter(&directory, &identity), &readers, OTHER);
    // The second group completes its own first record. Its next publication
    // must also leave room for the first group's still-pending receipt.
    let other_record = other
        .publish(operation(4), &request(2, NOW, &readers), clock(NOW))
        .unwrap()
        .record;
    other.emit_receipt(operation(5), other_record).unwrap();
    let head = other.coordinator().head().unwrap();
    assert!(matches!(
        other.publish(operation(6), &request(3, NOW, &readers), clock(NOW)),
        Err(Error::Fault(Fault::Capacity))
    ));
    assert_eq!(other.coordinator().head().unwrap(), head);
    assert_eq!(other.changes(ChangeCursor(0), 8).unwrap().changes.len(), 2);
    drop(other);
    let mut own = service(shared_adapter(&directory, &identity), &readers);
    assert!(own.has_pending_receipt(record).unwrap());
    own.emit_receipt(operation(6), record).unwrap();
    assert_eq!(own.progress(record, clock(NOW)).unwrap().received, 1);
}

#[test]
fn two_groups_rebuild_missing_reservations_without_consuming_output_capacity() {
    const OTHER: GroupId = GroupId([81; 32]);
    let directory = Directory::new().unwrap();
    let identity = LocalIdentity::generate([1; 32], 1).unwrap();
    let initial = shared_adapter(&directory, &identity);
    initial.create(operation(1), GROUP).unwrap();
    initial.create(operation(2), OTHER).unwrap();
    let readers = [endpoint()];
    let mut own = service(initial, &readers);
    let own_record = own
        .publish(operation(3), &request(1, NOW, &readers), clock(NOW))
        .unwrap()
        .record;
    drop(own);
    let mut other = service_for(shared_adapter(&directory, &identity), &readers, OTHER);
    let other_record = other
        .publish(operation(4), &request(2, NOW, &readers), clock(NOW))
        .unwrap()
        .record;
    drop(other);
    let migration = shared_adapter(&directory, &identity);
    migration
        .maintain(operation(5), &[], |tx| {
            for group in [GROUP, OTHER] {
                let mut key = [0; 33];
                key[0] = b'R';
                key[1..].copy_from_slice(&group.0);
                tx.delete(&key)?;
            }
            Ok::<_, prns_groups_store::StoreError<std::io::Error>>(())
        })
        .unwrap();
    let mut own = service(migration, &readers);
    let head = own.coordinator().head().unwrap();
    assert!(matches!(
        own.emit_receipt(operation(6), own_record),
        Err(Error::Fault(Fault::Capacity))
    ));
    assert_eq!(own.coordinator().head().unwrap(), head);
    // Each no-output maintenance transaction reconstructs only its own counter,
    // rebuilding multiple missing counters without an admission cycle.
    let plan = own.maintenance_plan(clock(NOW), 1, 4).unwrap();
    assert!(!plan.is_empty());
    assert!(plan.records().is_empty());
    assert!(plan.operations().is_empty());
    own.maintain(operation(6), &plan).unwrap();
    assert!(own.maintenance_plan(clock(NOW), 1, 4).unwrap().is_empty());
    drop(own);
    let mut other = service_for(shared_adapter(&directory, &identity), &readers, OTHER);
    let plan = other.maintenance_plan(clock(NOW), 1, 4).unwrap();
    other.maintain(operation(7), &plan).unwrap();
    assert!(other.maintenance_plan(clock(NOW), 1, 4).unwrap().is_empty());
    other.emit_receipt(operation(8), other_record).unwrap();
    drop(other);
    let mut own = service(shared_adapter(&directory, &identity), &readers);
    own.emit_receipt(operation(9), own_record).unwrap();
    assert_eq!(own.progress(own_record, clock(NOW)).unwrap().received, 1);
    assert_eq!(own.pending_operations().unwrap().len(), 6);
}
