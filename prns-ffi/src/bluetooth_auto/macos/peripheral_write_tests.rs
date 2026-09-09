use std::collections::HashMap;
use std::future::{poll_fn, Future};
use std::task::Poll;
use std::time::Duration;

use prns_core::interfaces::bluetooth_auto::{
    AppleHost, BleIdentity, Control, Endpoint, LinkCapabilities, PeerProtocol, CONTROL_MAX_LEN,
};
use tokio::sync::mpsc;

use super::gatt_link::{gatt_inbound_channel_with_budget, GattInboundReceiver};
use super::peripheral_write::{
    admit_write_batch, respond_to_write_batch, InboundProfile, WriteError, WriteRequest,
    WriteSession, WriteTarget,
};
use super::CoreBluetoothPeerId;

struct TestLink {
    peer: CoreBluetoothPeerId,
    context: u8,
    protocol: PeerProtocol,
    identity: Option<BleIdentity>,
    control: mpsc::Receiver<Control>,
    data: GattInboundReceiver,
}

struct Harness {
    sessions: HashMap<CoreBluetoothPeerId, WriteSession<u8>>,
    inbound: mpsc::Sender<TestLink>,
    links: mpsc::Receiver<TestLink>,
    capacity: usize,
}

impl Harness {
    fn new(capacity: usize, inbound_capacity: usize) -> Self {
        let (inbound, links) = mpsc::channel(inbound_capacity);
        Self {
            sessions: HashMap::new(),
            inbound,
            links,
            capacity,
        }
    }

    fn admit(
        &mut self,
        requests: impl IntoIterator<Item = Result<WriteRequest<u8>, WriteError>>,
    ) -> Result<(), WriteError> {
        admit_write_batch(
            true,
            requests,
            &mut self.sessions,
            self.capacity,
            &self.inbound,
            make_link,
        )
    }

    fn add_session(
        &mut self,
        peer: CoreBluetoothPeerId,
        profile: InboundProfile,
        control_capacity: usize,
        data_budget: usize,
    ) -> TestLink {
        let (control_tx, control) = mpsc::channel(control_capacity);
        let (data_tx, data) = gatt_inbound_channel_with_budget(data_budget);
        self.sessions.insert(
            peer,
            WriteSession {
                central: peer.0[15],
                protocol: profile.protocol(),
                control_tx,
                data_tx,
            },
        );
        TestLink {
            peer,
            context: peer.0[15],
            protocol: profile.protocol(),
            identity: profile.peer_identity(),
            control,
            data,
        }
    }

