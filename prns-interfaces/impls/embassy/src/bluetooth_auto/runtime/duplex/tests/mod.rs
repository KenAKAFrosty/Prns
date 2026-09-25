#![allow(clippy::unwrap_used)]

use core::cell::Cell;
use core::future::{poll_fn, Future};
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::{boxed::Box, rc::Rc, vec, vec::Vec};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use prns_core::interfaces::bluetooth_auto::{
    BleAddress, BleIdentity, BleSource, Control, L2capPlan,
};
use prns_core::interfaces::InterfaceStatus;
use prns_runtime::manifold::driver::InterfaceLifecycle;
use prns_runtime::runtime::{ManifoldLaneSet, StaticManifoldLane};

use super::super::BluetoothAutoShared;
use super::*;

type Mtx = CriticalSectionRawMutex;
const PEERS: usize = 3;
const FRAME: usize = 8;
type TestFleet = Fleet<Mtx, FRAME, PEERS, 1>;

#[derive(Debug, PartialEq, Eq)]
struct Closed;

enum Incoming {
    Frame(Vec<u8>),
    InvalidLength,
    Closed,
    Pending,
}

struct Source {
    next: Incoming,
    received: Rc<Cell<bool>>,
}

impl BleSource for Source {
    type Error = Closed;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
        match core::mem::replace(&mut self.next, Incoming::Pending) {
            Incoming::Frame(frame) => {
                out[..frame.len()].copy_from_slice(&frame);
                self.received.set(true);
                Ok(frame.len())
            }
            Incoming::InvalidLength => Ok(usize::MAX),
            Incoming::Closed => Err(Closed),
            Incoming::Pending => core::future::pending().await,
        }
    }
}

enum Sending {
    Ready,
    AfterReceive,
    Blocked,
    Failed,
    Gated(Rc<Cell<bool>>),
}

struct Sink {
    mode: Sending,
    received: Rc<Cell<bool>>,
    starts: Rc<Cell<usize>>,
}

impl BleSink for Sink {
    type Error = Closed;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
        assert_eq!(frame, b"send");
        self.starts.set(self.starts.get() + 1);
        poll_fn(|_| match &self.mode {
            Sending::Ready => Poll::Ready(Ok(())),
            Sending::AfterReceive if self.received.get() => Poll::Ready(Ok(())),
            Sending::AfterReceive | Sending::Blocked => Poll::Pending,
            Sending::Failed => Poll::Ready(Err(Closed)),
            Sending::Gated(ready) => {
                if ready.get() {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }
        })
        .await
    }
}

enum Link {}

impl BleLink for Link {
    type Error = Closed;
    type Source = Source;
    type Sink = Sink;

    fn peer_protocol(&self) -> prns_core::interfaces::bluetooth_auto::PeerProtocol {
        match *self {}
    }

    fn address(&self) -> BleAddress {
        match *self {}
    }
    async fn control_send(&mut self, _: &Control) -> Result<(), Closed> {
        match *self {}
    }
    async fn control_recv(&mut self) -> Result<Control, Closed> {
        match *self {}
    }
    async fn upgrade(&mut self, _: &L2capPlan) -> Result<(), Closed> {
        match *self {}
    }
    fn into_data(self) -> (Source, Sink) {
        match self {}
    }
}

fn member(slot: usize, next: Incoming, mode: Sending) -> Active<Link> {
    let received = Rc::new(Cell::new(false));
    Active {
        identity: BleIdentity::new([slot as u8; 16]),
        id: InterfaceId::new([slot as u8; 8]),
        slot,
        address: BleAddress::new([slot as u8; 6]),
        source: Source {
            next,
            received: received.clone(),
        },
        sink: Sink {
            mode,
            received,
            starts: Rc::new(Cell::new(0)),
        },
    }
}

fn fixture() -> (
    TestFleet,
    BluetoothAutoStatus<PEERS>,
    &'static Channel<Mtx, InterfaceId, PEERS>,
) {
    let id = InterfaceId::new([9; 8]);
    let lane = Box::leak(Box::new(StaticManifoldLane::<Mtx, FRAME, PEERS>::new()));
    let wake = Box::leak(Box::new(Signal::new()));
    let notify = Box::leak(Box::new(Channel::new()));
    let lifecycle = Box::leak(Box::new(Channel::<Mtx, InterfaceLifecycle, 1>::new()));
    let mut lanes = ManifoldLaneSet::<Mtx, 1, PEERS>::new();
    let fleet = lanes
        .claim_supervisor(lane, id, wake)
        .unwrap()
        .into_fleet(notify.sender(), lifecycle.sender());
    let shared = Box::leak(Box::new(BluetoothAutoShared::new(id)));
    let status = BluetoothAutoStatus::new(shared);
    for slot in 0..PEERS {
        status
            .member(slot)
            .assign(InterfaceId::new([slot as u8; 8]));
    }
    (fleet, status, notify)
}

