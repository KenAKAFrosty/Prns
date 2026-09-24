use super::*;

pub fn response_header(link: LinkId) -> personal_rns::wire::WirePacketHeader {
    use personal_rns::wire::*;
    WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: link.to_address(),
        context: WireContext::Response,
    }
}

pub enum ReplyFault {
    Lose,
    Cancel,
}
pub fn reply_fault(triple: &mut Triple<'_, '_>, fault: ReplyFault) {
    let wire = triple.nodes[TARGET].wire.clone();
    let link = triple.links[0];
    wire.lose_after(response_header(link), 0, NonZeroUsize::MIN);
    let handle = triple.nodes[PRIMARY].handle.clone();
    let message = RemoteControlAppMessage::from_slice(b"admitted-before-cancel")
        .expect("bounded app message");
    let before = triple.messages.0.borrow().len();
    let pending = Operation::start(triple, async move {
        fixture::exchange(handle, link, RemoteControlRequest::AppMessage(message)).await
    });
    let gate = wire.clone();
    triple.complete(async move {
        gate.first_loss().await;
    });
    assert_eq!(
        triple.messages.0.borrow().len(),
        before + 1,
        "local cancellation cannot undo admitted app work"
    );
    healthy(triple);
    match fault {
        ReplyFault::Lose => assert_eq!(
            pending.finish(triple),
            Err(SendError::Failed(SendRequestFailure::Timeout))
        ),
        ReplyFault::Cancel => pending.cancel(triple),
    }
    assert_eq!(wire.stop_loss(), 1);
    triple.advance_for(100);
    healthy(triple);
}
