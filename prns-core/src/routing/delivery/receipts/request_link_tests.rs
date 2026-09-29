use super::*;
use crate::crypto::Ed25519PublicKey;
use crate::engine::{CommandId, InstantMillis, SendRequestIntent};
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::units::ByteLimit;

const HASH: PacketHash = PacketHash::new([0x41; 32]);

#[test]
fn receipt_pressure_returns_the_displaced_identity_and_preserves_the_new_owner() {
    let owner = LinkId::new([1; 16]);
    let old = receipt(
        owner,
        CommandId(7),
        RequestReceiptPolicy::ApplicationMaximum(11),
    );
    let new = OutstandingReceipt {
        packet_hash: PacketHash::new([0x51; 32]),
        command_id: CommandId(8),
        sent_at: InstantMillis(200),
        timeout_at: InstantMillis(9_000),
        ..old
    };
    let mut receipts = Receipts::<FixedReceiptTable<1>>::default();
    assert_eq!(receipts.track(old), None);
    assert_eq!(
        receipts.track(new),
        Some(CulledReceipt {
            packet_hash: old.packet_hash,
            command_id: old.command_id,
            kind: old.kind,
        })
    );
    assert_eq!(
        view(&receipts, &owner, RequestId::of_packet(&old.packet_hash)),
        RequestView {
            pending: false,
            command: None,
            limit: None,
            intent: None,
            deadline: None,
        }
    );
    assert_eq!(
        view(&receipts, &owner, RequestId::of_packet(&new.packet_hash)),
        RequestView {
            pending: true,
            command: Some(new.command_id),
            limit: Some(ByteLimit::Maximum(11)),
            intent: Some(SendRequestIntent::Application),
            deadline: Some(new.timeout_at),
        }
    );
    assert_eq!(
        receipts.pop_expired(new.timeout_at),
        Some(ExpiredReceipt {
            packet_hash: new.packet_hash,
            command_id: new.command_id,
            kind: new.kind,
        })
    );
    assert!(receipts.is_empty());

    let mut zero = Receipts::<FixedReceiptTable<0>>::default();
    assert_eq!(
        zero.track(new),
        Some(CulledReceipt {
            packet_hash: new.packet_hash,
            command_id: new.command_id,
            kind: new.kind,
        })
    );
    assert!(zero.is_empty());
}

#[derive(Debug, PartialEq, Eq)]
struct RequestView {
    pending: bool,
    command: Option<CommandId>,
    limit: Option<ByteLimit>,
    intent: Option<SendRequestIntent>,
    deadline: Option<InstantMillis>,
}

fn view<C: ReceiptTable>(receipts: &Receipts<C>, link: &LinkId, request: RequestId) -> RequestView {
    RequestView {
        pending: receipts.has_pending_request(link, request),
        command: receipts.pending_request_command(link, request),
        limit: receipts.pending_request_response_limit(link, request),
        intent: receipts.pending_request_intent(link, request),
        deadline: receipts.pending_request_deadline(link, request),
    }
}

fn receipt(
    link_id: LinkId,
    command_id: CommandId,
    response: RequestReceiptPolicy,
) -> OutstandingReceipt {
    OutstandingReceipt {
        packet_hash: HASH,
        command_id,
        kind: ReceiptKind::SendRequest { link_id, response },
        peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([0x42; 32])),
        sent_at: InstantMillis(100),
        timeout_at: InstantMillis(7_000),
    }
}

fn check_foreign_link<C: ReceiptTable + Default>(owner: LinkId, foreign: LinkId) {
    let request = RequestId::of_packet(&HASH);
    for policy in [
        RequestReceiptPolicy::ApplicationUnlimited,
        RequestReceiptPolicy::ApplicationMaximum(0),
        RequestReceiptPolicy::RemoteControlControllerPairingUnlimited,
        RequestReceiptPolicy::RemoteControlControllerPairingMaximum(32),
    ] {
        let mut receipts = Receipts::<C>::default();
        assert_eq!(receipts.track(receipt(owner, CommandId(7), policy)), None);
        let expected = RequestView {
            pending: true,
            command: Some(CommandId(7)),
            limit: Some(policy.maximum_response_bytes()),
            intent: Some(policy.intent()),
            deadline: Some(InstantMillis(7_000)),
        };
        assert_eq!(view(&receipts, &owner, request), expected);
        // Neither half of the key is sufficient by itself.
        for (link, id) in [(foreign, request), (owner, RequestId([0x40; 16]))] {
            assert_eq!(
                view(&receipts, &link, id),
                RequestView {
                    pending: false,
                    command: None,
                    limit: None,
                    intent: None,
                    deadline: None,
                }
            );
            receipts.claim_request_for_transfer(&link, id);
            assert_eq!(view(&receipts, &owner, request), expected);
            receipts.arm_request_timeout(&link, id, InstantMillis(9_000));
            assert_eq!(view(&receipts, &owner, request), expected);
            assert_eq!(receipts.settle_by_request_id(&link, id), None);
            assert_eq!(view(&receipts, &owner, request), expected);
        }
        assert_eq!(receipts.earliest_timeout_at(), Some(InstantMillis(7_000)));
        assert_eq!(
            receipts.pop_expired(InstantMillis(7_000)),
            Some(ExpiredReceipt {
                packet_hash: HASH,
                command_id: CommandId(7),
                kind: receipt(owner, CommandId(7), policy).kind,
            })
        );
        assert!(receipts.is_empty());
    }
}