#[test]
fn fanout_sends_once_and_stops_receiving_after_the_last_send() {
    let (mut fleet, status, notify) = fixture();
    let mut members = [
        Some(member(
            0,
            Incoming::Frame(vec![1, 2, 3]),
            Sending::AfterReceive,
        )),
        Some(member(
            1,
            Incoming::Frame(vec![4, 5]),
            Sending::AfterReceive,
        )),
        Some(member(2, Incoming::Frame(vec![6]), Sending::Ready)),
    ];
    let states = [
        MemberSelection::Send,
        MemberSelection::Send,
        MemberSelection::ReceiveOnly,
    ]
    .map(MemberTransferState::new);
    let mut bufs = [[0; BLE_HW_MTU]; PEERS];
    {
        let mut send = pin!(send_members(
            &mut members,
            &states,
            b"send",
            &mut bufs,
            &mut fleet,
            &status
        ));
        assert!(send
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready());
    }
    assert!(matches!(
        states.each_ref().map(|state| state.send.get()),
        [SendState::Sent, SendState::Sent, SendState::NotSelected]
    ));
    assert_eq!(
        members
            .each_ref()
            .map(|m| m.as_ref().unwrap().sink.starts.get()),
        [1, 1, 0]
    );
    assert_eq!(
        (
            status.member(0).rx_bytes(),
            status.member(1).rx_bytes(),
            status.member(2).rx_bytes()
        ),
        (3, 2, 0)
    );
    assert_eq!(
        (
            status.member(0).tx_bytes(),
            status.member(1).tx_bytes(),
            status.member(2).tx_bytes()
        ),
        (4, 4, 0)
    );
    assert_eq!(notify.try_receive(), Ok(InterfaceId::new([0; 8])));
    assert_eq!(notify.try_receive(), Ok(InterfaceId::new([1; 8])));
    assert!(notify.try_receive().is_err());
    let mut expected = [[0; BLE_HW_MTU]; PEERS];
    expected[0][..3].copy_from_slice(&[1, 2, 3]);
    expected[1][..2].copy_from_slice(&[4, 5]);
    assert_eq!(bufs, expected);
}

