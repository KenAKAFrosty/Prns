use core::cell::Cell;
use core::future::{poll_fn, Future};
use core::pin::pin;
use core::task::Poll;

use embassy_futures::join::join_array;
use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
use embassy_sync::mutex::Mutex;
use prns_core::interfaces::bluetooth_auto::{
    receive_frames_during, BleDuplexOutcome, BleFrameForwarder, BleLink, BleSink, BLE_HW_MTU,
};
use prns_core::interfaces::InterfaceId;
use prns_runtime::runtime::{EmbassyFleet as Fleet, InboundDeliveryError};

use super::{Active, BluetoothAutoStatus, BluetoothMemberStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SendState {
    NotSelected,
    Pending,
    Sent,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MemberSelection {
    Send,
    ReceiveOnly,
}

#[derive(Clone, Copy)]
enum ReceiveState {
    Receiving,
    Forwarding,
    Failed,
}

pub(super) struct MemberTransferState {
    send: Cell<SendState>,
    receive: Cell<ReceiveState>,
}

impl MemberTransferState {
    pub(super) fn new(selection: MemberSelection) -> Self {
        Self {
            send: Cell::new(match selection {
                MemberSelection::Send => SendState::Pending,
                MemberSelection::ReceiveOnly => SendState::NotSelected,
            }),
            receive: Cell::new(ReceiveState::Receiving),
        }
    }

    pub(super) fn needs_retirement(&self) -> bool {
        matches!(self.send.get(), SendState::Pending | SendState::Failed)
            || matches!(
                self.receive.get(),
                ReceiveState::Forwarding | ReceiveState::Failed
            )
    }
}

fn sends_pending(states: &[MemberTransferState]) -> bool {
    states
        .iter()
        .any(|state| state.send.get() == SendState::Pending)
}

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
    receive: &'a Cell<ReceiveState>,
}

impl<M: RawMutex + 'static, const F: usize, const N: usize, const L: usize> BleFrameForwarder
    for MemberInbound<'_, '_, M, F, N, L>
{
    type Error = InboundDeliveryError;

    async fn forward(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        self.receive.set(ReceiveState::Forwarding);
        self.fleet
            .lock()
            .await
            .deliver_inbound(self.id, frame)
            .await?;
        self.status.add_rx(frame.len() as u64);
        self.receive.set(ReceiveState::Receiving);
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
    state: &MemberTransferState,
    states: &[MemberTransferState; MEMBERS],
    frame: &[u8],
    inbound: &mut [u8; BLE_HW_MTU],
    fleet: &SharedFleet<'_, M, FRAME, NOTIFY, LIFECYCLE>,
    status: &BluetoothAutoStatus<MEMBERS>,
) {
    let Some(member) = member.as_mut() else {
        if state.send.get() == SendState::Pending {
            state.send.set(SendState::Failed);
        }
        return;
    };
    let status = status.member(member.slot);
    let work = async {
        if state.send.get() == SendState::Pending {
            if let Err(error) = member.sink.send_frame(frame).await {
                state.send.set(SendState::Failed);
                return Err(error);
            }
            // Preserve confirmed TX even if later forwarding is cancelled by the deadline.
            status.add_tx(frame.len() as u64);
            state.send.set(SendState::Sent);
        }
        // The enclosing join repolls all members when the last pending send settles.
        poll_fn(|_| {
            if sends_pending(states) {
                Poll::Pending
            } else {
                Poll::Ready(Ok(()))
            }
        })
        .await
    };
    let work = pin!(work);
    let mut forwarder = MemberInbound {
        fleet,
        id: member.id,
        status,
        receive: &state.receive,
    };
    match receive_frames_during(work, &mut member.source, inbound, &mut forwarder).await {
        BleDuplexOutcome::Finished(_) => {}
        BleDuplexOutcome::ReceiveFailed(_)
        | BleDuplexOutcome::InvalidReceiveLength(_)
        | BleDuplexOutcome::ForwardFailed(_) => {
            state.receive.set(ReceiveState::Failed);
            if state.send.get() == SendState::Pending {
                state.send.set(SendState::Failed);
            }
        }
    }
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
    states: &[MemberTransferState; MEMBERS],
    frame: &[u8],
    inbufs: &mut [[u8; BLE_HW_MTU]; MEMBERS],
    fleet: &mut Fleet<M, FRAME, NOTIFY, LIFECYCLE>,
    status: &BluetoothAutoStatus<MEMBERS>,
) {
    // Every contender belongs to this one joined future; no task or interrupt shares this lock.
    let fleet = Mutex::<NoopRawMutex, _>::new(fleet);
    let mut entries = members.iter_mut().zip(states.iter()).zip(inbufs.iter_mut());
    let futures: [_; MEMBERS] = ::core::array::from_fn(|_| {
        let ((member, state), inbound) = entries.next().expect("one entry per member slot");
        send_member(member, state, states, frame, inbound, &fleet, status)
    });
    let mut joined = pin!(join_array(futures));
    poll_fn(|context| {
        let had_pending = sends_pending(states);
        let result = joined.as_mut().poll(context);
        if result.is_pending() && had_pending && !sends_pending(states) {
            // Earlier slots may still be waiting on later sends. One extra pass observes the
            // transition; pending forwarding retains its own wake, with no busy-polling loop.
            return joined.as_mut().poll(context);
        }
        result
    })
    .await;
}

#[cfg(test)]
mod tests;
