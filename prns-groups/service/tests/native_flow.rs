//! Three independently persisted endpoints exchange bytes directly. This proves
//! the shared library boundary, not a Reticulum transport or mobile lifecycle.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use prns_groups_core::{AuthorizationEra, ControlId};
use prns_groups_mls::{Adapter, Admission, Command, LocalIdentity, MemberBinding, Policy};
use prns_groups_service::{
    ChangeCursor, ChangeKind, ClockInterval, ClockReading, ControlReference, DeliverySummary,
    Descriptor, DurableReceipt, Endpoint, EndpointId, EndpointIncarnation, Error, Fault, GroupId,
    GroupService, Incoming, InventoryCursor, InventoryKind, InventorySnapshot, Limits, NamespaceId,
    OperationId, Publication, PublicationValidity, RecordId, RejectReason, SchemaId, Seconds,
    Timestamp,
};
use prns_groups_store::{Coordinator, OperationTicket, RecoveryOutcome, StoreIncarnation};
use prns_groups_store_redb::{RedbBackend, RedbLimits, RedbStoreOwner, StoreNamespace};
use redb::Database;

const GROUP: GroupId = GroupId([71; 32]);
const STORAGE: StoreNamespace = StoreNamespace([72; 16]);
const NOW: u64 = 1_792_000_000;
const CHAT: NamespaceId = NamespaceId(1);
const TELEMETRY: NamespaceId = NamespaceId(2);