    fn assert_no_new_link(&mut self) {
        assert!(matches!(
            self.links.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

fn make_link(
    request: &WriteRequest<u8>,
    profile: InboundProfile,
    control: mpsc::Receiver<Control>,
    data: GattInboundReceiver,
) -> TestLink {
    TestLink {
        peer: request.peer_id,
        context: request.central,
        protocol: profile.protocol(),
        identity: profile.peer_identity(),
        control,
        data,
    }
}

fn peer(value: u8) -> CoreBluetoothPeerId {
    let mut bytes = [0x5a; 16];
    bytes[15] = value;
    CoreBluetoothPeerId(bytes)
}

fn hello() -> Control {
    Control::Hello {
        identity: BleIdentity::new([0x11; 16]),
        endpoint: Endpoint::CoreBluetooth(AppleHost::Ios),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: 512,
        },
        peer_rssi: None,
    }
}

fn welcome() -> Control {
    Control::Welcome {
        identity: BleIdentity::new([0x22; 16]),
        endpoint: Endpoint::CoreBluetooth(AppleHost::MacOs),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: 512,
        },
        peer_rssi: Some(-47),
    }
}

fn request(peer_id: CoreBluetoothPeerId, target: WriteTarget, value: &[u8]) -> WriteRequest<u8> {
    WriteRequest {
        central: peer_id.0[15],
        peer_id,
        target,
        offset: 0,
        value: Some(Box::from(value)),
    }
}

fn control_request(peer_id: CoreBluetoothPeerId, control: Control) -> WriteRequest<u8> {
    let mut bytes = [0; CONTROL_MAX_LEN];
    let len = control.encode(&mut bytes).unwrap();
    request(peer_id, WriteTarget::Control, &bytes[..len])
}

async fn receive_data(receiver: &mut GattInboundReceiver) -> Box<[u8]> {
    tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .expect("committed data must already be queued")
        .expect("the session retains its data sender")
}

async fn assert_no_data(receiver: &mut GattInboundReceiver) {
    // Poll once: this detects an escaped prefix without relying on a timer or a sleeping reader.
    let mut receive = std::pin::pin!(receiver.recv());
    assert!(poll_fn(|cx| Poll::Ready(receive.as_mut().poll(cx)))
        .await
        .is_pending());
}

#[test]
fn response_seam_admits_once_and_answers_only_the_first_request() {
    let mut harness = Harness::new(1, 1);
    let identifiers = [41_u8, 42, 43];
    let mut admissions = 0;
    let mut replies = Vec::new();
    respond_to_write_batch(
        identifiers.first(),
        || {
            admissions += 1;
            harness.admit([
                Ok(control_request(peer(1), hello())),
                Ok(control_request(peer(1), welcome())),
                Ok(request(peer(1), WriteTarget::Data, &[7])),
            ])
        },
        |first, result| replies.push((*first, result)),
    );
    assert_eq!(admissions, 1);
    assert_eq!(replies, [(41, Ok(()))]);
    assert_eq!(harness.sessions.len(), 1);
    assert!(harness.links.try_recv().is_ok());
    harness.assert_no_new_link();
}

#[test]
fn empty_callback_neither_admits_nor_responds() {
    respond_to_write_batch::<u8>(
        None,
        || panic!("empty callback must not admit"),
        |_, _| panic!("empty callback has no request to answer"),
    );
}

#[test]
fn disabled_callback_rejects_once_without_evaluating_or_publishing_requests() {
    let mut harness = Harness::new(1, 1);
    let mut replies = Vec::new();
    respond_to_write_batch(
        Some(&17_u8),
        || {
            admit_write_batch(
                false,
                std::iter::once_with(|| panic!("disabled radio must not inspect values")),
                &mut harness.sessions,
                harness.capacity,
                &harness.inbound,
                make_link,
            )
        },
        |first, result| replies.push((*first, result)),
    );
    assert_eq!(replies, [(17, Err(WriteError::InsufficientResources))]);
    assert!(harness.sessions.is_empty());
    harness.assert_no_new_link();
}

#[test]
fn late_invalid_request_discards_every_staged_new_session_and_answers_first() {
    let mut invalid = control_request(peer(1), welcome());
    invalid.offset = 1;
    let mut harness = Harness::new(2, 2);
    let mut replies = Vec::new();
    respond_to_write_batch(
        Some(&31_u8),
        || {
            harness.admit([
                Ok(control_request(peer(1), hello())),
                Ok(control_request(peer(2), hello())),
                Ok(invalid),
            ])
        },
        |first, result| replies.push((*first, result)),
    );
    assert_eq!(replies, [(31, Err(WriteError::InvalidOffset))]);
    assert!(harness.sessions.is_empty());
    harness.assert_no_new_link();
    assert_eq!(harness.inbound.capacity(), 2);
    assert_eq!(
        harness.admit([Ok(control_request(peer(1), hello()))]),
        Ok(())
    );
}

#[tokio::test]
async fn late_decode_error_rolls_back_existing_control_and_data_reservations() {
    let mut harness = Harness::new(2, 2);
    let mut existing = harness.add_session(peer(1), InboundProfile::Native, 2, 5);
    let session = harness.sessions.get(&peer(1)).unwrap();
    session.control_tx.try_send(hello()).unwrap();
    session.data_tx.try_send(Box::from([9])).unwrap();
    assert_eq!(
        harness.admit([
            Ok(control_request(peer(1), welcome())),
            Ok(request(peer(1), WriteTarget::Data, &[1, 2, 3, 4])),
            Ok(control_request(peer(2), hello())),
            Err(WriteError::WriteNotPermitted),
        ]),
        Err(WriteError::WriteNotPermitted)
    );
    assert_eq!(harness.sessions.len(), 1);
    assert_eq!(harness.sessions.get(&peer(1)).unwrap().central, 1);
    assert_eq!(existing.control.try_recv(), Ok(hello()));
    assert_eq!(
        existing.control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );
    assert_eq!(receive_data(&mut existing.data).await.as_ref(), &[9]);
    assert_no_data(&mut existing.data).await;
    harness.assert_no_new_link();
    let session = harness.sessions.get(&peer(1)).unwrap();
    assert_eq!(session.control_tx.capacity(), 2);
    assert!(session.data_tx.try_reserve(Box::from([0; 5])).is_ok());
    assert_eq!(harness.inbound.capacity(), 2);
}

#[test]
fn inbound_pressure_and_session_capacity_do_not_publish_partial_peers() {
    // The first peer reserves the only ingress slot; the second refuses the entire batch.
    let mut harness = Harness::new(2, 1);
    assert_eq!(
        harness.admit([
            Ok(control_request(peer(1), hello())),
            Ok(control_request(peer(2), hello())),
        ]),
        Err(WriteError::InsufficientResources)
    );
    assert!(harness.sessions.is_empty());
    harness.assert_no_new_link();
    assert_eq!(harness.inbound.capacity(), 1);

    let mut harness = Harness::new(1, 2);
    let _existing = harness.add_session(peer(1), InboundProfile::Native, 8, 10);
    assert_eq!(
        harness.admit([Ok(control_request(peer(2), hello()))]),
        Err(WriteError::InsufficientResources)
    );
    assert_eq!(harness.sessions.len(), 1);
    assert!(harness.sessions.contains_key(&peer(1)));
    harness.assert_no_new_link();
    assert_eq!(harness.inbound.capacity(), 2);
}

#[test]
fn closed_inbound_receiver_does_not_insert_a_session() {
    let (inbound, receiver) = mpsc::channel::<TestLink>(1);
    drop(receiver);
    let mut sessions = HashMap::new();
    assert_eq!(
        admit_write_batch(
            true,
            [Ok(control_request(peer(1), hello()))],
            &mut sessions,
            1,
            &inbound,
            make_link,
        ),
        Err(WriteError::InsufficientResources)
    );
    assert!(sessions.is_empty());
}

#[test]
fn new_sessions_control_overflow_releases_its_inbound_slot_and_all_input() {
    let mut harness = Harness::new(1, 1);
    assert_eq!(
        harness.admit((0..9).map(|_| Ok(control_request(peer(1), hello())))),
        Err(WriteError::InsufficientResources)
    );
    assert!(harness.sessions.is_empty());
    harness.assert_no_new_link();
    assert_eq!(harness.inbound.capacity(), 1);
    assert_eq!(
        harness.admit([Ok(control_request(peer(1), hello()))]),
        Ok(())
    );
}

#[tokio::test]
async fn receiver_closure_after_reservation_does_not_partially_reject_admitted_input() {
    let mut harness = Harness::new(2, 1);
    let existing = harness.add_session(peer(1), InboundProfile::Native, 1, 2);
    let mut old_data = Some(existing.data);
    let mut old_control = existing.control;
    let prefix = [
        Ok(control_request(peer(1), hello())),
        Ok(request(peer(1), WriteTarget::Data, &[1, 2])),
    ];
    // This iterator boundary runs only after both old-owner reservations succeed.
    let requests = prefix.into_iter().chain(std::iter::once_with(|| {
        old_control.close();
        drop(old_data.take());
        Ok(control_request(peer(2), welcome()))
    }));
    assert_eq!(harness.admit(requests), Ok(()));
    assert_eq!(old_control.try_recv(), Ok(hello()));
    let mut new_link = harness.links.try_recv().unwrap();
    assert!(new_link.peer == peer(2));
    assert_eq!(new_link.control.try_recv(), Ok(welcome()));
    assert_no_data(&mut new_link.data).await;
    assert!(harness.sessions.get(&peer(1)).unwrap().data_tx.is_closed());
    assert_eq!(harness.sessions.get(&peer(1)).unwrap().central, 1);
    harness.assert_no_new_link();
}

#[tokio::test]
async fn full_control_queue_refuses_reserved_data_without_changing_old_input() {
    let mut harness = Harness::new(1, 1);
    let mut existing = harness.add_session(peer(1), InboundProfile::Native, 1, 2);
    harness
        .sessions
        .get(&peer(1))
        .unwrap()
        .control_tx
        .try_send(hello())
        .unwrap();
    assert_eq!(
        harness.admit([
            Ok(request(peer(1), WriteTarget::Data, &[1, 2])),
            Ok(control_request(peer(1), welcome())),
        ]),
        Err(WriteError::InsufficientResources)
    );
    assert_no_data(&mut existing.data).await;
    assert_eq!(existing.control.try_recv(), Ok(hello()));
    assert_eq!(
        existing.control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );
    assert!(harness
        .sessions
        .get(&peer(1))
        .unwrap()
        .data_tx
        .try_reserve(Box::from([0; 2]))
        .is_ok());
}

#[tokio::test]
async fn late_data_budget_failure_refunds_prior_data_and_control_reservations() {
    let mut harness = Harness::new(1, 1);
    let mut existing = harness.add_session(peer(1), InboundProfile::Native, 1, 2);
    assert_eq!(
        harness.admit([
            Ok(control_request(peer(1), hello())),
            Ok(request(peer(1), WriteTarget::Data, &[1, 2])),
            Ok(request(peer(1), WriteTarget::Data, &[3])),
        ]),
        Err(WriteError::InsufficientResources)
    );
    assert_no_data(&mut existing.data).await;
    assert_eq!(
        existing.control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );
    let session = harness.sessions.get(&peer(1)).unwrap();
    assert_eq!(session.control_tx.capacity(), 1);
    assert!(session.data_tx.try_reserve(Box::from([0; 2])).is_ok());
}

#[tokio::test]
async fn closed_control_or_data_receiver_refuses_the_other_lanes_prefix() {
    let mut harness = Harness::new(1, 1);
    let mut existing = harness.add_session(peer(1), InboundProfile::Native, 1, 2);
    existing.control.close();
    assert_eq!(
        harness.admit([
            Ok(request(peer(1), WriteTarget::Data, &[1, 2])),
            Ok(control_request(peer(1), hello())),
        ]),
        Err(WriteError::InsufficientResources)
    );
    assert_no_data(&mut existing.data).await;
    assert!(harness
        .sessions
        .get(&peer(1))
        .unwrap()
        .data_tx
        .try_reserve(Box::from([0; 2]))
        .is_ok());

    let mut harness = Harness::new(1, 1);
    let mut existing = harness.add_session(peer(1), InboundProfile::Native, 1, 2);
    drop(existing.data);
    assert_eq!(
        harness.admit([
            Ok(control_request(peer(1), hello())),
            Ok(request(peer(1), WriteTarget::Data, &[1])),
        ]),
        Err(WriteError::InsufficientResources)
    );
    assert_eq!(
        existing.control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );
    assert_eq!(
        harness
            .sessions
            .get(&peer(1))
            .unwrap()
            .control_tx
            .capacity(),
        1
    );
}

#[tokio::test]
async fn repeated_native_opening_and_mixed_input_publish_one_initialized_link() {
    let mut harness = Harness::new(1, 1);
    assert_eq!(
        harness.admit([
            Ok(control_request(peer(1), hello())),
            Ok(request(peer(1), WriteTarget::Data, &[1, 2])),
            Ok(control_request(peer(1), welcome())),
            Ok(request(peer(1), WriteTarget::Data, &[])),
            Ok(request(peer(1), WriteTarget::Data, &[3])),
        ]),
        Ok(())
    );
    assert_eq!(harness.sessions.len(), 1);
    let mut link = harness.links.try_recv().unwrap();
    assert!(link.peer == peer(1));
    assert_eq!(link.context, 1);
    assert_eq!(link.protocol, PeerProtocol::Native);
    assert_eq!(link.identity, None);
    assert_eq!(link.control.try_recv(), Ok(hello()));
    assert_eq!(link.control.try_recv(), Ok(welcome()));
    assert_eq!(receive_data(&mut link.data).await.as_ref(), &[1, 2]);
    assert!(receive_data(&mut link.data).await.is_empty());
    assert_eq!(receive_data(&mut link.data).await.as_ref(), &[3]);
    assert_no_data(&mut link.data).await;
    harness.assert_no_new_link();
}

#[tokio::test]
async fn columba_identity_is_consumed_once_and_following_data_keeps_order() {
    let mut harness = Harness::new(1, 1);
    assert_eq!(
        harness.admit([
            Ok(request(peer(1), WriteTarget::ColumbaRx, &[0x33; 16])),
            Ok(request(peer(1), WriteTarget::ColumbaRx, &[1, 2])),
            Ok(request(peer(1), WriteTarget::ColumbaRx, &[0x44; 16])),
        ]),
        Ok(())
    );
    let mut link = harness.links.try_recv().unwrap();
    assert_eq!(link.protocol, PeerProtocol::Columba);
    assert_eq!(link.identity, Some(BleIdentity::new([0x33; 16])));
    assert_eq!(
        link.control.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    );
    assert_eq!(receive_data(&mut link.data).await.as_ref(), &[1, 2]);
    assert_eq!(receive_data(&mut link.data).await.as_ref(), &[0x44; 16]);
    assert_no_data(&mut link.data).await;
    assert_eq!(harness.sessions.len(), 1);
    harness.assert_no_new_link();
}

#[test]
fn invalid_shapes_and_protocol_mismatches_never_open_or_replace_sessions() {
    let mut nil = control_request(peer(1), hello());
    nil.value = None;
    let mut offset = control_request(peer(1), hello());
    offset.offset = 1;
    for (request, expected) in [
        (nil, WriteError::InvalidValueLength),
        (offset, WriteError::InvalidOffset),
        (
            request(peer(1), WriteTarget::Control, &[]),
            WriteError::InvalidValueLength,
        ),
        (
            request(peer(1), WriteTarget::Control, &[0xff]),
            WriteError::InvalidValueLength,
        ),
        (
            request(peer(1), WriteTarget::ColumbaRx, &[1; 15]),
            WriteError::InvalidValueLength,
        ),
        (
            request(peer(1), WriteTarget::Data, &[1]),
            WriteError::WriteNotPermitted,
        ),
        (
            request(peer(1), WriteTarget::Unsupported, &[1]),
            WriteError::WriteNotPermitted,
        ),
    ] {
        let mut harness = Harness::new(1, 1);
        assert_eq!(harness.admit([Ok(request)]), Err(expected));
        assert!(harness.sessions.is_empty());
        harness.assert_no_new_link();
    }
    for (profile, target) in [
        (InboundProfile::Native, WriteTarget::ColumbaRx),
        (
            InboundProfile::Columba(BleIdentity::new([3; 16])),
            WriteTarget::Control,
        ),
        (
            InboundProfile::Columba(BleIdentity::new([3; 16])),
            WriteTarget::Data,
        ),
    ] {
        let mut harness = Harness::new(1, 1);
        let _existing = harness.add_session(peer(1), profile, 8, 32);
        let mut wrong = control_request(peer(1), hello());
        wrong.target = target;
        assert_eq!(
            harness.admit([Ok(wrong)]),
            Err(WriteError::WriteNotPermitted)
        );
        assert_eq!(harness.sessions.len(), 1);
        assert_eq!(
            harness.sessions.get(&peer(1)).unwrap().protocol,
            profile.protocol()
        );
        harness.assert_no_new_link();
    }
}

#[tokio::test]
async fn peers_with_colliding_synthetic_addresses_keep_exact_session_and_input_owners() {
    assert_eq!(peer(1).address(), peer(2).address());
    let mut harness = Harness::new(2, 2);
    assert_eq!(
        harness.admit([
            Ok(control_request(peer(1), hello())),
            Ok(control_request(peer(2), welcome())),
            Ok(request(peer(1), WriteTarget::Data, &[1])),
            Ok(request(peer(2), WriteTarget::Data, &[2])),
        ]),
        Ok(())
    );
    assert_eq!(harness.sessions.len(), 2);
    let mut first = harness.links.try_recv().unwrap();
    let mut second = harness.links.try_recv().unwrap();
    assert!(first.peer == peer(1));
    assert!(second.peer == peer(2));
    assert_eq!(first.context, 1);
    assert_eq!(second.context, 2);
    assert_eq!(first.control.try_recv(), Ok(hello()));
    assert_eq!(second.control.try_recv(), Ok(welcome()));
    assert_eq!(receive_data(&mut first.data).await.as_ref(), &[1]);
    assert_eq!(receive_data(&mut second.data).await.as_ref(), &[2]);
    assert_no_data(&mut first.data).await;
    assert_no_data(&mut second.data).await;
    harness.assert_no_new_link();
}