#[test]
fn blocked_peer_does_not_hide_completed_fanout_and_cancellation_retains_exact_tx_counts() {
    let (mut fleet, status, _) = fixture();
    let mut members = [
        Some(member(0, Incoming::Pending, Sending::Ready)),
        Some(member(1, Incoming::Pending, Sending::Blocked)),
        None,
    ];
    let states = [
        MemberSelection::Send,
        MemberSelection::Send,
        MemberSelection::ReceiveOnly,
    ]
    .map(MemberTransferState::new);
    let mut bufs = [[0; BLE_HW_MTU]; PEERS];
    {
        let mut send = pin!(send_members(
            &mut members,
            &states,
            b"send",
            &mut bufs,
            &mut fleet,
            &status
        ));
        assert!(send
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    assert!(matches!(
        states.each_ref().map(|state| state.send.get()),
        [SendState::Sent, SendState::Pending, SendState::NotSelected]
    ));
    assert_eq!(
        (status.member(0).tx_bytes(), status.member(1).tx_bytes()),
        (4, 0)
    );
}

#[test]
fn failed_receives_sends_and_forwarding_are_isolated_from_other_selected_peers() {
    for (incoming, sending) in [
        (Incoming::InvalidLength, Sending::Blocked),
        (Incoming::Closed, Sending::Blocked),
        (Incoming::Pending, Sending::Failed),
        (Incoming::Frame(vec![7; FRAME + 1]), Sending::Blocked),
    ] {
        let (mut fleet, status, notify) = fixture();
        let mut members = [
            Some(member(0, incoming, sending)),
            Some(member(
                1,
                Incoming::Frame(vec![4, 5]),
                Sending::AfterReceive,
            )),
            None,
        ];
        let states = [
            MemberSelection::Send,
            MemberSelection::Send,
            MemberSelection::ReceiveOnly,
        ]
        .map(MemberTransferState::new);
        let mut bufs = [[0; BLE_HW_MTU]; PEERS];
        {
            let mut send = pin!(send_members(
                &mut members,
                &states,
                b"send",
                &mut bufs,
                &mut fleet,
                &status
            ));
            assert!(send
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_ready());
        }
        assert!(matches!(
            states.each_ref().map(|state| state.send.get()),
            [SendState::Failed, SendState::Sent, SendState::NotSelected]
        ));
        assert_eq!(
            (status.member(0).rx_bytes(), status.member(0).tx_bytes()),
            (0, 0)
        );
        assert_eq!(
            (status.member(1).rx_bytes(), status.member(1).tx_bytes()),
            (2, 4)
        );
        assert_eq!(notify.try_receive(), Ok(InterfaceId::new([1; 8])));
        assert!(notify.try_receive().is_err());
    }
}

#[test]
fn shared_lane_pressure_preserves_received_frames_and_already_completed_sends() {
    let (mut fleet, status, notify) = fixture();
    let placeholder = InterfaceId::new([99; 8]);
    for _ in 0..PEERS {
        notify.try_send(placeholder).unwrap();
    }
    let mut members = [
        Some(member(0, Incoming::Frame(vec![1]), Sending::AfterReceive)),
        Some(member(1, Incoming::Frame(vec![2]), Sending::AfterReceive)),
        None,
    ];
    let states = [
        MemberSelection::Send,
        MemberSelection::Send,
        MemberSelection::ReceiveOnly,
    ]
    .map(MemberTransferState::new);
    let mut bufs = [[0; BLE_HW_MTU]; PEERS];
    {
        let mut send = pin!(send_members(
            &mut members,
            &states,
            b"send",
            &mut bufs,
            &mut fleet,
            &status
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(send.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            (status.member(0).tx_bytes(), status.member(1).tx_bytes()),
            (4, 4)
        );
        assert_eq!(
            (status.member(0).rx_bytes(), status.member(1).rx_bytes()),
            (0, 0)
        );
        for _ in 0..PEERS {
            assert_eq!(notify.try_receive(), Ok(placeholder));
        }
        assert!(send.as_mut().poll(&mut context).is_ready());
    }
    assert!(matches!(
        states.each_ref().map(|state| state.send.get()),
        [SendState::Sent, SendState::Sent, SendState::NotSelected]
    ));
    assert_eq!(
        (status.member(0).rx_bytes(), status.member(1).rx_bytes()),
        (1, 1)
    );
    assert_eq!(notify.try_receive(), Ok(InterfaceId::new([0; 8])));
    assert_eq!(notify.try_receive(), Ok(InterfaceId::new([1; 8])));
    assert!(notify.try_receive().is_err());
}

#[test]
fn cancellation_during_forwarding_keeps_confirmed_tx_without_reporting_rx_completion() {
    let (mut fleet, status, notify) = fixture();
    for _ in 0..PEERS {
        notify.try_send(InterfaceId::new([99; 8])).unwrap();
    }
    let mut members = [
        Some(member(0, Incoming::Frame(vec![1]), Sending::AfterReceive)),
        Some(member(1, Incoming::Frame(vec![2]), Sending::AfterReceive)),
        None,
    ];
    let starts = members[..2]
        .iter()
        .map(|member| member.as_ref().unwrap().sink.starts.clone())
        .collect::<Vec<_>>();
    let states = [
        MemberSelection::Send,
        MemberSelection::Send,
        MemberSelection::ReceiveOnly,
    ]
    .map(MemberTransferState::new);
    let mut bufs = [[0; BLE_HW_MTU]; PEERS];
    {
        let mut send = pin!(send_members(
            &mut members,
            &states,
            b"send",
            &mut bufs,
            &mut fleet,
            &status
        ));
        assert!(send
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    assert!(matches!(
        states.each_ref().map(|state| state.send.get()),
        [SendState::Sent, SendState::Sent, SendState::NotSelected]
    ));
    assert_eq!(
        starts.iter().map(|starts| starts.get()).collect::<Vec<_>>(),
        vec![1, 1]
    );
    assert_eq!(
        (status.member(0).tx_bytes(), status.member(1).tx_bytes()),
        (4, 4)
    );
    assert_eq!(
        (status.member(0).rx_bytes(), status.member(1).rx_bytes()),
        (0, 0)
    );
    assert_eq!(
        states.each_ref().map(MemberTransferState::needs_retirement),
        [true, true, false]
    );
    // Unsettled forwarding must retire the transport even though both sends succeeded.
    drop(members);
}

mod receive_progress;