fn control() -> ControlReference {
    ControlReference {
        head: ControlId([73; 32]),
        era: AuthorizationEra(ControlId([74; 32])),
    }
}
fn clock() -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(NOW + 1), Timestamp(NOW + 1)).unwrap())
}
fn endpoint(peer: usize) -> Endpoint {
    Endpoint {
        id: EndpointId([(peer + 1) as u8; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}
fn incarnation(peer: usize) -> StoreIncarnation {
    StoreIncarnation::new([(peer + 1) as u8; 24]).unwrap()
}
fn service_limits() -> Limits {
    Limits {
        max_records: 32,
        max_targets: 3,
        max_body_bytes: 1024,
        max_changes: 128,
        max_envelopes: 256,
        max_retained_wire_bytes: 1_048_576,
        maximum_window: Seconds(3600),
        future_skew: Seconds(0),
    }
}
fn mls_limits() -> prns_groups_mls::Limits {
    prns_groups_mls::Limits {
        max_members: 3,
        max_groups: 1,
        max_wire_bytes: 65_536,
        max_plaintext_bytes: 4096,
        max_snapshot_bytes: 65_536,
        max_key_packages: 4,
        max_retained_epochs: 1,
        max_pending_proposals: 8,
        max_pending_outputs: 128,
    }
}
fn store_limits() -> RedbLimits {
    RedbLimits {
        max_changes: 128,
        max_key_bytes: 256,
        max_value_bytes: 131_072,
        max_staged_bytes: 1_048_576,
        max_live_objects: 2048,
        max_live_bytes: 8_388_608,
        max_receipts: 256,
    }
}
fn coordinator_limits() -> prns_groups_store::Limits {
    store_limits().transaction_limits().unwrap()
}
fn next(peer: usize, coordinator: &Coordinator<RedbBackend<'_>>) -> OperationId {
    OperationTicket::new(incarnation(peer), coordinator.head().unwrap().0 + 1)
        .unwrap()
        .operation_id()
}
fn publication<'a>(
    nonce: u8,
    namespace: NamespaceId,
    targets: &'a [Endpoint],
    body: &'a [u8],
) -> Publication<'a> {
    Publication {
        nonce: [nonce; 32],
        namespace,
        schema: SchemaId(1),
        validity: PublicationValidity::new(Timestamp(NOW), Timestamp(NOW + 600), Seconds(3600))
            .unwrap(),
        targets,
        body,
    }
}

struct Trio {
    directory: tempfile::TempDir,
    bindings: [MemberBinding; 3],
    policy: Policy,
    readers: [Endpoint; 3],
    mls_limits: [prns_groups_mls::Limits; 3],
    service_limits: [Limits; 3],
}
impl Trio {
    fn enrolled() -> Self {
        Self::enrolled_with_mls_limits([mls_limits(); 3])
    }
    fn enrolled_with_mls_limits(mls_limits: [prns_groups_mls::Limits; 3]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let identities = [
            LocalIdentity::generate([1; 32], 1).unwrap(),
            LocalIdentity::generate([2; 32], 1).unwrap(),
            LocalIdentity::generate([3; 32], 1).unwrap(),
        ];
        let bindings = identities.each_ref().map(LocalIdentity::binding);
        let policy = Policy::new(bindings[0].clone(), bindings.to_vec()).unwrap();
        for (peer, identity) in identities.into_iter().enumerate() {
            // Separate plaintext test identity fixture, not a platform key store.
            let (_, secret) = identity.into_parts();
            std::fs::write(
                directory.path().join(format!("identity-{peer}")),
                secret.as_ref(),
            )
            .unwrap();
            let db = Database::create(directory.path().join(format!("peer-{peer}.redb"))).unwrap();
            let mut owner = RedbStoreOwner::new(&db);
            owner
                .initialize(STORAGE, incarnation(peer), store_limits())
                .unwrap();
        }
        let peers = Self {
            directory,
            bindings,
            policy,
            readers: [endpoint(0), endpoint(1), endpoint(2)],
            mls_limits,
            service_limits: [service_limits(); 3],
        };
        peers.with_adapter(0, |adapter| {
            adapter
                .create(next(0, adapter.coordinator()), GROUP)
                .unwrap();
        });
        let bob = peers.with_adapter(1, |adapter| {
            adapter
                .key_package(next(1, adapter.coordinator()), NOW)
                .unwrap()
        });
        let carol = peers.with_adapter(2, |adapter| {
            adapter
                .key_package(next(2, adapter.coordinator()), NOW)
                .unwrap()
        });
        let added = peers.with_adapter(0, |adapter| {
            adapter
                .add(
                    next(0, adapter.coordinator()),
                    GROUP,
                    &[&bob.output.messages[0], &carol.output.messages[0]],
                    NOW,
                )
                .unwrap()
        });
        for peer in [1, 2] {
            peers.with_adapter(peer, |adapter| {
                adapter
                    .join(
                        next(peer, adapter.coordinator()),
                        GROUP,
                        &added.output.messages[1],
                        NOW,
                    )
                    .unwrap();
            });
        }
        peers
    }
    fn identity(&self, peer: usize) -> LocalIdentity {
        let secret = std::fs::read(self.directory.path().join(format!("identity-{peer}"))).unwrap();
        LocalIdentity::from_parts(self.bindings[peer].clone(), secret.into()).unwrap()
    }
    fn with_adapter<T>(
        &self,
        peer: usize,
        action: impl FnOnce(&Adapter<RedbBackend<'_>>) -> T,
    ) -> T {
        let db = Database::open(self.directory.path().join(format!("peer-{peer}.redb"))).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        let backend = owner
            .open(STORAGE, incarnation(peer), store_limits())
            .unwrap();
        let coordinator = Coordinator::new(backend, coordinator_limits()).unwrap();
        let adapter = Adapter::new(
            coordinator,
            self.identity(peer),
            self.policy.clone(),
            self.mls_limits[peer],
        )
        .unwrap();
        action(&adapter)
    }
    fn with_service<T>(
        &self,
        peer: usize,
        action: impl FnOnce(&mut GroupService<RedbBackend<'_>>) -> T,
    ) -> T {
        let db = Database::open(self.directory.path().join(format!("peer-{peer}.redb"))).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        let backend = owner
            .open(STORAGE, incarnation(peer), store_limits())
            .unwrap();
        let coordinator = Coordinator::new(backend, coordinator_limits()).unwrap();
        let adapter = Adapter::new(
            coordinator,
            self.identity(peer),
            self.policy.clone(),
            self.mls_limits[peer],
        )
        .unwrap();
        let mut service = GroupService::new(
            adapter,
            Descriptor {
                group: GROUP,
                control: control(),
                readers: &self.readers,
            },
            self.service_limits[peer],
        )
        .unwrap();
        action(&mut service)
    }
    fn receive(&self, peer: usize, wire: &[u8]) -> Incoming {
        self.with_service(peer, |service| {
            service
                .receive(next(peer, service.coordinator()), wire, clock())
                .unwrap()
        })
    }
    fn receipt(&self, peer: usize, record: RecordId) -> Vec<u8> {
        let (operation, first) = self.with_service(peer, |service| {
            assert!(service.has_pending_receipt(record).unwrap());
            let operation = next(peer, service.coordinator());
            let emitted = service.emit_receipt(operation, record).unwrap();
            assert_eq!(emitted.record, record);
            (
                operation,
                service.outgoing(operation).unwrap().unwrap().to_vec(),
            )
        });
        self.with_service(peer, |service| {
            let queued = service.changes(ChangeCursor(0), self.service_limits[peer].max_changes.min(128)).unwrap().changes.into_iter()
                .find(|change| matches!(change.kind, ChangeKind::ReceiptQueued(receipt) if receipt.record == record))
                .unwrap();
            assert_eq!(queued.operation, operation);
            assert_eq!(
                service.outgoing(queued.operation).unwrap().unwrap().as_slice(),
                first
            );
            assert_eq!(
                service.emit_receipt(operation, record).unwrap().record,
                record
            );
        });
        first
    }
    fn progress(&self, peer: usize, record: RecordId) -> DeliverySummary {
        self.with_service(peer, |service| service.progress(record, clock()).unwrap())
    }
}

#[test]
fn operation_facts_distinguish_service_work_from_enrollment_after_reopen() {
    let peers = Trio::enrolled();
    let enrollment = peers.with_adapter(0, |adapter| adapter.pending_operations().unwrap());
    assert!(!enrollment.is_empty());
    let (published, record, wire) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        let record = service
            .publish(
                operation,
                &publication(91, CHAT, &[endpoint(1)], b"retained service fact"),
                clock(),
            )
            .unwrap()
            .record;
        let wire = service.outgoing(operation).unwrap().unwrap().to_vec();
        (operation, record, wire)
    });
    let received = peers.with_service(1, |service| {
        let operation = next(1, service.coordinator());
        assert_eq!(
            service.receive(operation, &wire, clock()).unwrap(),
            Incoming::Record(record),
        );
        operation
    });
    let receipt = peers.with_service(1, |service| {
        let operation = next(1, service.coordinator());
        service.emit_receipt(operation, record).unwrap();
        operation
    });

    // Reopen the actual adapter rather than relying on an in-memory service.
    // The predicate recognizes all three fact kinds without claiming MLS-only
    // enrollment operations, and reading it cannot advance the durable head.
    for (peer, operation) in [(0, published), (1, received), (1, receipt)] {
        peers.with_adapter(peer, |adapter| {
            let before = adapter.coordinator().head().unwrap();
            assert!(prns_groups_service::has_operation_fact(adapter, operation).unwrap());
            assert!(
                !prns_groups_service::has_operation_fact(adapter, OperationId([255; 32]),).unwrap()
            );
            if peer == 0 {
                for operation in &enrollment {
                    assert!(!prns_groups_service::has_operation_fact(adapter, *operation).unwrap());
                }
            }
            assert_eq!(adapter.coordinator().head().unwrap(), before);
        });
    }
}

#[test]
fn pending_delivery_rotates_durable_authored_work_and_stops_only_for_the_receipted_target() {
    let peers = Trio::enrolled();
    let targets = [endpoint(1), endpoint(2)];
    let mut records = Vec::new();
    for nonce in [51, 52] {
        records.push(peers.with_service(0, |service| {
            let operation = next(0, service.coordinator());
            let published = service
                .publish(
                    operation,
                    &publication(nonce, CHAT, &targets, b"push once"),
                    clock(),
                )
                .unwrap();
            (
                published.record,
                service.outgoing(operation).unwrap().unwrap().to_vec(),
            )
        }));
    }
    let first = peers.with_service(0, |service| {
        service
            .pending_delivery(endpoint(1), InventoryCursor(0), clock())
            .unwrap()
            .unwrap()
    });
    assert_eq!(first.record, records[0].0);
    let second = peers.with_service(0, |service| {
        service
            .pending_delivery(endpoint(1), first.position, clock())
            .unwrap()
            .unwrap()
    });
    assert_eq!(second.record, records[1].0);
    peers.with_service(0, |service| {
        assert_eq!(
            service
                .pending_delivery(endpoint(1), second.position, clock())
                .unwrap(),
            Some(first)
        );
        assert_eq!(
            service.envelope(first.id).unwrap().unwrap().as_slice(),
            records[0].1
        );
        assert!(service
            .pending_delivery(endpoint(1), InventoryCursor(0), ClockReading::Unknown)
            .unwrap()
            .is_none());
        let expired = ClockReading::Trusted(
            ClockInterval::new(Timestamp(NOW + 600), Timestamp(NOW + 600)).unwrap(),
        );
        assert!(service
            .pending_delivery(endpoint(1), InventoryCursor(0), expired)
            .unwrap()
            .is_none());
        assert!(matches!(
            service.pending_delivery(endpoint(0), InventoryCursor(0), clock()),
            Err(Error::Fault(Fault::InvalidTargets))
        ));
        assert!(matches!(
            service.pending_delivery(endpoint(3), InventoryCursor(0), clock()),
            Err(Error::Fault(Fault::InvalidTargets))
        ));
    });
    // Receiving content alone cannot clear its author's durable obligation.
    assert_eq!(
        peers.receive(1, &records[0].1),
        Incoming::Record(records[0].0)
    );
    peers.with_service(0, |service| {
        assert_eq!(
            service
                .pending_delivery(endpoint(1), InventoryCursor(0), clock())
                .unwrap(),
            Some(first)
        );
    });
    // Received copies are available for pull repair, not automatically fanned out.
    peers.with_service(1, |service| {
        assert!(service
            .pending_delivery(endpoint(2), InventoryCursor(0), clock())
            .unwrap()
            .is_none());
    });
    let receipt = peers.receipt(1, records[0].0);
    assert!(matches!(peers.receive(0, &receipt), Incoming::Receipt(_)));
    peers.with_service(0, |service| {
        assert_eq!(
            service
                .pending_delivery(endpoint(1), InventoryCursor(0), clock())
                .unwrap(),
            Some(second)
        );
        assert_eq!(
            service
                .pending_delivery(endpoint(2), InventoryCursor(0), clock())
                .unwrap(),
            Some(first)
        );
    });
}

#[test]
fn local_receipt_replays_exact_committed_output_after_receive_and_emit_reopens() {
    let peers = Trio::enrolled();
    let (record, wire) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        let published = service
            .publish(
                operation,
                &publication(53, CHAT, &[endpoint(1)], b"lost reply"),
                clock(),
            )
            .unwrap();
        (
            published.record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
    peers.with_service(1, |service| {
        assert!(service.has_pending_receipt(record).unwrap());
        assert!(service.local_receipt(record).unwrap().is_none());
    });
    let receipt = peers.receipt(1, record);
    // Repeat the exact input after the response was lost. Neither replay nor
    // lookup creates another MLS operation or receipt ciphertext.
    peers.with_service(1, |service| {
        let head = service.coordinator().head().unwrap();
        assert_eq!(
            service
                .receive(next(1, service.coordinator()), &wire, clock())
                .unwrap(),
            Incoming::Record(record)
        );
        assert_eq!(
            service.local_receipt(record).unwrap().unwrap().as_slice(),
            receipt
        );
        assert_eq!(service.coordinator().head().unwrap(), head);
    });
    peers.with_service(0, |service| {
        assert!(service.local_receipt(record).unwrap().is_none());
    });
}

#[test]
fn three_reopened_endpoints_exchange_chat_and_telemetry_with_distinct_delivery_evidence() {
    let peers = Trio::enrolled();
    let chat_targets = [endpoint(1), endpoint(2)];
    let chat = publication(1, CHAT, &chat_targets, b"Dinner at seven?");
    // The publication result is deliberately discarded, as if its caller exited.
    let chat_operation = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        service.publish(operation, &chat, clock()).unwrap();
        operation
    });
    let (chat_id, chat_wire) = peers.with_service(0, |service| {
        let RecoveryOutcome::Committed(receipt) =
            service.coordinator().recover(chat_operation).unwrap()
        else {
            panic!("durable publication was lost")
        };
        let head = service.coordinator().head().unwrap();
        let retry = service.publish(chat_operation, &chat, clock()).unwrap();
        assert_eq!(retry.commit, receipt);
        assert_eq!(service.coordinator().head().unwrap(), head);
        let wire = service.outgoing(chat_operation).unwrap().unwrap().to_vec();
        let changed = publication(
            1,
            CHAT,
            &chat_targets,
            b"Different text must not reuse this operation",
        );
        assert!(matches!(
            service.publish(chat_operation, &changed, clock()),
            Err(Error::Fault(Fault::IdempotencyConflict))
        ));
        assert_eq!(
            service
                .outgoing(chat_operation)
                .unwrap()
                .unwrap()
                .as_slice(),
            wire
        );
        (retry.record, wire)
    });
    assert_eq!(
        peers.progress(0, chat_id),
        DeliverySummary {
            original_targets: 2,
            received: 0,
            removed: 0,
            pending: 2,
            expired: 0,
            time_uncertain: 0
        }
    );

    // A different peer publishes a non-message profile before receiving chat.
    let telemetry_targets = [endpoint(0), endpoint(2)];
    let telemetry_body = [0x10, 0x00, 0xff, 0x7f, 0x42];
    let telemetry = publication(2, TELEMETRY, &telemetry_targets, &telemetry_body);
    let (telemetry_id, telemetry_wire) = peers.with_service(1, |service| {
        let operation = next(1, service.coordinator());
        let published = service.publish(operation, &telemetry, clock()).unwrap();
        (
            published.record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    assert_eq!(peers.receive(1, &chat_wire), Incoming::Record(chat_id));
    assert_eq!(
        peers.receive(2, &telemetry_wire),
        Incoming::Record(telemetry_id)
    );
    assert_eq!(peers.receive(2, &chat_wire), Incoming::Record(chat_id));
    assert_eq!(
        peers.receive(0, &telemetry_wire),
        Incoming::Record(telemetry_id)
    );
    assert!(!peers.progress(0, chat_id).all_received());
    assert_eq!(peers.progress(0, chat_id).received, 0);
    assert_eq!(peers.progress(1, telemetry_id).received, 0);

    // Duplicate ciphertext is handled before entering the consumed MLS ratchet.
    let before_duplicate = peers.with_service(1, |service| {
        service.changes(ChangeCursor(0), 128).unwrap().head
    });
    assert_eq!(peers.receive(1, &chat_wire), Incoming::Record(chat_id));
    let after_duplicate = peers.with_service(1, |service| {
        service.changes(ChangeCursor(0), 128).unwrap().head
    });
    assert_eq!(before_duplicate, after_duplicate);

    // Reopening between receive and emission must retain the receipt obligation.
    let bob_chat_receipt = peers.receipt(1, chat_id);
    let carol_chat_receipt = peers.receipt(2, chat_id);
    assert_eq!(
        peers.receive(0, &bob_chat_receipt),
        Incoming::Receipt(DurableReceipt {
            group: GROUP,
            record: chat_id,
            target: endpoint(1)
        })
    );
    assert_eq!(peers.progress(0, chat_id).received, 1);
    assert!(!peers.progress(0, chat_id).all_received());
    assert_eq!(
        peers.receive(0, &carol_chat_receipt),
        Incoming::Receipt(DurableReceipt {
            group: GROUP,
            record: chat_id,
            target: endpoint(2)
        })
    );
    assert!(peers.progress(0, chat_id).all_received());
    peers.receive(2, &bob_chat_receipt);
    peers.receive(1, &carol_chat_receipt);
    assert!(peers.progress(1, chat_id).all_received());
    assert!(peers.progress(2, chat_id).all_received());

    let alice_telemetry_receipt = peers.receipt(0, telemetry_id);
    let carol_telemetry_receipt = peers.receipt(2, telemetry_id);
    peers.receive(1, &alice_telemetry_receipt);
    assert!(!peers.progress(1, telemetry_id).all_received());
    peers.receive(1, &carol_telemetry_receipt);
    assert!(peers.progress(1, telemetry_id).all_received());

    for peer in 0..3 {
        peers.with_service(peer, |service| {
            let record = service.record(chat_id).unwrap().unwrap();
            assert_eq!(record.metadata.namespace, CHAT);
            assert_eq!(record.metadata.author, endpoint(0));
            assert_eq!(record.targets, chat_targets);
            assert_eq!(record.body.as_slice(), chat.body);
            let telemetry_record = service.record(telemetry_id).unwrap().unwrap();
            assert_eq!(telemetry_record.metadata.namespace, TELEMETRY);
            assert_eq!(telemetry_record.targets, telemetry_targets);
            assert_eq!(telemetry_record.body.as_slice(), telemetry_body);
            // Durable cursors rebuild application state after dropped events/reload.
            let mut cursor = ChangeCursor(0);
            let mut records = Vec::new();
            loop {
                let page = service.changes(cursor, 1).unwrap();
                assert!(page.changes.len() <= 1);
                for change in page.changes {
                    assert!(change.cursor > cursor);
                    match change.kind {
                        ChangeKind::Published(id) => {
                            assert!(service.outgoing(change.operation).unwrap().is_some());
                            records.push(id);
                        }
                        ChangeKind::Received(id) => records.push(id),
                        ChangeKind::ReceiptQueued(_) => {
                            assert!(service.outgoing(change.operation).unwrap().is_some());
                        }
                        ChangeKind::Receipt(_) | ChangeKind::Rejected { .. } => {}
                    }
                }
                cursor = page.next;
                if cursor == page.head {
                    break;
                }
            }
            records.sort();
            let mut expected = vec![chat_id, telemetry_id];
            expected.sort();
            assert_eq!(records, expected);
        });
    }
}

// The provisional codec is intentionally constructed independently here: a valid
// MLS member can sign application claims that its recipients must still reject.
fn malicious_record(author: Endpoint, targets: &[Endpoint], body: &[u8]) -> Vec<u8> {
    let mut encoded = b"PGS\x01\x01".to_vec();
    encoded.extend_from_slice(&GROUP.0);
    append_endpoint(&mut encoded, author);
    encoded.extend_from_slice(&CHAT.0.to_be_bytes());
    encoded.extend_from_slice(&1u32.to_be_bytes());
    encoded.extend_from_slice(&control().head.0);
    encoded.extend_from_slice(&control().era.0 .0);
    encoded.extend_from_slice(&NOW.to_be_bytes());
    encoded.extend_from_slice(&(NOW + 600).to_be_bytes());
    encoded.extend_from_slice(&[99; 32]);
    encoded.extend_from_slice(&(targets.len() as u32).to_be_bytes());
    for target in targets {
        append_endpoint(&mut encoded, *target);
    }
    encoded.extend_from_slice(&(body.len() as u32).to_be_bytes());
    encoded.extend_from_slice(body);
    encoded
}
fn append_endpoint(bytes: &mut Vec<u8>, endpoint: Endpoint) {
    bytes.extend_from_slice(&endpoint.id.0);
    bytes.extend_from_slice(&endpoint.incarnation.get().to_be_bytes());
}

#[test]
fn mls_authentication_does_not_authorize_false_authors_or_unselected_targets() {
    let peers = Trio::enrolled();
    for (body, expected) in [
        (
            malicious_record(endpoint(2), &[endpoint(1)], b"forged author"),
            RejectReason::WrongAuthor,
        ),
        (
            malicious_record(endpoint(0), &[endpoint(2)], b"not for Bob"),
            RejectReason::NotTargeted,
        ),
    ] {
        let wire = peers.with_adapter(0, |adapter| {
            adapter
                .send(next(0, adapter.coordinator()), GROUP, &body)
                .unwrap()
                .output
                .messages[0]
                .clone()
        });
        assert_eq!(peers.receive(1, &wire), Incoming::Rejected(expected));
        let head = peers.with_service(1, |service| {
            service.changes(ChangeCursor(0), 128).unwrap().head
        });
        assert_eq!(peers.receive(1, &wire), Incoming::Rejected(expected));
        peers.with_service(1, |service| {
            let page = service.changes(ChangeCursor(0), 128).unwrap();
            assert_eq!(page.head, head);
            assert!(page
                .changes
                .iter()
                .all(|change| matches!(change.kind, ChangeKind::Rejected { .. })));
        });
    }
    let targets = [endpoint(1)];
    let valid = publication(3, CHAT, &targets, b"valid after rejected ratchet entries");
    let (record, wire) = peers.with_service(0, |service| {
        let op = next(0, service.coordinator());
        let published = service.publish(op, &valid, clock()).unwrap();
        (
            published.record,
            service.outgoing(op).unwrap().unwrap().to_vec(),
        )
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
}

#[test]
fn receipts_require_the_selected_endpoint_signer_and_tampering_does_not_advance_state() {
    let peers = Trio::enrolled();
    let targets = [endpoint(2)];
    let request = publication(4, CHAT, &targets, b"Carol only");
    let (record, wire) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        let published = service.publish(operation, &request, clock()).unwrap();
        (
            published.record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    let mut tampered = wire.clone();
    *tampered.last_mut().unwrap() ^= 1;
    peers.with_service(2, |service| {
        let head = service.coordinator().head().unwrap();
        let operation = next(2, service.coordinator());
        let oversized = vec![0; mls_limits().max_wire_bytes + 1];
        assert!(matches!(
            service.receive(operation, &oversized, clock()),
            Err(Error::Fault(Fault::Capacity))
        ));
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert!(matches!(
            service.receive(operation, &tampered, clock()),
            Err(Error::Security(_))
        ));
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert!(service.record(record).unwrap().is_none());
    });
    assert_eq!(peers.receive(2, &wire), Incoming::Record(record));

    // Alice cannot attest as Carol, and Bob cannot attest as an unselected target.
    for (signer, claimed_target) in [(0, endpoint(2)), (1, endpoint(1))] {
        let mut body = b"PGS\x02\x02".to_vec();
        body.extend_from_slice(&GROUP.0);
        body.extend_from_slice(&record.0);
        append_endpoint(&mut body, claimed_target);
        body.extend_from_slice(&request.validity.deadline().0.to_be_bytes());
        let forged = peers.with_adapter(signer, |adapter| {
            adapter
                .send(next(signer, adapter.coordinator()), GROUP, &body)
                .unwrap()
                .output
                .messages[0]
                .clone()
        });
        assert_eq!(
            peers.receive(2, &forged),
            Incoming::Rejected(RejectReason::InvalidReceipt)
        );
        assert_eq!(peers.progress(2, record).received, 0);
        assert!(!peers.progress(0, record).all_received());
    }
    let valid = peers.receipt(2, record);
    assert_eq!(
        peers.receive(0, &valid),
        Incoming::Receipt(DurableReceipt {
            group: GROUP,
            record,
            target: endpoint(2)
        })
    );
    assert!(peers.progress(0, record).all_received());
}

#[test]
fn receipt_before_record_remains_retryable_without_consuming_mls_or_feed_state() {
    let peers = Trio::enrolled();
    let targets = [endpoint(1), endpoint(2)];
    let request = publication(5, CHAT, &targets, b"Record arrives after its receipt");
    let (record, wire) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        let published = service.publish(operation, &request, clock()).unwrap();
        (
            published.record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
    let receipt_wire = peers.receipt(1, record);
    let (deferred_operation, previous_head, previous_feed) = peers.with_service(2, |service| {
        let operation = next(2, service.coordinator());
        let head = service.coordinator().head().unwrap();
        let feed = service.changes(ChangeCursor(0), 128).unwrap().head;
        assert!(matches!(
            service.receive(operation, &receipt_wire, clock()),
            Err(Error::Fault(Fault::MissingRecord))
        ));
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert_eq!(service.changes(ChangeCursor(0), 128).unwrap().head, feed);
        assert!(service.record(record).unwrap().is_none());
        assert_eq!(service.snapshot().unwrap(), None);
        assert!(!service
            .has_envelope(service.envelope_id(&receipt_wire))
            .unwrap());
        (operation, head, feed)
    });
    // Reopening must not turn this recoverable dependency into durable rejection.
    peers.with_service(2, |service| {
        assert_eq!(
            service.coordinator().recover(deferred_operation).unwrap(),
            RecoveryOutcome::NotCommitted {
                revision: previous_head
            }
        );
        assert_eq!(
            service.changes(ChangeCursor(0), 128).unwrap().head,
            previous_feed
        );
        assert_eq!(
            service
                .receive(next(2, service.coordinator()), &wire, clock())
                .unwrap(),
            Incoming::Record(record)
        );
    });
    assert_eq!(
        peers.receive(2, &receipt_wire),
        Incoming::Receipt(DurableReceipt {
            group: GROUP,
            record,
            target: endpoint(1)
        })
    );
    assert_eq!(peers.progress(2, record).received, 1);
    assert!(!peers.progress(2, record).all_received());
}

#[test]
fn unknown_clock_defers_receive_and_same_operation_and_ciphertext_succeed_after_clock_recovers() {
    let peers = Trio::enrolled();
    let targets = [endpoint(1)];
    let request = publication(6, CHAT, &targets, b"Wait for a trusted clock");
    let (record, wire) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        let published = service.publish(operation, &request, clock()).unwrap();
        (
            published.record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    let (operation, head, feed) = peers.with_service(1, |service| {
        let operation = next(1, service.coordinator());
        let head = service.coordinator().head().unwrap();
        let feed = service.changes(ChangeCursor(0), 128).unwrap().head;
        assert!(matches!(
            service.receive(operation, &wire, ClockReading::Unknown),
            Err(Error::Fault(Fault::TimeUncertain))
        ));
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert_eq!(service.changes(ChangeCursor(0), 128).unwrap().head, feed);
        assert!(service.record(record).unwrap().is_none());
        (operation, head, feed)
    });
    peers.with_service(1, |service| {
        assert_eq!(
            service.coordinator().recover(operation).unwrap(),
            RecoveryOutcome::NotCommitted { revision: head }
        );
        assert_eq!(service.changes(ChangeCursor(0), 128).unwrap().head, feed);
        assert_eq!(
            service.receive(operation, &wire, clock()).unwrap(),
            Incoming::Record(record)
        );
        assert_eq!(
            service.record(record).unwrap().unwrap().body.as_slice(),
            request.body
        );
        let received = service.changes(ChangeCursor(0), 128).unwrap();
        assert_eq!(received.changes.len(), 1);
        assert_eq!(received.changes[0].kind, ChangeKind::Received(record));
    });
}

#[test]
fn authenticated_body_over_receiver_mls_limit_is_a_durable_invalid_packet_rejection() {
    let low_receiver = prns_groups_mls::Limits {
        max_plaintext_bytes: 128,
        ..mls_limits()
    };
    let peers = Trio::enrolled_with_mls_limits([mls_limits(), low_receiver, mls_limits()]);
    let wire = peers.with_adapter(0, |adapter| {
        adapter
            .send(next(0, adapter.coordinator()), GROUP, &[0xaa; 256])
            .unwrap()
            .output
            .messages[0]
            .clone()
    });
    let operation = peers.with_service(1, |service| {
        let operation = next(1, service.coordinator());
        assert_eq!(
            service.receive(operation, &wire, clock()).unwrap(),
            Incoming::Rejected(RejectReason::InvalidPacket)
        );
        operation
    });
    peers.with_service(1, |service| {
        assert!(matches!(
            service.coordinator().recover(operation).unwrap(),
            RecoveryOutcome::Committed(_)
        ));
        let head = service.coordinator().head().unwrap();
        let feed = service.changes(ChangeCursor(0), 128).unwrap();
        assert_eq!(feed.changes.len(), 1);
        assert!(matches!(
            feed.changes[0].kind,
            ChangeKind::Rejected {
                reason: RejectReason::InvalidPacket,
                ..
            }
        ));
        assert_eq!(
            service
                .receive(next(1, service.coordinator()), &wire, clock())
                .unwrap(),
            Incoming::Rejected(RejectReason::InvalidPacket)
        );
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert_eq!(
            service.changes(ChangeCursor(0), 128).unwrap().head,
            feed.head
        );
    });
}

#[test]
fn retained_envelopes_survive_reopen_and_relay_without_the_author_or_ratchet_echoes() {
    let peers = Trio::enrolled();
    let targets = [endpoint(1), endpoint(2)];
    let request = publication(80, CHAT, &targets, b"Available while the author is offline");
    let (record, wire, id, snapshot) = peers.with_service(0, |service| {
        assert_eq!(service.snapshot().unwrap(), None);
        let operation = next(0, service.coordinator());
        let record = service
            .publish(operation, &request, clock())
            .unwrap()
            .record;
        let wire = service.outgoing(operation).unwrap().unwrap().to_vec();
        let id = service.envelope_id(&wire);
        let snapshot = service.snapshot().unwrap().unwrap();
        assert_eq!(snapshot.head, InventoryCursor(1));
        assert!(service.has_envelope(id).unwrap());
        let info = service.envelope_info(id).unwrap().unwrap();
        assert_eq!(info.position, InventoryCursor(1));
        assert_eq!(info.kind, InventoryKind::Record);
        assert_eq!(info.record, record);
        assert_eq!(info.bytes, wire.len());
        assert_eq!(service.envelope(id).unwrap().unwrap().as_slice(), wire);
        let head = service.coordinator().head().unwrap();
        let changes = service.changes(ChangeCursor(0), 128).unwrap().head;
        assert_eq!(
            service
                .receive(next(0, service.coordinator()), &wire, clock())
                .unwrap(),
            Incoming::Record(record)
        );
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert_eq!(service.changes(ChangeCursor(0), 128).unwrap().head, changes);
        assert_eq!(service.snapshot().unwrap(), Some(snapshot));
        (record, wire, id, snapshot)
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
    let receipt = peers.receipt(1, record);
    // The author is never reopened during catch-up: Bob serves both exact
    // ciphertexts from his durable ledger, and Carol verifies the MLS issuers.
    let (forwarded, forwarded_receipt, bob_snapshot) = peers.with_service(1, |service| {
        let bob_snapshot = service.snapshot().unwrap().unwrap();
        assert_ne!(bob_snapshot.generation, snapshot.generation);
        assert_eq!(bob_snapshot.head, InventoryCursor(2));
        let records = service
            .inventory(
                bob_snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(2),
                1,
            )
            .unwrap();
        assert_eq!(records.items.len(), 1);
        assert_eq!(records.items[0].position, InventoryCursor(1));
        assert_eq!(records.items[0].id, id);
        assert_eq!(records.items[0].record, record);
        assert_eq!(records.items[0].bytes, wire.len());
        assert_eq!(service.envelope_info(id).unwrap(), Some(records.items[0]));
        let receipt_page = service
            .inventory(
                bob_snapshot,
                records.next,
                InventoryKind::Receipt,
                endpoint(2),
                1,
            )
            .unwrap();
        assert_eq!(receipt_page.items.len(), 1);
        assert_eq!(receipt_page.items[0].position, InventoryCursor(2));
        assert_eq!(
            service.envelope_info(receipt_page.items[0].id).unwrap(),
            Some(receipt_page.items[0])
        );
        assert_eq!(receipt_page.next, bob_snapshot.head);
        let head = service.coordinator().head().unwrap();
        assert!(matches!(
            service
                .receive(next(1, service.coordinator()), &receipt, clock())
                .unwrap(),
            Incoming::Receipt(_)
        ));
        assert_eq!(service.coordinator().head().unwrap(), head);
        (
            service
                .envelope(records.items[0].id)
                .unwrap()
                .unwrap()
                .to_vec(),
            service
                .envelope(receipt_page.items[0].id)
                .unwrap()
                .unwrap()
                .to_vec(),
            bob_snapshot,
        )
    });
    assert_eq!(forwarded, wire);
    assert_eq!(forwarded_receipt, receipt);
    peers.with_service(1, |service| {
        assert_eq!(service.snapshot().unwrap(), Some(bob_snapshot));
    });
    assert_eq!(peers.receive(2, &forwarded), Incoming::Record(record));
    assert_eq!(peers.progress(2, record).received, 0);
    assert!(matches!(
        peers.receive(2, &forwarded_receipt),
        Incoming::Receipt(_)
    ));
    assert_eq!(peers.progress(2, record).received, 1);
    let carol_receipt = peers.receipt(2, record);
    assert!(peers.progress(2, record).all_received());
    assert!(matches!(
        peers.receive(1, &carol_receipt),
        Incoming::Receipt(_)
    ));
    assert!(peers.progress(1, record).all_received());
    peers.with_service(1, |service| {
        // New receipt appends do not change the frozen two-entry snapshot.
        assert_eq!(
            service.snapshot().unwrap().unwrap().head,
            InventoryCursor(3)
        );
        let receipts = service
            .inventory(
                bob_snapshot,
                InventoryCursor(0),
                InventoryKind::Receipt,
                endpoint(0),
                8,
            )
            .unwrap();
        assert_eq!(receipts.next, bob_snapshot.head);
        assert_eq!(receipts.items.len(), 1);
        assert_eq!(receipts.items[0].id, service.envelope_id(&receipt));
    });
}

#[test]
fn inventory_scans_are_bounded_filtered_and_bound_to_the_frozen_generation() {
    let peers = Trio::enrolled();
    let first_targets = [endpoint(1)];
    let second_targets = [endpoint(2)];
    let first = publication(81, TELEMETRY, &first_targets, b"Bob's archive");
    let second = publication(82, CHAT, &second_targets, b"Carol's conversation");
    let first_snapshot = peers.with_service(0, |service| {
        service
            .publish(next(0, service.coordinator()), &first, clock())
            .unwrap();
        service.snapshot().unwrap().unwrap()
    });
    peers.with_service(0, |service| {
        let second_id = service
            .publish(next(0, service.coordinator()), &second, clock())
            .unwrap()
            .record;
        let snapshot = service.snapshot().unwrap().unwrap();
        let page = service
            .inventory(
                snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(2),
                1,
            )
            .unwrap();
        assert!(page.items.is_empty());
        assert_eq!(page.next, InventoryCursor(1));
        let page = service
            .inventory(snapshot, page.next, InventoryKind::Record, endpoint(2), 1)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].record, second_id);
        assert_eq!(page.next, snapshot.head);
        let frozen = service
            .inventory(
                first_snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(2),
                8,
            )
            .unwrap();
        assert!(frozen.items.is_empty());
        assert_eq!(frozen.next, first_snapshot.head);
        // Authors can reconcile all original targets' receipts even when they
        // are not themselves a delivery target.
        assert_eq!(
            service
                .inventory(
                    snapshot,
                    InventoryCursor(0),
                    InventoryKind::Record,
                    endpoint(0),
                    8,
                )
                .unwrap()
                .items
                .len(),
            2
        );
        for invalid in [
            InventorySnapshot {
                generation: [0; 32],
                ..snapshot
            },
            InventorySnapshot {
                generation: first_snapshot.generation.map(|byte| byte ^ 1),
                ..snapshot
            },
        ] {
            assert!(matches!(
                service.inventory(
                    invalid,
                    InventoryCursor(0),
                    InventoryKind::Record,
                    endpoint(1),
                    1,
                ),
                Err(Error::Fault(Fault::RestartInventory))
            ));
        }
        assert!(matches!(
            service.inventory(
                InventorySnapshot {
                    head: InventoryCursor(snapshot.head.0 + 1),
                    ..snapshot
                },
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(1),
                1,
            ),
            Err(Error::Fault(Fault::RestartInventory))
        ));
        assert!(matches!(
            service.inventory(
                snapshot,
                InventoryCursor(snapshot.head.0 + 1),
                InventoryKind::Record,
                endpoint(1),
                1,
            ),
            Err(Error::Fault(Fault::InvalidEncoding))
        ));
        assert!(matches!(
            service.inventory(
                snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(9),
                1,
            ),
            Err(Error::Fault(Fault::InvalidTargets))
        ));
        assert!(matches!(
            service.inventory(
                snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(1),
                0,
            ),
            Err(Error::Fault(Fault::Capacity))
        ));
    });
}

#[test]
fn partial_inventory_restarts_after_same_generation_head_regression() {
    let peers = Trio::enrolled();
    let targets = [endpoint(1)];
    let first = publication(181, CHAT, &targets, b"retained prefix");
    let second = publication(182, CHAT, &targets, b"later prefix");
    let previous = peers.with_service(0, |service| {
        service
            .publish(next(0, service.coordinator()), &first, clock())
            .unwrap();
        service.snapshot().unwrap().unwrap()
    });
    let path = peers.directory.path().join("peer-0.redb");
    let backup = peers.directory.path().join("older-inventory.redb");
    std::fs::copy(&path, &backup).unwrap();
    let (frozen, after) = peers.with_service(0, |service| {
        service
            .publish(next(0, service.coordinator()), &second, clock())
            .unwrap();
        let snapshot = service.snapshot().unwrap().unwrap();
        assert_eq!(snapshot.generation, previous.generation);
        let page = service
            .inventory(
                snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(1),
                1,
            )
            .unwrap();
        assert!(page.next < snapshot.head);
        (snapshot, page.next)
    });
    // Simulate detection of an older remote ledger while a peer holds a partial
    // scan. This does not qualify MLS state rollback for sending new messages.
    std::fs::copy(&backup, &path).unwrap();
    peers.with_service(0, |service| {
        assert_eq!(service.snapshot().unwrap(), Some(previous));
        assert!(matches!(
            service.inventory(frozen, after, InventoryKind::Record, endpoint(1), 1),
            Err(Error::Fault(Fault::RestartInventory))
        ));
        // Restart discovery can inspect the surviving prefix; an invalid cursor
        // beyond its own claimed snapshot is still a malformed request.
        let page = service
            .inventory(
                previous,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(1),
                1,
            )
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next, previous.head);
        assert!(matches!(
            service.inventory(
                previous,
                InventoryCursor(previous.head.0 + 1),
                InventoryKind::Record,
                endpoint(1),
                1
            ),
            Err(Error::Fault(Fault::InvalidEncoding))
        ));
    });
}

#[test]
fn envelope_quota_failure_aborts_publication_and_receive_without_losing_retryability() {
    let mut peers = Trio::enrolled();
    peers.service_limits[0].max_envelopes = 3;
    peers.service_limits[1].max_retained_wire_bytes = 1;
    let targets = [endpoint(1), endpoint(2)];
    let first = publication(
        83,
        CHAT,
        &targets,
        b"Fits one record plus its two future receipts",
    );
    let second = publication(84, CHAT, &targets, b"Must not evict the first");
    let wire = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        service.publish(operation, &first, clock()).unwrap();
        service.outgoing(operation).unwrap().unwrap().to_vec()
    });
    peers.with_service(0, |service| {
        let head = service.coordinator().head().unwrap();
        let snapshot = service.snapshot().unwrap();
        let changes = service.changes(ChangeCursor(0), 128).unwrap().head;
        for _ in 0..2 {
            assert!(matches!(
                service.publish(next(0, service.coordinator()), &second, clock()),
                Err(Error::Fault(Fault::Capacity))
            ));
            assert_eq!(service.coordinator().head().unwrap(), head);
            assert_eq!(service.snapshot().unwrap(), snapshot);
            assert_eq!(service.changes(ChangeCursor(0), 128).unwrap().head, changes);
        }
    });
    peers.with_service(1, |service| {
        let operation = next(1, service.coordinator());
        let head = service.coordinator().head().unwrap();
        for _ in 0..2 {
            assert!(matches!(
                service.receive(operation, &wire, clock()),
                Err(Error::Fault(Fault::Capacity))
            ));
            assert_eq!(service.coordinator().head().unwrap(), head);
            assert_eq!(service.snapshot().unwrap(), None);
            assert!(!service.has_envelope(service.envelope_id(&wire)).unwrap());
        }
    });
    // No application transaction committed at Bob, so a larger configuration is
    // still permitted and the same cipher/ratchet material remains consumable.
    peers.service_limits[1] = service_limits();
    assert!(matches!(peers.receive(1, &wire), Incoming::Record(_)));
}

#[test]
fn record_admission_reserves_each_targets_receipt_across_reopen_and_full_quotas() {
    for quota in ["envelopes", "changes", "wire bytes"] {
        let mut peers = Trio::enrolled();
        match quota {
            "envelopes" => peers.service_limits[1].max_envelopes = 3,
            "changes" => peers.service_limits[1].max_changes = 3,
            "wire bytes" => peers.service_limits[1].max_retained_wire_bytes = 3 * 1024,
            _ => unreachable!(),
        }
        let targets = [endpoint(1), endpoint(2)];
        let publish = |nonce| {
            peers.with_service(0, |service| {
                let operation = next(0, service.coordinator());
                let published = service
                    .publish(
                        operation,
                        &publication(nonce, CHAT, &targets, b"bounded"),
                        clock(),
                    )
                    .unwrap();
                (
                    published.record,
                    service.outgoing(operation).unwrap().unwrap().to_vec(),
                )
            })
        };
        let (record, first) = publish(91);
        let (later, second) = publish(92);
        assert_eq!(peers.receive(1, &first), Incoming::Record(record));
        assert_eq!(peers.receive(2, &first), Incoming::Record(record));
        // Bob's first acceptance is reopened with a pending local receipt. A
        // further record must not consume that receipt's or Carol's reserved room.
        peers.with_service(1, |service| {
            assert!(service.has_pending_receipt(record).unwrap());
            let operation = next(1, service.coordinator());
            let before = service.coordinator().head().unwrap();
            assert!(
                matches!(
                    service.receive(operation, &second, clock()),
                    Err(Error::Fault(Fault::Capacity))
                ),
                "{quota}"
            );
            assert_eq!(service.coordinator().head().unwrap(), before);
            assert!(service.record(later).unwrap().is_none());
        });
        let bob_receipt = peers.receipt(1, record);
        let carol_receipt = peers.receipt(2, record);
        assert!(bob_receipt.len() <= prns_groups_service::MAX_RECEIPT_WIRE_BYTES);
        assert!(carol_receipt.len() <= prns_groups_service::MAX_RECEIPT_WIRE_BYTES);
        assert!(matches!(
            peers.receive(1, &carol_receipt),
            Incoming::Receipt(_)
        ));
        assert!(peers.progress(1, record).all_received(), "{quota}");
        peers.with_service(1, |service| {
            assert!(!service.has_pending_receipt(record).unwrap());
            assert_eq!(
                service.snapshot().unwrap().unwrap().head,
                InventoryCursor(3)
            );
            assert_eq!(
                service.changes(ChangeCursor(0), 3).unwrap().head,
                ChangeCursor(3)
            );
            let before = service.coordinator().head().unwrap();
            assert!(matches!(
                service
                    .receive(next(1, service.coordinator()), &carol_receipt, clock())
                    .unwrap(),
                Incoming::Receipt(_)
            ));
            assert_eq!(service.coordinator().head().unwrap(), before);
        });
    }
}

#[test]
fn authenticated_expiry_rejection_makes_dependent_receipt_terminal_after_reopen() {
    let peers = Trio::enrolled();
    let targets = [endpoint(1), endpoint(2)];
    let (record, wire) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        let published = service
            .publish(
                operation,
                &publication(93, CHAT, &targets, b"expires offline"),
                clock(),
            )
            .unwrap();
        (
            published.record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
    let receipt = peers.receipt(1, record);
    let late = ClockReading::Trusted(
        ClockInterval::new(Timestamp(NOW + 601), Timestamp(NOW + 601)).unwrap(),
    );
    peers.with_service(2, |service| {
        let before = service.coordinator().head().unwrap();
        // Before its signed deadline, the receipt still needs its record.
        assert!(matches!(
            service.receive(next(2, service.coordinator()), &receipt, clock()),
            Err(Error::Fault(Fault::MissingRecord))
        ));
        assert_eq!(service.coordinator().head().unwrap(), before);
        assert_eq!(
            service
                .receive(next(2, service.coordinator()), &wire, late)
                .unwrap(),
            Incoming::Rejected(RejectReason::Expired)
        );
    });
    peers.with_service(2, |service| {
        assert_eq!(
            service
                .receive(next(2, service.coordinator()), &receipt, late)
                .unwrap(),
            Incoming::Rejected(RejectReason::UnknownRecord)
        );
        assert!(service.record(record).unwrap().is_none());
        assert!(!service.has_pending_receipt(record).unwrap());
        assert_eq!(
            service.snapshot().unwrap().unwrap().head,
            InventoryCursor(0)
        );
        assert_eq!(
            service.changes(ChangeCursor(0), 128).unwrap().head,
            ChangeCursor(2)
        );
    });
    peers.with_service(2, |service| {
        let before = service.coordinator().head().unwrap();
        assert_eq!(
            service
                .receive(next(2, service.coordinator()), &receipt, late)
                .unwrap(),
            Incoming::Rejected(RejectReason::UnknownRecord)
        );
        assert_eq!(service.coordinator().head().unwrap(), before);
    });
    assert_eq!(peers.progress(0, record).received, 0);
}

#[test]
fn missing_reservations_reconstruct_atomically_from_exact_target_progress() {
    let mut peers = Trio::enrolled();
    peers.service_limits[1].max_envelopes = 3;
    let targets = [endpoint(1), endpoint(2)];
    let publish = |nonce| {
        peers.with_service(0, |service| {
            let operation = next(0, service.coordinator());
            let published = service
                .publish(
                    operation,
                    &publication(nonce, CHAT, &targets, b"reservation recovery"),
                    clock(),
                )
                .unwrap();
            (
                published.record,
                service.outgoing(operation).unwrap().unwrap().to_vec(),
            )
        })
    };
    let (record, first) = publish(94);
    let (_, second) = publish(95);
    assert_eq!(peers.receive(1, &first), Incoming::Record(record));
    assert_eq!(peers.receive(2, &first), Incoming::Record(record));
    peers.receipt(1, record);
    let carol = peers.receipt(2, record);
    let mut reserve_key = vec![b'R'];
    reserve_key.extend_from_slice(&GROUP.0);
    peers.with_adapter(1, |adapter| {
        // Model a missing counter while admitted receipt obligations remain.
        adapter
            .compose(
                next(1, adapter.coordinator()),
                prns_groups_mls::Command::Send {
                    group: GROUP,
                    body: b"missing-reservation fixture",
                },
                |_, _, _| {
                    Ok::<_, Error<prns_groups_store_redb::RedbError>>(
                        prns_groups_mls::Admission::Accept,
                    )
                },
                |tx, _| {
                    tx.delete(&reserve_key)?;
                    Ok(())
                },
            )
            .unwrap();
    });
    peers.with_service(1, |service| {
        assert!(matches!(
            service.receive(next(1, service.coordinator()), &second, clock()),
            Err(Error::Fault(Fault::Capacity))
        ));
    });
    peers.with_adapter(1, |adapter| {
        assert!(
            adapter.application_read(&reserve_key).unwrap().is_none(),
            "failed admission must roll back the reservation migration too"
        );
    });
    assert!(matches!(peers.receive(1, &carol), Incoming::Receipt(_)));
    assert!(peers.progress(1, record).all_received());
    peers.with_adapter(1, |adapter| {
        assert_eq!(
            adapter
                .application_read(&reserve_key)
                .unwrap()
                .unwrap()
                .as_slice(),
            &[1, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    });
}

#[test]
fn retained_own_wire_cannot_bypass_retirement_of_its_original_mls_output() {
    let peers = Trio::enrolled();
    let targets = [endpoint(1)];
    let request = publication(
        85,
        CHAT,
        &targets,
        b"Dispatch obligations prohibit premature retirement",
    );
    let (operation, id) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        service.publish(operation, &request, clock()).unwrap();
        let wire = service.outgoing(operation).unwrap().unwrap();
        (operation, service.envelope_id(&wire))
    });
    peers.with_adapter(0, |adapter| {
        adapter
            .acknowledge(next(0, adapter.coordinator()), operation)
            .unwrap();
    });
    peers.with_service(0, |service| {
        assert!(service.has_envelope(id).unwrap());
        assert!(
            service.has_pending_authored_deliveries(clock()).unwrap(),
            "pruning recovery output is never delivery evidence"
        );
        assert!(matches!(
            service.envelope(id),
            Err(Error::Fault(Fault::EvidencePruned))
        ));
    });
}

#[test]
fn terminal_groups_keep_history_without_releasing_or_accepting_traffic() {
    for local in [0, 1] {
        let peers = Trio::enrolled();
        let (record, operation, wire) = peers.with_service(0, |service| {
            let operation = next(0, service.coordinator());
            let record = service
                .publish(
                    operation,
                    &publication(110, CHAT, &[endpoint(1), endpoint(2)], b"terminal history"),
                    clock(),
                )
                .unwrap()
                .record;
            (
                record,
                operation,
                service.outgoing(operation).unwrap().unwrap().to_vec(),
            )
        });
        assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
        if local == 0 {
            peers.with_adapter(0, |adapter| {
                adapter
                    .compose(
                        next(0, adapter.coordinator()),
                        Command::Retire { group: GROUP },
                        |_, _, _| Ok::<_, ()>(Admission::Accept),
                        |tx, _| tx.put(b"terminal-control", b"closed").map_err(|_| ()),
                    )
                    .unwrap();
            });
        } else {
            let removal = peers.with_adapter(0, |adapter| {
                adapter
                    .remove(next(0, adapter.coordinator()), GROUP, 1)
                    .unwrap()
            });
            peers.with_adapter(1, |adapter| {
                adapter
                    .receive(
                        next(1, adapter.coordinator()),
                        GROUP,
                        &removal.output.messages[0],
                        |_| Admission::Accept,
                    )
                    .unwrap();
            });
        }
        peers.with_service(local, |service| {
            let before = service.coordinator().head().unwrap();
            assert_eq!(
                service.record(record).unwrap().unwrap().body.as_slice(),
                b"terminal history"
            );
            assert!(!service
                .changes(ChangeCursor(0), 32)
                .unwrap()
                .changes
                .is_empty());
            assert_eq!(service.progress(record, clock()).unwrap().received, 0);
            assert!(!service.in_current_era(record).unwrap());
            assert!(!service.has_pending_receipt(record).unwrap());
            assert!(!service.has_pending_authored_deliveries(clock()).unwrap());
            let dependencies = service.dependencies().unwrap();
            assert!(dependencies.retired);
            assert_eq!(dependencies.pending_authored_records, 0);
            assert_eq!(dependencies.pending_authored_targets, 0);
            assert_eq!(dependencies.pending_receipts, 0);
            assert_eq!(dependencies.retained_records, 1);
            assert!(dependencies.retained_result_changes > 0);
            assert!(service
                .envelope(service.envelope_id(&wire))
                .unwrap()
                .is_none());
            assert!(service.local_receipt(record).unwrap().is_none());
            assert!(service
                .pending_delivery(endpoint(2), InventoryCursor(0), clock())
                .unwrap()
                .is_none());
            assert!(service.outgoing(operation).is_err());
            let next = next(local, service.coordinator());
            assert!(service
                .publish(
                    next,
                    &publication(111, CHAT, &[endpoint(2)], b"blocked"),
                    clock()
                )
                .is_err());
            assert!(service.receive(next, &wire, clock()).is_err());
            assert!(service.emit_receipt(next, record).is_err());
            let snapshot = service.snapshot().unwrap().unwrap();
            assert!(service
                .inventory(
                    snapshot,
                    InventoryCursor(0),
                    InventoryKind::Record,
                    endpoint(2),
                    32
                )
                .is_err());
            assert_eq!(service.coordinator().head().unwrap(), before);
        });
    }
}

#[test]
fn managed_membership_barrier_preserves_history_but_never_offers_old_era_bytes() {
    let trio = Trio::enrolled();
    let mut changed = control();
    changed.head = ControlId([90; 32]);
    changed.era = AuthorizationEra(ControlId([91; 32]));
    let db = Database::open(trio.directory.path().join("peer-0.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let mut record = None;
    let mut envelope = None;
    for current in [control(), changed] {
        let backend = owner.open(STORAGE, incarnation(0), store_limits()).unwrap();
        let adapter = Adapter::new(
            Coordinator::new(backend, coordinator_limits()).unwrap(),
            trio.identity(0),
            trio.policy.clone(),
            trio.mls_limits[0],
        )
        .unwrap();
        let mut service = GroupService::new_managed(
            adapter,
            Descriptor {
                group: GROUP,
                control: current,
                readers: &trio.readers,
            },
            service_limits(),
        )
        .unwrap();
        if current == control() {
            let published = service
                .publish(
                    next(0, service.coordinator()),
                    &publication(99, CHAT, &[endpoint(1), endpoint(2)], b"retained locally"),
                    clock(),
                )
                .unwrap();
            assert!(service.has_pending_authored_deliveries(clock()).unwrap());
            record = Some(published.record);
            let snapshot = service.snapshot().unwrap().unwrap();
            let page = service
                .inventory(
                    snapshot,
                    InventoryCursor(0),
                    InventoryKind::Record,
                    endpoint(1),
                    32,
                )
                .unwrap();
            assert_eq!(page.items.len(), 1);
            envelope = Some(page.items[0].id);
        } else {
            let record = record.unwrap();
            assert_eq!(
                service.record(record).unwrap().unwrap().body.as_slice(),
                b"retained locally"
            );
            assert!(!service.in_current_era(record).unwrap());
            assert!(!service.has_pending_authored_deliveries(clock()).unwrap());
            let dependencies = service.dependencies().unwrap();
            assert!(!dependencies.retired);
            assert_eq!(dependencies.pending_authored_targets, 0);
            assert_eq!(dependencies.pending_receipts, 0);
            assert_eq!(dependencies.retained_records, 1);
            assert!(dependencies.retained_result_changes > 0);
            assert_eq!(service.progress(record, clock()).unwrap().received, 0);
            assert!(service.envelope(envelope.unwrap()).unwrap().is_none());
            assert!(service
                .pending_delivery(endpoint(1), InventoryCursor(0), clock())
                .unwrap()
                .is_none());
            let snapshot = service.snapshot().unwrap().unwrap();
            let page = service
                .inventory(
                    snapshot,
                    InventoryCursor(0),
                    InventoryKind::Record,
                    endpoint(1),
                    32,
                )
                .unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.next, snapshot.head);
            // A subsequent publication reconstructs only current-era receipt
            // reservations. It must not report old targets as delivered.
            service
                .publish(
                    next(0, service.coordinator()),
                    &publication(100, CHAT, &[endpoint(1)], b"current era"),
                    clock(),
                )
                .unwrap();
            assert_eq!(service.progress(record, clock()).unwrap().received, 0);
        }
    }
}

#[test]
fn membership_readiness_uses_durable_authored_receipts_and_definite_expiry() {
    let peers = Trio::enrolled();
    let (record, wire) = peers.with_service(0, |service| {
        assert!(!service
            .has_pending_authored_deliveries(ClockReading::Unknown)
            .unwrap());
        let operation = next(0, service.coordinator());
        let record = service
            .publish(
                operation,
                &publication(
                    101,
                    CHAT,
                    &[endpoint(1), endpoint(2)],
                    b"finish before adding",
                ),
                clock(),
            )
            .unwrap()
            .record;
        assert!(service.has_pending_authored_deliveries(clock()).unwrap());
        assert!(service
            .has_pending_authored_deliveries(ClockReading::Unknown)
            .unwrap());
        (
            record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    for peer in [1, 2] {
        assert_eq!(peers.receive(peer, &wire), Incoming::Record(record));
        peers.with_service(peer, |service| {
            assert!(
                !service.has_pending_authored_deliveries(clock()).unwrap(),
                "receiving someone else's record is not an authored delivery"
            );
        });
        peers.receive(0, &peers.receipt(peer, record));
        peers.with_service(0, |service| {
            assert_eq!(
                service.has_pending_authored_deliveries(clock()).unwrap(),
                peer == 1
            );
        });
    }
    peers.with_service(0, |service| {
        service
            .publish(
                next(0, service.coordinator()),
                &publication(102, CHAT, &[endpoint(1)], b"expires without a receipt"),
                clock(),
            )
            .unwrap();
        assert!(service
            .has_pending_authored_deliveries(at(NOW + 599))
            .unwrap());
        let uncertain = ClockReading::Trusted(
            ClockInterval::new(Timestamp(NOW + 599), Timestamp(NOW + 601)).unwrap(),
        );
        assert!(service.has_pending_authored_deliveries(uncertain).unwrap());
        assert!(!service
            .has_pending_authored_deliveries(at(NOW + 600))
            .unwrap());
    });
}

fn at(seconds: u64) -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(seconds), Timestamp(seconds)).unwrap())
}

#[test]
fn expiry_retirement_preserves_live_records_and_deadline_receipts_do_not_need_deleted_dependencies()
{
    let peers = Trio::enrolled();
    let targets = [endpoint(1), endpoint(2)];
    let (old, old_wire, live, live_wire) = peers.with_service(0, |service| {
        let op = next(0, service.coordinator());
        let old = service
            .publish(op, &publication(151, CHAT, &targets, b"old"), clock())
            .unwrap()
            .record;
        let old_wire = service.outgoing(op).unwrap().unwrap().to_vec();
        let mut request = publication(152, CHAT, &targets, b"live");
        request.validity =
            PublicationValidity::new(Timestamp(NOW), Timestamp(NOW + 900), Seconds(3600)).unwrap();
        let op = next(0, service.coordinator());
        let live = service.publish(op, &request, clock()).unwrap().record;
        (
            old,
            old_wire,
            live,
            service.outgoing(op).unwrap().unwrap().to_vec(),
        )
    });
    for peer in [1, 2] {
        assert_eq!(peers.receive(peer, &old_wire), Incoming::Record(old));
        assert_eq!(peers.receive(peer, &live_wire), Incoming::Record(live));
    }
    let delayed_receipt = peers.receipt(2, old);
    peers.with_service(1, |service| {
        assert!(matches!(
            service.maintenance_plan(ClockReading::Unknown, 1, 8),
            Err(Error::Fault(Fault::TimeUncertain))
        ));
        let overlap = ClockReading::Trusted(
            ClockInterval::new(Timestamp(NOW + 599), Timestamp(NOW + 601)).unwrap(),
        );
        assert!(service.maintenance_plan(overlap, 1, 8).unwrap().is_empty());
        let before = service.snapshot().unwrap().unwrap();
        let plan = service.maintenance_plan(at(NOW + 601), 1, 8).unwrap();
        assert_eq!(plan.records(), &[old]);
        let operation = next(1, service.coordinator());
        let result = service.maintain(operation, &plan).unwrap();
        assert_eq!(result.records_removed, 1);
        assert_eq!(service.maintain(operation, &plan).unwrap(), result);
        assert!(service.record(old).unwrap().is_none());
        assert!(!service.has_pending_receipt(old).unwrap());
        assert!(service.record(live).unwrap().is_some());
        assert!(service.has_pending_receipt(live).unwrap());
        assert_eq!(service.progress(live, clock()).unwrap().received, 0);
        assert!(matches!(
            service.inventory(
                before,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(0),
                4
            ),
            Err(Error::Fault(Fault::RestartInventory))
        ));
        let page = service.changes(ChangeCursor(0), 128).unwrap();
        assert_eq!(page.changes.len(), 1);
        assert!(matches!(page.changes[0].kind, ChangeKind::Received(id) if id == live));
        assert_eq!(service.expiry_floor().unwrap(), Timestamp(NOW + 601));
    });
    peers.with_service(1, |service| {
        // The authenticated v2 deadline allows terminal rejection even after
        // maintenance deleted the body AND its modern dependency tombstone.
        assert_eq!(
            service
                .receive(next(1, service.coordinator()), &delayed_receipt, clock())
                .unwrap(),
            Incoming::Rejected(RejectReason::UnknownRecord)
        );
        assert_eq!(service.progress(live, clock()).unwrap().received, 0);
        assert!(service.record(old).unwrap().is_none());
        let snapshot = service.snapshot().unwrap().unwrap();
        let page = service
            .inventory(
                snapshot,
                InventoryCursor(0),
                InventoryKind::Record,
                endpoint(0),
                1,
            )
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].record, live);
        assert!(page.next.0 > 1); // a legitimate gap left by retirement
    });
}

#[test]
fn receipt_deadline_must_match_live_record_and_mls_capacity_reserves_pending_receipts() {
    let mut caps = mls_limits();
    caps.max_pending_outputs = 7;
    let peers = Trio::enrolled_with_mls_limits([caps; 3]);
    let targets = [endpoint(1), endpoint(2)];
    let (record, wire) = peers.with_service(0, |service| {
        let op = next(0, service.coordinator());
        let published = service
            .publish(
                op,
                &publication(153, CHAT, &targets, b"owed receipts"),
                clock(),
            )
            .unwrap();
        assert!(matches!(
            service.publish(
                next(0, service.coordinator()),
                &publication(154, CHAT, &targets, b"cannot consume reserves"),
                clock()
            ),
            Err(Error::Fault(Fault::Capacity))
        ));
        (
            published.record,
            service.outgoing(op).unwrap().unwrap().to_vec(),
        )
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
    assert_eq!(peers.receive(2, &wire), Incoming::Record(record));
    let bob = peers.receipt(1, record);
    let carol = peers.receipt(2, record);
    assert!(matches!(peers.receive(0, &bob), Incoming::Receipt(_)));
    assert!(matches!(peers.receive(0, &carol), Incoming::Receipt(_)));
    assert_eq!(peers.progress(0, record).received, 2);

    // A fresh receiver with room distinguishes authentication from a true
    // deadline claim; the member's signature alone cannot change the record.
    let peers = Trio::enrolled();
    let (record, wire) = peers.with_service(0, |service| {
        let op = next(0, service.coordinator());
        let id = service
            .publish(
                op,
                &publication(155, CHAT, &targets, b"deadline binding"),
                clock(),
            )
            .unwrap()
            .record;
        (id, service.outgoing(op).unwrap().unwrap().to_vec())
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
    let mut body = b"PGS\x02\x02".to_vec();
    body.extend_from_slice(&GROUP.0);
    body.extend_from_slice(&record.0);
    append_endpoint(&mut body, endpoint(2));
    body.extend_from_slice(&(NOW + 1).to_be_bytes());
    let forged = peers.with_adapter(2, |adapter| {
        adapter
            .send(next(2, adapter.coordinator()), GROUP, &body)
            .unwrap()
            .output
            .messages[0]
            .clone()
    });
    assert_eq!(
        peers.receive(1, &forged),
        Incoming::Rejected(RejectReason::InvalidReceipt)
    );
    assert_eq!(peers.progress(1, record).received, 0);
}

#[test]
fn unsupported_expiry_tombstone_fails_without_consuming_the_record() {
    use sha2::{Digest, Sha256};

    let peers = Trio::enrolled();
    let targets = [endpoint(1)];
    let (record, wire) = peers.with_service(0, |service| {
        let operation = next(0, service.coordinator());
        let record = service
            .publish(
                operation,
                &publication(157, CHAT, &targets, b"unsupported tombstone"),
                clock(),
            )
            .unwrap()
            .record;
        (
            record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    let mut hash = Sha256::new();
    hash.update(GROUP.0);
    hash.update(record.0);
    let mut key = vec![b'e'];
    key.extend_from_slice(&hash.finalize());
    peers.with_adapter(1, |adapter| {
        adapter
            .maintain(next(1, adapter.coordinator()), &[], |tx| tx.put(&key, &[1]))
            .unwrap();
    });
    peers.with_service(1, |service| {
        let head = service.coordinator().head().unwrap();
        assert!(matches!(
            service.receive(next(1, service.coordinator()), &wire, clock()),
            Err(Error::Fault(Fault::Corrupt))
        ));
        assert_eq!(service.coordinator().head().unwrap(), head);
        assert!(service.record(record).unwrap().is_none());
    });
    peers.with_adapter(1, |adapter| {
        assert_eq!(
            adapter.application_read(&key).unwrap().unwrap().as_slice(),
            &[1]
        );
    });
}

#[test]
fn unsupported_receipt_operation_conflicts_without_rewriting_retained_ciphertext() {
    use sha2::{Digest, Sha256};
    let peers = Trio::enrolled();
    let targets = [endpoint(1), endpoint(2)];
    let (record, wire) = peers.with_service(0, |service| {
        let op = next(0, service.coordinator());
        let id = service
            .publish(
                op,
                &publication(156, CHAT, &targets, b"unsupported receipt format"),
                clock(),
            )
            .unwrap()
            .record;
        (id, service.outgoing(op).unwrap().unwrap().to_vec())
    });
    assert_eq!(peers.receive(1, &wire), Incoming::Record(record));
    let mut body = b"PGS\x01\x02".to_vec();
    body.extend_from_slice(&GROUP.0);
    body.extend_from_slice(&record.0);
    append_endpoint(&mut body, endpoint(1));
    let mut hash = Sha256::new();
    hash.update(b"prns.groups.service.operation.v1\0");
    hash.update(b"receipt");
    hash.update(GROUP.0);
    hash.update(&body);
    let fingerprint: [u8; 32] = hash.finalize().into();
    // Inject an unsupported receipt's retained output and operation fact. A
    // different encoding must not reuse that operation ID or replace its bytes.
    let (operation, original) = peers.with_adapter(1, |adapter| {
        let operation = next(1, adapter.coordinator());
        let output = adapter
            .compose(
                operation,
                prns_groups_mls::Command::Send {
                    group: GROUP,
                    body: &body,
                },
                |_, _, _| {
                    Ok::<_, prns_groups_store::StoreError<_>>(prns_groups_mls::Admission::Accept)
                },
                |tx, _| {
                    let mut key = vec![b'o'];
                    key.extend_from_slice(&operation.0);
                    let mut fact = b"PGF\x01".to_vec();
                    fact.extend_from_slice(&fingerprint);
                    fact.push(2);
                    fact.extend_from_slice(&record.0);
                    tx.put(&key, &fact)?;
                    Ok::<_, prns_groups_store::StoreError<_>>(())
                },
            )
            .unwrap();
        (operation, output.output.messages[0].clone())
    });
    peers.with_service(1, |service| {
        let before = service.coordinator().head().unwrap();
        assert!(matches!(
            service.emit_receipt(operation, record),
            Err(Error::Fault(Fault::IdempotencyConflict))
        ));
        assert_eq!(service.coordinator().head().unwrap(), before);
        assert_eq!(
            service.outgoing(operation).unwrap().unwrap().as_slice(),
            original
        );
    });
}

#[path = "native_flow/dependencies.rs"]
mod dependency_tests;
