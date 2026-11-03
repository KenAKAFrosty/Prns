#![allow(clippy::unwrap_used)]

use core::future::{poll_fn, Future};
use core::pin::{pin, Pin};
use core::task::{Context, Poll, Waker};
use std::{boxed::Box, collections::VecDeque, rc::Rc, vec::Vec};

use embassy_sync::channel::Channel;
use prns_core::interfaces::bluetooth_auto::PeerDiscoveryGroups;
use prns_core::interfaces::DiscoveryGroupId;
use prns_runtime::manifold::driver::InterfaceLifecycle;
use prns_runtime::runtime::{ManifoldLaneSet, StaticManifoldLane};

use super::*;

const MEMBERS: usize = 3;
const FRAME: usize = contract::BLE_HW_MTU;
type TestFleet = Fleet<CriticalSectionRawMutex, FRAME, MEMBERS, 16>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    RadioRequested(RadioMode),
    RadioApplied(RadioMode),
    Capabilities,
    Advertising(AdvertisingMode),
    Scanning(ScanningMode),
    Closed(BleAddress),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Failure {
    None,
    RadioOn,
    Capabilities,
    Advertising,
}

struct State {
    events: RefCell<VecDeque<BleEvent<Link>>>,
    operations: RefCell<Vec<Operation>>,
    controls: RefCell<Vec<(BleAddress, Control)>>,
    dropped_links: RefCell<Vec<BleAddress>>,
    radio_on: Cell<bool>,
    hold_radio_on: Cell<bool>,
    hold_advertising: Cell<bool>,
    failure: Cell<Failure>,
    capabilities: Cell<LinkCapabilities>,
}

impl State {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            events: RefCell::new(VecDeque::new()),
            operations: RefCell::new(Vec::new()),
            controls: RefCell::new(Vec::new()),
            dropped_links: RefCell::new(Vec::new()),
            radio_on: Cell::new(false),
            hold_radio_on: Cell::new(false),
            hold_advertising: Cell::new(false),
            failure: Cell::new(Failure::None),
            capabilities: Cell::new(CAPS),
        })
    }

    fn incoming(self: &Rc<Self>, address: u8, groups: Option<DiscoveryGroupSet>) {
        self.events.borrow_mut().push_back(BleEvent::Inbound(Link {
            state: self.clone(),
            address: BleAddress::new([address; 6]),
            incoming: groups.map(|groups| Control::Hello {
                identity: BleIdentity::new([address; 16]),
                endpoint: Endpoint::Nrf52(Nrf52Host::Nrf52),
                capabilities: CAPS,
                peer_rssi: None,
                discovery_groups: PeerDiscoveryGroups::Explicit(groups.hashes()),
            }),
        }));
    }
}

struct Backend(Rc<State>);

impl BleBackend<MEMBERS> for Backend {
    type Error = MockError;
    type Link = Link;

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), MockError> {
        self.0
            .operations
            .borrow_mut()
            .push(Operation::RadioRequested(mode));
        if mode.is_on() {
            wait_for_release(&self.0.hold_radio_on).await;
            if self.0.failure.get() == Failure::RadioOn {
                return Err(MockError);
            }
        }
        self.0.radio_on.set(mode.is_on());
        self.0
            .operations
            .borrow_mut()
            .push(Operation::RadioApplied(mode));
        Ok(())
    }

    async fn local_capabilities(
        &mut self,
        configured: LinkCapabilities,
    ) -> Result<LinkCapabilities, MockError> {
        assert_eq!(configured, CAPS);
        assert!(self.0.radio_on.get());
        self.0.operations.borrow_mut().push(Operation::Capabilities);
        if self.0.failure.get() == Failure::Capabilities {
            Err(MockError)
        } else {
            Ok(self.0.capabilities.get())
        }
    }

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), MockError> {
        if mode.is_on() {
            wait_for_release(&self.0.hold_advertising).await;
            if self.0.failure.get() == Failure::Advertising {
                return Err(MockError);
            }
        }
        self.0
            .operations
            .borrow_mut()
            .push(Operation::Advertising(mode));
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), MockError> {
        self.0
            .operations
            .borrow_mut()
            .push(Operation::Scanning(mode));
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<Link> {
        poll_fn(|_| {
            // Match the embedded backend: an off radio cannot supply a usable new link.
            if self.0.radio_on.get() {
                self.0
                    .events
                    .borrow_mut()
                    .pop_front()
                    .map_or(Poll::Pending, Poll::Ready)
            } else {
                Poll::Pending
            }
        })
        .await
    }

    async fn dial(&mut self, _address: BleAddress) -> DialOutcome {
        DialOutcome::UnknownPeer
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        self.0
            .operations
            .borrow_mut()
            .push(Operation::Closed(address));
    }
}

struct Link {
    state: Rc<State>,
    address: BleAddress,
    incoming: Option<Control>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.state.dropped_links.borrow_mut().push(self.address);
    }
}

