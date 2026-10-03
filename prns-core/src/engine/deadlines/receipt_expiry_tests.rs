use crate::crypto::Ed25519PublicKey;
use crate::engine::test_support::TestStorageLayout;
use crate::engine::{
    CommandId, EngineReaction, EngineState, InstantMillis, Journaled, SendRequestFailure,
    Settlement,
};
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::routing::delivery::receipts::{OutstandingReceipt, ReceiptKind, RequestReceiptPolicy};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::assembly::AssemblyBytes;
use crate::routing::links::resources::assembly::{AssemblyCorrelation, AssemblyProgress};
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::resources::ResourceSegment;
use crate::routing::links::LinkId;

#[test]
fn receipt_expiry_reclaims_only_its_own_response_assembly() {
    let owner = LinkId::new([1; 16]);
    let other = LinkId::new([2; 16]);
    let packet = PacketHash::new([3; 32]);
    let request = RequestId::of_packet(&packet);
    let hash = ResourceHash::new([4; 32]);
    for (correlation, retained) in [
        (None, None),
        (Some(AssemblyCorrelation::Response(request)), None),
        (
            Some(AssemblyCorrelation::Unsolicited),
            Some(AssemblyCorrelation::Unsolicited),
        ),
        (
            Some(AssemblyCorrelation::Request(request)),
            Some(AssemblyCorrelation::Request(request)),
        ),
        (
            Some(AssemblyCorrelation::Response(RequestId([5; 16]))),
            Some(AssemblyCorrelation::Response(RequestId([5; 16]))),
        ),
    ] {
        let mut state = EngineState::<TestStorageLayout>::default();
        if let Some(correlation) = correlation {
            state
                .incoming_assemblies
                .begin(owner, hash, 2, 19, correlation);
            assert_eq!(
                state.incoming_assemblies.advance(
                    &owner,
                    &hash,
                    ResourceSegment {
                        index: 1,
                        total_segments: 2,
                        total_data_bytes: 19
                    },
                    AssemblyBytes {
                        stream: 17,
                        value: 17
                    },
                    correlation
                ),
                Some(AssemblyProgress::Assembling)
            );
        }
        state
            .incoming_assemblies
            .begin(other, hash, 2, 19, AssemblyCorrelation::Response(request));
        assert_eq!(
            state.incoming_assemblies.advance(
                &other,
                &hash,
                ResourceSegment {
                    index: 1,
                    total_segments: 2,
                    total_data_bytes: 19
                },
                AssemblyBytes {
                    stream: 17,
                    value: 17
                },
                AssemblyCorrelation::Response(request)
            ),
            Some(AssemblyProgress::Assembling)
        );
        assert_eq!(
            state.receipts.track(OutstandingReceipt {
                packet_hash: packet,
                command_id: CommandId(7),
                kind: ReceiptKind::SendRequest {
                    link_id: owner,
                    response: RequestReceiptPolicy::ApplicationUnlimited
                },
                peer_signing_key: IdentitySigningPublicKey::new(Ed25519PublicKey([6; 32])),
                sent_at: InstantMillis(100),
                timeout_at: InstantMillis(200),
            }),
            None
        );
        for (now, expected) in [
            (InstantMillis(199), None),
            (
                InstantMillis(200),
                Some((
                    CommandId(7),
                    Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
                )),
            ),
            (InstantMillis(201), None),
        ] {
            let mut settlement = None;
            state.settle_timed_out_receipts(now, &mut |reaction| {
                let EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: value,
                }) = reaction
                else {
                    unreachable!("receipt expiry emits only its settlement");
                };
                assert_eq!(settlement.replace((id, value)), None);
            });
            assert_eq!(settlement, expected);
            let expected_owner = if now == InstantMillis(199) {
                correlation
            } else {
                retained
            };
            assert_eq!(
                (
                    state.incoming_assemblies.original_hash(&owner),
                    state.incoming_assemblies.correlation(&owner)
                ),
                (expected_owner.map(|_| hash), expected_owner)
            );
        }
        assert!(state.receipts.is_empty());
        for (link, correlation) in [
            (owner, retained),
            (other, Some(AssemblyCorrelation::Response(request))),
        ] {
            if let Some(correlation) = correlation {
                assert_eq!(
                    state.incoming_assemblies.advance(
                        &link,
                        &hash,
                        ResourceSegment {
                            index: 2,
                            total_segments: 2,
                            total_data_bytes: 19
                        },
                        AssemblyBytes {
                            stream: 2,
                            value: 2
                        },
                        correlation
                    ),
                    Some(AssemblyProgress::Complete {
                        total_size_bytes: 19
                    })
                );
            }
        }
    }
}
