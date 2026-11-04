#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{
    ControlReference, Endpoint, EndpointId, EndpointIncarnation, GroupId, NamespaceId, Publication,
    PublicationValidity, SchemaId, Seconds, Timestamp,
};
use alloc::vec;
fn limits() -> Limits {
    Limits {
        max_records: 8,
        max_targets: 2,
        max_body_bytes: 128,
        max_changes: 16,
        max_envelopes: 16,
        max_retained_wire_bytes: 8192,
        maximum_window: Seconds(60),
        future_skew: Seconds(0),
    }
}
fn endpoint(n: u8) -> Endpoint {
    Endpoint {
        id: EndpointId([n; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}
fn put(key: &[u8], value: &[u8]) -> Mutation {
    let mut prefixed = vec![0];
    prefixed.extend_from_slice(key);
    Mutation::Put {
        key: prefixed,
        value: value.to_vec(),
    }
}
fn writes() -> (Vec<Mutation>, crate::RecordId) {
    let group = GroupId([1; 32]);
    let control = ControlReference {
        head: prns_groups_core::ControlId([2; 32]),
        era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([3; 32])),
    };
    let encoded = codec::encode_record(
        group,
        control,
        endpoint(1),
        &Publication {
            nonce: [4; 32],
            namespace: NamespaceId(9),
            schema: SchemaId(4),
            validity: PublicationValidity::new(Timestamp(10), Timestamp(20), Seconds(60)).unwrap(),
            targets: &[endpoint(2)],
            body: b"opaque\0bytes",
        },
        limits(),
    )
    .unwrap();
    let id = decode_record(&encoded, limits()).unwrap().metadata.record;
    let change = crate::Change {
        cursor: ChangeCursor(7),
        operation: crate::OperationId([8; 32]),
        kind: ChangeKind::Published(id),
    };
    (
        vec![
            put(&storage::record_key(b'r', group, id), &encoded),
            put(
                &storage::feed_key(group, change.cursor),
                &storage::encode_change(&change).unwrap(),
            ),
        ],
        id,
    )
}
#[test]
fn canonical_insert_and_receipt_projection_ignores_crypto_and_deletions() {
    let (mut changes, id) = writes();
    let group = GroupId([1; 32]);
    let receipt = DurableReceipt {
        group,
        record: id,
        target: endpoint(2),
    };
    let change = crate::Change {
        cursor: ChangeCursor(8),
        operation: crate::OperationId([9; 32]),
        kind: ChangeKind::Receipt(receipt),
    };
    changes.push(put(
        &storage::feed_key(group, change.cursor),
        &storage::encode_change(&change).unwrap(),
    ));
    changes.push(Mutation::Put {
        key: b"mls-state".to_vec(),
        value: vec![1, 2, 3],
    });
    let projected = records(&changes, 8, limits()).unwrap();
    assert_eq!(projected.len(), 2);
    let RecordWrite::Insert(insert) = &projected[0] else {
        panic!("insert")
    };
    assert_eq!(insert.record.body.as_slice(), b"opaque\0bytes");
    assert_eq!(insert.position, ChangeCursor(7));
    assert!(insert.authored);
    assert!(matches!(projected[1],RecordWrite::Receipt(found) if found==receipt));
    let deleted = Mutation::Delete {
        key: changes[0].key().to_vec(),
    };
    assert!(records(&[deleted], 8, limits()).unwrap().is_empty());
}
#[test]
fn incomplete_duplicate_malformed_or_oversize_projection_fails_closed() {
    let (changes, _) = writes();
    assert!(records(&changes, 1, limits()).is_err());
    assert!(records(&changes[..1], 8, limits()).is_err());
    assert!(records(&changes[1..], 8, limits()).is_err());
    let (mut changes, _) = writes();
    changes.push(put(
        &storage::feed_key(GroupId([1; 32]), ChangeCursor(7)),
        b"broken",
    ));
    assert!(records(&changes, 8, limits()).is_err());
    assert!(records(&[put(b"F", b"broken")], 8, limits()).is_err());
}
