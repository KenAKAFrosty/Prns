use super::*;

#[test]
fn durable_dependencies_distinguish_deliveries_receipt_intents_and_passive_results() {
    let peers = Trio::enrolled();
    let (record, wire) = peers.with_service(0, |service| {
        // MLS enrollment has retained outputs; those are not active deliveries.
        assert!(!service.pending_operations().unwrap().is_empty());
        assert_eq!(
            service.dependencies().unwrap(),
            prns_groups_service::DurableDependencies::default()
        );
        let operation = next(0, service.coordinator());
        let record = service
            .publish(
                operation,
                &publication(121, TELEMETRY, &[endpoint(1), endpoint(2)], b"sample"),
                clock(),
            )
            .unwrap()
            .record;
        let before = service.coordinator().head().unwrap();
        let first = service.dependencies().unwrap();
        assert_eq!(first, service.dependencies().unwrap());
        assert_eq!(service.coordinator().head().unwrap(), before);
        assert_eq!(first.pending_authored_records, 1);
        assert_eq!(first.pending_authored_targets, 2);
        assert_eq!(first.pending_receipts, 0);
        assert_eq!(first.retained_records, 1);
        assert_eq!(first.retained_result_changes, 1);
        (
            record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    });
    for peer in [1, 2] {
        assert_eq!(peers.receive(peer, &wire), Incoming::Record(record));
        peers.with_service(peer, |service| {
            let before = service.coordinator().head().unwrap();
            let dependencies = service.dependencies().unwrap();
            assert_eq!(dependencies.pending_authored_records, 0);
            assert_eq!(dependencies.pending_authored_targets, 0);
            assert_eq!(dependencies.pending_receipts, 1);
            assert_eq!(dependencies.retained_records, 1);
            assert_eq!(service.coordinator().head().unwrap(), before);
        });
        let receipt = peers.receipt(peer, record);
        peers.with_service(peer, |service| {
            let dependencies = service.dependencies().unwrap();
            assert_eq!(dependencies.pending_receipts, 0);
            assert_eq!(dependencies.retained_records, 1);
            assert_eq!(dependencies.retained_result_changes, 2);
        });
        peers.receive(0, &receipt);
        peers.with_service(0, |service| {
            let dependencies = service.dependencies().unwrap();
            assert_eq!(dependencies.pending_authored_records, u64::from(peer == 1));
            assert_eq!(dependencies.pending_authored_targets, u64::from(peer == 1));
            assert_eq!(dependencies.retained_records, 1);
            assert!(dependencies.retained_result_changes > 1);
        });
    }
}

#[test]
fn dependency_inspection_does_not_turn_a_clock_observation_into_settlement() {
    let peers = Trio::enrolled();
    peers.with_service(0, |service| {
        service
            .publish(
                next(0, service.coordinator()),
                &publication(122, TELEMETRY, &[endpoint(1)], b"bounded retention"),
                clock(),
            )
            .unwrap();
        // Scheduling can recognize definite expiry, but this no-clock durable
        // report remains conservative until authoritative maintenance removes it.
        assert!(!service
            .has_pending_authored_deliveries(at(NOW + 601))
            .unwrap());
        assert_eq!(service.dependencies().unwrap().pending_authored_targets, 1);
        let plan = service.maintenance_plan(at(NOW + 601), 1, 128).unwrap();
        service
            .maintain(next(0, service.coordinator()), &plan)
            .unwrap();
        let after = service.coordinator().head().unwrap();
        let dependencies = service.dependencies().unwrap();
        assert_eq!(dependencies.pending_authored_records, 0);
        assert_eq!(dependencies.pending_authored_targets, 0);
        assert_eq!(dependencies.retained_records, 0);
        assert_eq!(service.coordinator().head().unwrap(), after);
    });
}

#[test]
fn corrupt_delivery_flags_and_missing_receipt_intents_are_incomplete_evidence() {
    use sha2::{Digest, Sha256};
    for (peer, tag, replacement) in [(0, b'p', Some(vec![2])), (1, b'q', None)] {
        let peers = Trio::enrolled();
        let (record, wire) = peers.with_service(0, |service| {
            let operation = next(0, service.coordinator());
            let record = service
                .publish(
                    operation,
                    &publication(123, TELEMETRY, &[endpoint(1)], b"evidence"),
                    clock(),
                )
                .unwrap()
                .record;
            (
                record,
                service.outgoing(operation).unwrap().unwrap().to_vec(),
            )
        });
        if peer == 1 {
            peers.receive(1, &wire);
        }
        let mut hash = Sha256::new();
        hash.update(GROUP.0);
        hash.update(record.0);
        let mut key = vec![tag];
        key.extend_from_slice(&hash.finalize());
        peers.with_adapter(peer, |adapter| {
            adapter
                .maintain(
                    next(peer, adapter.coordinator()),
                    &[],
                    |tx| match replacement {
                        Some(ref bytes) => tx.put(&key, bytes),
                        None => tx.delete(&key),
                    },
                )
                .unwrap();
        });
        peers.with_service(peer, |service| {
            let before = service.coordinator().head().unwrap();
            assert!(matches!(
                service.dependencies(),
                Err(Error::Fault(Fault::Corrupt))
            ));
            assert_eq!(service.coordinator().head().unwrap(), before);
        });
    }
}