impl BleLink for Link {
    type Error = MockError;
    type Source = MockSource;
    type Sink = MockSink;
    type Control = contract::NoBleControl<MockError>;

    fn peer_protocol(&self) -> PeerProtocol {
        PeerProtocol::Native
    }
    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, message: &Control) -> Result<(), MockError> {
        self.state
            .controls
            .borrow_mut()
            .push((self.address, *message));
        Ok(())
    }

    async fn control_recv(&mut self) -> Result<Control, MockError> {
        match self.incoming.take() {
            Some(message) => Ok(message),
            None => core::future::pending().await,
        }
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), MockError> {
        Ok(())
    }

    fn into_parts(self) -> BleLinkParts<MockSource, MockSink, Self::Control> {
        BleLinkParts {
            source: MockSource::Pending,
            sink: MockSink,
            control: None,
        }
    }
}

async fn wait_for_release(hold: &Cell<bool>) {
    poll_fn(|_| {
        if hold.get() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await
}

fn poll<T>(future: Pin<&mut impl Future<Output = T>>) -> Poll<T> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn group(name: &str) -> DiscoveryGroupSet {
    DiscoveryGroupSet::singleton(DiscoveryGroupId::parse(name).unwrap()).unwrap()
}

fn fixture() -> (TestFleet, &'static BluetoothAutoShared<MEMBERS>, Rc<State>) {
    let id = InterfaceId::new([0x73; 8]);
    let lane = Box::leak(Box::new(StaticManifoldLane::<
        CriticalSectionRawMutex,
        FRAME,
        MEMBERS,
    >::new()));
    let wake = Box::leak(Box::new(Signal::new()));
    let notify = Box::leak(Box::new(Channel::new()));
    let lifecycle = Box::leak(Box::new(Channel::<
        CriticalSectionRawMutex,
        InterfaceLifecycle,
        16,
    >::new()));
    let mut lanes = ManifoldLaneSet::<CriticalSectionRawMutex, 1, MEMBERS>::new();
    let fleet = lanes
        .claim_supervisor(lane, id, wake)
        .unwrap()
        .into_fleet(notify.sender(), lifecycle.sender());
    let shared = Box::leak(Box::new(BluetoothAutoShared::new(id)));
    (fleet, shared, State::new())
}

fn supervisor(
    state: &Rc<State>,
    shared: &'static BluetoothAutoShared<MEMBERS>,
) -> BluetoothAuto<Backend, MEMBERS> {
    BluetoothAuto::new(
        Backend(state.clone()),
        BleIdentity::new([1; 16]),
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        CAPS,
        shared,
    )
}

#[test]
fn supervisor_replacement_restarts_radio_before_ack_and_uses_refreshed_handshake_policy() {
    let (fleet, shared, state) = fixture();
    let supervisor = supervisor(&state, shared);
    let status = supervisor.status();
    let mut run = pin!(supervisor.run(fleet));
    assert!(poll(run.as_mut()).is_pending());
    state.incoming(2, Some(DiscoveryGroupSet::reticulum()));
    state.incoming(3, None);
    assert!(poll(run.as_mut()).is_pending());
    assert_eq!(status.members().count(), 1);
    state.operations.borrow_mut().clear();
    state.dropped_links.borrow_mut().clear();

    let desired = group("isolated");
    let refreshed = LinkCapabilities {
        link_mtu: 384,
        ..CAPS
    };
    state.capabilities.set(refreshed);
    state.hold_radio_on.set(true);
    state.hold_advertising.set(true);
    let mut replace = pin!(status.replace_discovery_groups(&desired));
    assert!(poll(replace.as_mut()).is_pending());
    assert!(poll(run.as_mut()).is_pending());
    assert_eq!(status.members().count(), 0);
    assert!(state
        .dropped_links
        .borrow()
        .contains(&BleAddress::new([3; 6])));
    assert!(state
        .operations
        .borrow()
        .contains(&Operation::Closed(BleAddress::new([2; 6]))));
    assert!(!state.radio_on.get());
    assert!(
        state
            .operations
            .borrow()
            .contains(&Operation::RadioRequested(RadioMode::On)),
        "a replacement must prepare the radio again after disabling old members"
    );
    assert!(poll(replace.as_mut()).is_pending());

    state.hold_radio_on.set(false);
    assert!(poll(run.as_mut()).is_pending());
    assert!(state.radio_on.get());
    assert!(state.operations.borrow().contains(&Operation::Capabilities));
    assert!(
        poll(replace.as_mut()).is_pending(),
        "radio-on alone is not an applied policy"
    );
    state.hold_advertising.set(false);
    assert!(poll(run.as_mut()).is_pending());
    assert_eq!(
        poll(replace.as_mut()),
        Poll::Ready(DiscoveryGroupApplyOutcome::Applied)
    );
    let operations = state.operations.borrow();
    let radio_on = operations
        .iter()
        .position(|op| *op == Operation::RadioApplied(RadioMode::On))
        .unwrap();
    let capabilities = operations
        .iter()
        .position(|op| *op == Operation::Capabilities)
        .unwrap();
    let advertising = operations
        .iter()
        .position(|op| *op == Operation::Advertising(AdvertisingMode::On))
        .unwrap();
    assert!(radio_on < capabilities && capabilities < advertising);
    drop(operations);

    state.controls.borrow_mut().clear();
    state.incoming(4, Some(DiscoveryGroupSet::reticulum()));
    state.incoming(5, Some(desired));
    assert!(poll(run.as_mut()).is_pending());
    assert_eq!(status.members().count(), 1);
    let controls = state.controls.borrow();
    assert!(controls.contains(&(
        BleAddress::new([4; 6]),
        Control::Close {
            reason: CloseReason::Incompatible
        }
    )));
    assert!(controls.iter().any(|(address, control)| *address == BleAddress::new([5; 6]) && matches!(control,
        Control::Welcome { capabilities, discovery_groups: PeerDiscoveryGroups::Explicit(groups), .. }
            if *capabilities == refreshed && *groups == desired.hashes())));
}

#[test]
fn supervisor_replacement_failure_never_acknowledges_applied_and_leaves_radio_off() {
    for failure in [
        Failure::RadioOn,
        Failure::Capabilities,
        Failure::Advertising,
    ] {
        let (fleet, shared, state) = fixture();
        let supervisor = supervisor(&state, shared);
        let status = supervisor.status();
        let mut run = pin!(supervisor.run(fleet));
        assert!(poll(run.as_mut()).is_pending());
        state.operations.borrow_mut().clear();
        state.failure.set(failure);
        let desired = group("isolated");
        let mut replace = pin!(status.replace_discovery_groups(&desired));
        assert!(poll(replace.as_mut()).is_pending());
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(
            poll(replace.as_mut()),
            Poll::Ready(DiscoveryGroupApplyOutcome::Failed)
        );
        assert!(status.is_failed());
        assert_eq!(status.failure_reason(), Some(RADIO_CONTROL_REASON));
        assert!(!state.radio_on.get());
        assert_eq!(status.members().count(), 0);
        assert!(!state
            .operations
            .borrow()
            .contains(&Operation::Advertising(AdvertisingMode::On)));
    }
}

#[test]
fn supervisor_startup_uses_constructor_default_only_when_no_groups_were_restored() {
    for explicit in [true, false] {
        let (fleet, shared, state) = fixture();
        let expected = if explicit {
            group("configured")
        } else {
            DiscoveryGroupSet::reticulum()
        };
        let supervisor = if explicit {
            BluetoothAuto::new_with_discovery_groups(
                Backend(state.clone()),
                BleIdentity::new([1; 16]),
                Endpoint::Nrf52(Nrf52Host::Nrf52),
                CAPS,
                shared,
                expected,
            )
        } else {
            supervisor(&state, shared)
        };
        let status = supervisor.status();
        let mut run = pin!(supervisor.run(fleet));
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(status.discovery_groups(), expected);
        state.incoming(2, Some(expected));
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(status.members().count(), 1);
    }
}

#[test]
fn supervisor_startup_preserves_restored_groups_before_or_after_default_or_explicit_constructor() {
    for restore_before in [true, false] {
        for explicit in [true, false] {
            let (fleet, shared, state) = fixture();
            let status = BluetoothAutoStatus::new(shared);
            let saved = group("saved");
            if restore_before {
                assert!(status.restore_discovery_groups_before_start(saved));
            }
            let supervisor = if explicit {
                BluetoothAuto::new_with_discovery_groups(
                    Backend(state.clone()),
                    BleIdentity::new([1; 16]),
                    Endpoint::Nrf52(Nrf52Host::Nrf52),
                    CAPS,
                    shared,
                    group("configured"),
                )
            } else {
                supervisor(&state, shared)
            };
            if !restore_before {
                assert!(status.restore_discovery_groups_before_start(saved));
            }
            let mut run = pin!(supervisor.run(fleet));
            assert!(poll(run.as_mut()).is_pending());
            assert_eq!(status.discovery_groups(), saved);
            assert!(!status.restore_discovery_groups_before_start(group("late")));
            state.incoming(2, Some(DiscoveryGroupSet::reticulum()));
            state.incoming(3, Some(saved));
            assert!(poll(run.as_mut()).is_pending());
            assert_eq!(status.members().count(), 1);
            let controls = state.controls.borrow();
            assert!(controls.contains(&(
                BleAddress::new([2; 6]),
                Control::Close {
                    reason: CloseReason::Incompatible
                }
            )));
            assert!(controls.iter().any(|(address, control)| *address == BleAddress::new([3; 6]) && matches!(control,
                Control::Welcome { discovery_groups: PeerDiscoveryGroups::Explicit(groups), .. } if *groups == saved.hashes())));
        }
    }
}
