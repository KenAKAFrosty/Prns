use crate::engine::test_support::routable_descriptor;
use crate::engine::{
    CommandId, Directive, EngineReaction, InstantMillis, Journaled, LinkClosedReason,
    SendRequestFailure, SendToChannelFailure, Settlement, WakeSchedule, WakeSchedules,
};
use crate::interfaces::AttachedInterfaces;
use crate::routing::dedup::PacketHash;
use crate::routing::links::channel::table::{ChannelTable, OutstandingSend, TxOutcome};
use crate::routing::links::channel::{ChannelSequence, MessageType};
use crate::routing::links::maintenance::{stale_ms_from, timeout_grace_ms_from, write_link_close};
use crate::routing::links::resources::receive::tests_support::{
    engine_with_active_link, lane, link_id, link_key, track_pending_request,
};
use crate::routing::links::table::LinkPhase;
use crate::wire::BROADCAST_MTU;

#[test]
fn stale_link_teardown_disarms_channel_and_receipt_wakes_exactly_once() {
    let mut engine = engine_with_active_link();
    let link = link_id();
    let Some(LinkPhase::Active {
        last_inbound,
        keepalive_ms,
        rtt,
        ..
    }) = engine.links.phase_for(&link)
    else {
        panic!("fixture must have an active link");
    };
    let teardown =
        InstantMillis(last_inbound.0 + stale_ms_from(*keepalive_ms) + timeout_grace_ms_from(*rtt));
    let operation_deadline = InstantMillis(teardown.0 + 10_000);
    let channel = engine.channels.ensure(&link).unwrap();
    assert_eq!(
        engine.channels.push_outstanding(
            channel,
            OutstandingSend {
                packet_hash: PacketHash::new([0x31; 32]),
                command_id: CommandId(31),
                sequence: ChannelSequence(0),
                message_type: MessageType(7),
                body: b"channel",
                iv: [0x32; 16],
                sent_at: InstantMillis(2_100),
                timeout_at: operation_deadline,
            },
        ),
        TxOutcome::Tracked,
    );
    track_pending_request(&mut engine, CommandId(35), 2_100, operation_deadline.0);
    let descriptors = [routable_descriptor(lane())];
    let interfaces = AttachedInterfaces::new(&descriptors);
    let mut cached = engine.wake_schedules(interfaces);
    assert_eq!(
        (cached.channel_timeouts, cached.receipt_timeouts),
        (
            WakeSchedule::At(operation_deadline),
            WakeSchedule::At(operation_deadline),
        ),
    );

    let mut settlements = std::vec::Vec::new();
    let mut closed = std::vec::Vec::new();
    let mut frames = std::vec::Vec::new();
    let delta = engine.fire_due_link_deadlines(
        teardown,
        interfaces,
        &mut |bytes| bytes.fill(0x66),
        &mut |reaction| match reaction {
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                settlements.push((id, settlement));
            }
            EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) => {
                closed.push((link_id, reason));
            }
            EngineReaction::Directive(Directive::Send { target, bytes }) => {
                frames.push((target, bytes.to_vec()));
            }
            _ => panic!("unexpected teardown reaction"),
        },
    );
    cached.merge(delta);
    assert_eq!(cached, engine.wake_schedules(interfaces));
    assert_eq!(
        delta,
        WakeSchedules {
            link_deadlines: WakeSchedule::Idle,
            resource_deadlines: WakeSchedule::Idle,
            receipt_timeouts: WakeSchedule::Idle,
            channel_timeouts: WakeSchedule::Idle,
            remote_control_pairing: WakeSchedule::Idle,
            ..WakeSchedules::UNCHANGED
        },
    );
    assert_eq!(
        settlements,
        [
            (
                CommandId(31),
                Settlement::SendToChannel(Err(SendToChannelFailure::LinkClosed)),
            ),
            (
                CommandId(35),
                Settlement::SendRequest(Err(SendRequestFailure::LinkClosed)),
            ),
        ],
    );
    assert_eq!(closed, [(link, LinkClosedReason::Timeout)]);
    let mut expected_close = [0; BROADCAST_MTU];
    let length = write_link_close(&link, &link_key(), &[0x66; 16], &mut expected_close).unwrap();
    assert_eq!(frames, [(lane(), expected_close[..length].to_vec())]);
    assert!(engine.links.is_empty());
    assert!(engine.channels.is_empty());
    assert!(engine.receipts.is_empty());

    let mut reject_duplicate =
        |_: EngineReaction<'_>| panic!("retired operations must not emit again");
    cached.merge(engine.fire_due_link_deadlines(
        operation_deadline,
        interfaces,
        &mut |_| panic!("retired link must not consume entropy"),
        &mut reject_duplicate,
    ));
    cached.merge(engine.settle_timed_out_receipts(operation_deadline, &mut reject_duplicate));
    cached.merge(engine.fire_due_channel_timeouts(
        operation_deadline,
        interfaces,
        &mut |_| panic!("retired channel must not consume entropy"),
        &mut reject_duplicate,
    ));
    assert_eq!(cached, engine.wake_schedules(interfaces));
}