#[test]
fn foreign_links_cannot_read_claim_rearm_or_settle_fixed_or_heap_receipts() {
    let owner = LinkId::new([0x51; 16]);
    let foreign = LinkId::new([0x52; 16]);
    check_foreign_link::<FixedReceiptTable<4>>(owner, foreign);
    #[cfg(feature = "alloc")]
    check_foreign_link::<HeapReceiptTable>(owner, foreign);
}

proptest::proptest! {
    #[test]
    fn arbitrary_distinct_links_preserve_request_ownership(owner in proptest::prelude::any::<[u8;16]>(), foreign in proptest::prelude::any::<[u8;16]>()) {
        proptest::prop_assume!(owner != foreign);
        check_foreign_link::<FixedReceiptTable<4>>(LinkId::new(owner), LinkId::new(foreign));
    }
}

fn check_colliding_requests<C: ReceiptTable + Default>() {
    let owner = LinkId::new([0x51; 16]);
    let other = LinkId::new([0x52; 16]);
    let request = RequestId::of_packet(&HASH);
    let mut receipts = Receipts::<C>::default();
    // Neither another send kind nor another link may shadow the matching row.
    let mut ordinary = receipt(
        owner,
        CommandId(6),
        RequestReceiptPolicy::ApplicationUnlimited,
    );
    ordinary.kind = ReceiptKind::SendToLink(owner);
    assert_eq!(receipts.track(ordinary), None);
    assert_eq!(
        receipts.track(receipt(
            owner,
            CommandId(7),
            RequestReceiptPolicy::ApplicationMaximum(11)
        )),
        None
    );
    assert_eq!(
        receipts.track(receipt(
            other,
            CommandId(8),
            RequestReceiptPolicy::RemoteControlControllerPairingMaximum(22)
        )),
        None
    );
    let expected_owner = RequestView {
        pending: true,
        command: Some(CommandId(7)),
        limit: Some(ByteLimit::Maximum(11)),
        intent: Some(SendRequestIntent::Application),
        deadline: Some(InstantMillis(7_000)),
    };
    let expected_other = RequestView {
        pending: true,
        command: Some(CommandId(8)),
        limit: Some(ByteLimit::Maximum(22)),
        intent: Some(SendRequestIntent::RemoteControlControllerPairing),
        deadline: Some(InstantMillis(7_000)),
    };
    assert_eq!(view(&receipts, &owner, request), expected_owner);
    assert_eq!(view(&receipts, &other, request), expected_other);
    receipts.claim_request_for_transfer(&other, request);
    assert_eq!(view(&receipts, &owner, request), expected_owner);
    assert_eq!(
        view(&receipts, &other, request),
        RequestView {
            deadline: None,
            ..expected_other
        }
    );
    receipts.arm_request_timeout(&other, request, InstantMillis(9_000));
    assert_eq!(view(&receipts, &owner, request), expected_owner);
    assert_eq!(
        view(&receipts, &other, request),
        RequestView {
            deadline: Some(InstantMillis(9_000)),
            ..expected_other
        }
    );
    for (link, command_id, intent) in [
        (
            other,
            CommandId(8),
            SendRequestIntent::RemoteControlControllerPairing,
        ),
        (owner, CommandId(7), SendRequestIntent::Application),
    ] {
        assert_eq!(
            receipts.settle_by_request_id(&link, request),
            Some(ProvenRequestReceipt {
                command_id,
                intent,
                sent_at: InstantMillis(100)
            })
        );
        assert_eq!(receipts.settle_by_request_id(&link, request), None);
    }
    assert_eq!(
        receipts.pop_expired(InstantMillis(7_000)),
        Some(ExpiredReceipt {
            packet_hash: HASH,
            command_id: CommandId(6),
            kind: ReceiptKind::SendToLink(owner)
        })
    );
    assert!(receipts.is_empty());
}

#[test]
fn equal_request_ids_keep_independent_owners_in_fixed_and_heap_storage() {
    check_colliding_requests::<FixedReceiptTable<4>>();
    #[cfg(feature = "alloc")]
    check_colliding_requests::<HeapReceiptTable>();
}
