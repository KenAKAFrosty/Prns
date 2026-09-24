use std::cell::RefCell;
use std::convert::Infallible;
use std::rc::Rc;

use embassy_sync::channel::Channel;
use personal_rns::engine::{
    CommandId, EgressTarget, InstantMillis, IssuedCommand, LinkClosedReason, PrnsCommand,
    SendPlainPacket, SendPlainPacketPayload, Settlement, MAX_SEND_REQUEST_DATA_LEN,
};
use personal_rns::interfaces::bluetooth_auto::Endpoint;
use personal_rns::interfaces::InterfaceId;
use personal_rns::remote_control::RemoteControlService;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, Message, NoPersistence, NoRemoteControlHostControls,
    PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe,
};
use personal_rns::storage::{GrowableHeap, StorageLayout};
use personal_rns::wire::{DestinationHash, WireContext};
use prns_core::entropy::EntropySource;
use prns_interfaces_embassy::bluetooth_auto::BluetoothAutoStatus;
use prns_runtime_embassy::manifold::driver::EmbassyHost;
use prns_runtime_embassy::runtime::{
    CompletionPool, PrnsNode, PrnsNodeHandle, RequestRoutingCapacity, SharedRuntimeEntropy,
};
use prns_simulation::ble::VirtualBleLab;

use super::clock::EmbassyTasks;
use super::echo::{self, Echo};
use super::fixture::{RadioFixture, RawMutex, MAX_PEERS};
use super::request_probe::RequestProbe;
use super::respond_probe::RespondProbe;
use super::response_trace::ResponseTrace;
use super::wire_gate::WireGate;

const COMMAND_CAPACITY: usize = 4;
const EVENT_CAPACITY: usize = 8;
pub(super) const REQUEST_CAPACITY: usize = 2;
pub(super) const PAYLOAD_BYTES: usize = 256;
pub(super) type Handle<const RESPONSE_BYTES: usize> = PrnsNodeHandle<
    'static,
    RawMutex,
    COMMAND_CAPACITY,
    COMMAND_CAPACITY,
    REQUEST_CAPACITY,
    RESPONSE_BYTES,
>;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Received {
    pub destination: DestinationHash,
    pub source: InterfaceId,
    pub context: WireContext,
    pub arrived_at: InstantMillis,
    pub payload: Vec<u8>,
}

// This deterministic source is confined to this non-shipping test binary.
struct TestEntropy(u8);

impl EntropySource for TestEntropy {
    type Error = Infallible;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        output.fill(self.0);
        self.0 = self.0.wrapping_add(1);
        Ok(())
    }
}

pub(super) fn destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Plain {
        app_name: "simulation",
        aspects: &["embassy-ble"],
    }
}

pub(super) type Node = NodeFixture<PAYLOAD_BYTES, MAX_SEND_REQUEST_DATA_LEN>;

pub(super) struct NodeInputs {
    pub address: u8,
    pub endpoint: Endpoint,
    pub entropy_seed: u8,
}

pub(super) struct NodeFixture<const RESPONSE_BYTES: usize, const REQUEST_BYTES: usize> {
    pub handle: Handle<RESPONSE_BYTES>,
    pub status: BluetoothAutoStatus<MAX_PEERS>,
    pub responses: ResponseTrace,
    pub requests: RequestProbe,
    pub responded: RespondProbe,
    pub wire: WireGate,
    received: Rc<RefCell<Vec<Received>>>,
    settled: Rc<RefCell<Vec<(CommandId, Settlement)>>>,
    closed: Rc<RefCell<Vec<(LinkId, LinkClosedReason)>>>,
    task: prns_simulation::ManualTaskId,
}

