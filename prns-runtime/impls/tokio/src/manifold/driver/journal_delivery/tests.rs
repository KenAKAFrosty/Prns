use tokio::sync::{mpsc, oneshot};

use super::*;
use crate::engine::PacketReceiptDelivered;
use crate::routing::links::resources::ResourceHash;

fn delivered(ms: u64) -> PacketReceiptDelivered {
    PacketReceiptDelivered {
        rtt: RttMillis::new(ms),
        evidence: crate::engine::DeliveryEvidence::Proof(crate::engine::DeliveryProof::Implicit(
            crate::routing::dedup::PacketHash::new([0; 32]),
        )),
    }
}

#[test]
fn settle_fires_the_awaited_completion_and_suppresses_the_event() {
    let mut delivery = JournalDelivery::default();
    let (completion, mut settled) = oneshot::channel();
    delivery.register_completion(CommandId(7), completion);

    let settlement = Settlement::SendSinglePacket(Ok(delivered(9)));
    let forwarded = delivery.route(Journaled::CommandSettled {
        id: CommandId(7),
        settlement: settlement.clone(),
    });

    assert!(
        forwarded.is_none(),
        "an awaited settlement is consumed, not forwarded to the app"
    );
    assert_eq!(
        settled
            .try_recv()
            .expect("the awaiter received its settlement"),
        settlement
    );
    assert!(
        delivery.completions.is_empty(),
        "the awaiter is removed from the registry once fired"
    );
}

#[test]
fn settle_forwards_a_settlement_nobody_awaits() {
    let mut delivery = JournalDelivery::default();
    let forwarded = delivery.route(Journaled::CommandSettled {
        id: CommandId(3),
        settlement: Settlement::SendSinglePacket(Ok(delivered(1))),
    });
    assert!(
        forwarded.is_some(),
        "a settlement with no awaiter passes through to on_event"
    );
}

const RES_LINK: LinkId = LinkId::new([0x44; 16]);

fn resource_delivery() -> (JournalDelivery, mpsc::UnboundedReceiver<ResourceInbound>) {
    let mut delivery = JournalDelivery::default();
    let (sink, receiver) = mpsc::unbounded_channel();
    delivery.register_resource_sink(RES_LINK, sink);
    (delivery, receiver)
}

#[test]
fn route_resource_routes_a_segment_and_keeps_the_sink() {
    let (mut delivery, mut receiver) = resource_delivery();
    let forwarded = delivery.route(Journaled::ResourceSegmentReceived {
        link_id: RES_LINK,
        original_hash: ResourceHash::new([1; 32]),
        segment_index: 1,
        total_segments: 2,
        metadata: None,
        data: b"first",
    });
    assert!(
        forwarded.is_none(),
        "a routed segment is suppressed from the app event stream"
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(ResourceInbound::Chunk(chunk)) if chunk == b"first"
    ));
    assert!(
        delivery.resource_sinks.contains_key(&RES_LINK),
        "the sink stays for the segments still to come"
    );
}

#[test]
fn route_resource_completes_and_retires_on_assembly() {
    let (mut delivery, mut receiver) = resource_delivery();
    let forwarded = delivery.route(Journaled::ResourceAssembled {
        link_id: RES_LINK,
        original_hash: ResourceHash::new([2; 32]),
        total_size_bytes: 4096,
    });
    assert!(forwarded.is_none());
    assert!(matches!(
        receiver.try_recv(),
        Ok(ResourceInbound::Complete {
            total_size_bytes: 4096,
            ..
        })
    ));
    assert!(
        delivery.resource_sinks.is_empty(),
        "an assembled resource retires its one-shot sink"
    );
}

#[test]
fn route_resource_delivers_a_single_segment_then_retires() {
    let (mut delivery, mut receiver) = resource_delivery();
    let forwarded = delivery.route(Journaled::ResourceReceived {
        link_id: RES_LINK,
        hash: ResourceHash::new([3; 32]),
        metadata: None,
        data: b"whole",
    });
    assert!(forwarded.is_none());
    assert!(matches!(
        receiver.try_recv(),
        Ok(ResourceInbound::Chunk(chunk)) if chunk == b"whole"
    ));
    assert!(matches!(
        receiver.try_recv(),
        Ok(ResourceInbound::Complete {
            total_size_bytes: 5,
            ..
        })
    ));
    assert!(
        delivery.resource_sinks.is_empty(),
        "a single-segment resource completes and retires in one go"
    );
}

#[test]
fn route_resource_passes_through_an_unregistered_link() {
    let mut delivery = JournalDelivery::default();
    let forwarded = delivery.route(Journaled::ResourceSegmentReceived {
        link_id: RES_LINK,
        original_hash: ResourceHash::new([4; 32]),
        segment_index: 1,
        total_segments: 2,
        metadata: None,
        data: b"x",
    });
    assert!(
        forwarded.is_some(),
        "with no sink registered the journal flows on to the app event stream"
    );
}
