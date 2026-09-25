use embassy_futures::join::join_array;
use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
use embassy_sync::mutex::Mutex;
use prns_core::interfaces::bluetooth_auto::{
    send_frame_duplex, BleDuplexOutcome, BleFrameForwarder, BleLink, BleSink, BLE_HW_MTU,
};
use prns_core::interfaces::InterfaceId;
use prns_runtime::runtime::{EmbassyFleet as Fleet, InboundDeliveryError};

use super::{Active, BluetoothAutoStatus, BluetoothMemberStatus, SendState};

type SharedFleet<'a, M, const FRAME: usize, const NOTIFY: usize, const LIFECYCLE: usize> =
    Mutex<NoopRawMutex, &'a mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>>;

struct MemberInbound<
    'a,
    'fleet,
    M: RawMutex + 'static,
    const F: usize,
    const N: usize,
    const L: usize,
> {
    fleet: &'a SharedFleet<'fleet, M, F, N, L>,
    id: InterfaceId,
    status: &'a BluetoothMemberStatus,
}

impl<M: RawMutex + 'static, const F: usize, const N: usize, const L: usize> BleFrameForwarder
    for MemberInbound<'_, '_, M, F, N, L>
{
    type Error = InboundDeliveryError;

    async fn forward(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        self.fleet
            .lock()
            .await
            .deliver_inbound(self.id, frame)
            .await?;
        self.status.add_rx(frame.len() as u64);
        Ok(())
    }
}

struct CountedSink<'a, S> {
    sink: &'a mut S,
    status: &'a BluetoothMemberStatus,
}

impl<S: BleSink> BleSink for CountedSink<'_, S> {
    type Error = S::Error;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        self.sink.send_frame(frame).await?;
        // A send can settle while forwarding still waits. Preserve its accounting even if the
        // surrounding fanout deadline later cancels that forwarding and retires the peer.
        self.status.add_tx(frame.len() as u64);
        Ok(())
    }
}

async fn send_member<
    L: BleLink,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    member: &mut Option<Active<L>>,
    state: &mut SendState,
    frame: &[u8],
    inbound: &mut [u8; BLE_HW_MTU],
    fleet: &SharedFleet<'_, M, FRAME, NOTIFY, LIFECYCLE>,
    status: &BluetoothAutoStatus<MEMBERS>,
) {
    if *state != SendState::Pending {
        return;
    }
    let Some(member) = member.as_mut() else {
        *state = SendState::Failed;
        return;
    };
    let status = status.member(member.slot);
    let mut sink = CountedSink {
        sink: &mut member.sink,
        status,
    };
    *state = match send_frame_duplex(
        &mut member.source,
        &mut sink,
        frame,
        inbound,
        MemberInbound {
            fleet,
            id: member.id,
            status,
        },
    )
    .await
    {
        BleDuplexOutcome::Finished(Ok(())) => SendState::Sent,
        BleDuplexOutcome::Finished(Err(_))
        | BleDuplexOutcome::ReceiveFailed(_)
        | BleDuplexOutcome::InvalidReceiveLength(_)
        | BleDuplexOutcome::ForwardFailed(_) => SendState::Failed,
    };
}

#[expect(
    clippy::expect_used,
    reason = "all three zipped arrays have exactly MEMBERS entries"
)]
pub(super) async fn send_members<
    L: BleLink,
    M: RawMutex + 'static,
    const FRAME: usize,
    const NOTIFY: usize,
    const LIFECYCLE: usize,
    const MEMBERS: usize,
>(
    members: &mut [Option<Active<L>>; MEMBERS],
    states: &mut [SendState; MEMBERS],
    frame: &[u8],
    inbufs: &mut [[u8; BLE_HW_MTU]; MEMBERS],
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    status: &BluetoothAutoStatus<MEMBERS>,
) {
    // Every contender belongs to this one joined future; no task or interrupt shares this lock.
    let fleet = Mutex::<NoopRawMutex, _>::new(fleet);
    let mut entries = members
        .iter_mut()
        .zip(states.iter_mut())
        .zip(inbufs.iter_mut());
    let futures: [_; MEMBERS] = ::core::array::from_fn(|_| {
        let ((member, state), inbound) = entries.next().expect("one entry per member slot");
        send_member(member, state, frame, inbound, &fleet, status)
    });
    join_array(futures).await;
}

#[cfg(test)]
mod tests;