impl<const RESPONSE_BYTES: usize, const REQUEST_BYTES: usize>
    NodeFixture<RESPONSE_BYTES, REQUEST_BYTES>
{
    pub fn start(
        tasks: &mut EmbassyTasks<'_>,
        lab: &VirtualBleLab,
        address: u8,
        endpoint: Endpoint,
    ) -> Self {
        Self::with_endpoints(tasks, lab, address, endpoint, [destination()], ())
    }

    pub fn start_echo(
        tasks: &mut EmbassyTasks<'_>,
        lab: &VirtualBleLab,
        address: u8,
        endpoint: Endpoint,
    ) -> Self {
        Self::with_endpoints(
            tasks,
            lab,
            address,
            endpoint,
            [echo::destination(address)],
            personal_rns::request_endpoints![Echo],
        )
    }

    pub(super) fn with_endpoints<R: RequestEndpointSet<NoRemoteControlHostControls> + 'static>(
        tasks: &mut EmbassyTasks<'_>,
        lab: &VirtualBleLab,
        address: u8,
        endpoint: Endpoint,
        destinations: [PreConfiguredDestination<'static>; 1],
        endpoints: R,
    ) -> Self {
        Self::with_storage(
            tasks,
            lab,
            address,
            endpoint,
            destinations,
            endpoints,
            GrowableHeap,
        )
    }

    pub(super) fn with_storage<
        R: RequestEndpointSet<NoRemoteControlHostControls> + 'static,
        S: StorageLayout + 'static,
    >(
        tasks: &mut EmbassyTasks<'_>,
        lab: &VirtualBleLab,
        address: u8,
        endpoint: Endpoint,
        destinations: [PreConfiguredDestination<'static>; 1],
        endpoints: R,
        storage: S,
    ) -> Self {
        Self::with_storage_inputs(
            tasks,
            lab,
            NodeInputs {
                address,
                endpoint,
                entropy_seed: address,
            },
            destinations,
            endpoints,
            storage,
        )
    }

    pub(super) fn with_storage_inputs<
        R: RequestEndpointSet<NoRemoteControlHostControls> + 'static,
        S: StorageLayout + 'static,
    >(
        tasks: &mut EmbassyTasks<'_>,
        lab: &VirtualBleLab,
        inputs: NodeInputs,
        destinations: [PreConfiguredDestination<'static>; 1],
        endpoints: R,
        storage: S,
    ) -> Self {
        let NodeInputs {
            address,
            endpoint,
            entropy_seed,
        } = inputs;
        let RadioFixture {
            supervisor,
            fleet,
            lanes,
            notify,
            lifecycle,
            wire,
        } = RadioFixture::new(lab, address, endpoint);
        let status = supervisor.status();
        let commands =
            super::static_storage::allocate(
                Channel::<RawMutex, IssuedCommand, COMMAND_CAPACITY>::new(),
            );
        let completions = super::static_storage::allocate(CompletionPool::<
            RawMutex,
            COMMAND_CAPACITY,
            REQUEST_CAPACITY,
            RESPONSE_BYTES,
        >::new());
        let handle = Handle::new(commands.sender(), completions);
        let wiring = lanes.into_manifold_wiring(
            notify.receiver(),
            commands.receiver(),
            lifecycle.receiver(),
            handle,
        );
        let received = Rc::new(RefCell::new(Vec::with_capacity(EVENT_CAPACITY)));
        let settled = Rc::new(RefCell::new(Vec::with_capacity(EVENT_CAPACITY)));
        let closed = Rc::new(RefCell::new(Vec::with_capacity(EVENT_CAPACITY)));
        let received_events = received.clone();
        let settled_events = settled.clone();
        let closed_events = closed.clone();
        let responses = ResponseTrace::new();
        let requests = RequestProbe::new();
        let responded = RespondProbe::new();
        let respond_events = responded.clone();
        let request_events = requests.clone();
        let response_events = responses.clone();
        let entropy = super::static_storage::allocate(
            SharedRuntimeEntropy::<RawMutex, _>::try_new(TestEntropy(entropy_seed)).unwrap(),
        );
        let recipe = PrnsNodeRecipe {
            transport_identity: None,
            remote_control: RemoteControlService::Unavailable.into(),
            pre_configured_destinations: destinations,
            app_state: NoRemoteControlHostControls,
            storage,
            request_endpoints: endpoints,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: move |event, _: &NoRemoteControlHostControls| {
                response_events.observe(&event);
                request_events.observe(&event);
                respond_events.observe(&event);
                match event {
                    PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, reason }) => {
                        let mut events = closed_events.borrow_mut();
                        assert!(events.len() < EVENT_CAPACITY, "bounded closure inventory");
                        events.push((link_id, reason));
                    }
                    PrnsEvent::Message(Message::Delivered(Delivery::Plain(delivery))) => {
                        let mut events = received_events.borrow_mut();
                        assert!(events.len() < EVENT_CAPACITY, "bounded delivery inventory");
                        events.push(Received {
                            destination: delivery.destination,
                            source: delivery.source_interface,
                            context: delivery.context,
                            arrived_at: delivery.arrived_at,
                            payload: delivery.payload.to_vec(),
                        });
                    }
                    PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                        let mut events = settled_events.borrow_mut();
                        assert!(
                            events.len() < EVENT_CAPACITY,
                            "bounded settlement inventory"
                        );
                        events.push((id, settlement));
                    }
                    _ => {}
                }
            },
        };
        let node: PrnsNode<
            _,
            _,
            _,
            _,
            _,
            _,
            1,
            MAX_PEERS,
            2,
            COMMAND_CAPACITY,
            4,
            COMMAND_CAPACITY,
            4,
            REQUEST_BYTES,
            REQUEST_CAPACITY,
            RESPONSE_BYTES,
        > = PrnsNode::new_with_request_capacity(
            recipe,
            wiring,
            EmbassyHost::new(entropy.handle()),
            RequestRoutingCapacity::new(),
        );
        let task = tasks.insert(node.run(supervisor.run(fleet)));
        Self {
            task,
            handle,
            status,
            responses,
            requests,
            responded,
            wire,
            received,
            settled,
            closed,
        }
    }

    pub fn send(&self, target: EgressTarget, payload: &[u8]) -> CommandId {
        self.handle
            .issue(PrnsCommand::SendPlainPacket(SendPlainPacket {
                destination: destination().destination_hash().unwrap(),
                target,
                payload: SendPlainPacketPayload::from_slice(payload).unwrap(),
            }))
            .unwrap()
    }

    pub(super) fn stop(self, tasks: &mut EmbassyTasks<'_>) {
        assert_eq!(
            tasks.cancel(self.task),
            prns_simulation::ManualTaskCancellation::Cancelled
        );
        assert_eq!(
            tasks.cancel(self.task),
            prns_simulation::ManualTaskCancellation::NotLive
        );
    }

    pub fn take_received(&self) -> Vec<Received> {
        let mut received: Vec<_> = self.received.borrow_mut().drain(..).collect();
        received.sort_by_key(|event| event.source);
        received
    }

    pub fn take_settled(&self) -> Vec<(CommandId, Settlement)> {
        self.settled.borrow_mut().drain(..).collect()
    }

    pub fn take_closed(&self) -> Vec<(LinkId, LinkClosedReason)> {
        let mut closed: Vec<_> = self.closed.borrow_mut().drain(..).collect();
        closed.sort_by_key(|(link, _)| *link.as_bytes());
        closed
    }
}
