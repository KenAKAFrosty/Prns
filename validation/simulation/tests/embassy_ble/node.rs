use std::cell::RefCell;
use std::convert::Infallible;
use std::rc::Rc;

use embassy_sync::channel::Channel;
use personal_rns::engine::{
    CommandId, EgressTarget, InstantMillis, IssuedCommand, PrnsCommand, SendPlainPacket,
    SendPlainPacketPayload, Settlement,
};
use personal_rns::interfaces::bluetooth_auto::Endpoint;
use personal_rns::interfaces::InterfaceId;
use personal_rns::remote_control::RemoteControlService;
use personal_rns::routing::delivery::Delivery;
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, Message, NoPersistence, NoRemoteControlHostControls,
    PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::{DestinationHash, WireContext};
use prns_core::entropy::EntropySource;
use prns_interfaces_embassy::bluetooth_auto::BluetoothAutoStatus;
use prns_runtime_embassy::manifold::driver::EmbassyHost;
use prns_runtime_embassy::runtime::{
    CompletionPool, PrnsNode, PrnsNodeHandle, SharedRuntimeEntropy,
};
use prns_simulation::ble::VirtualBleLab;

use super::clock::EmbassyTasks;
use super::fixture::{RadioFixture, RawMutex, MAX_PEERS};

const COMMAND_CAPACITY: usize = 4;
const EVENT_CAPACITY: usize = 8;
pub(super) const PAYLOAD_BYTES: usize = 256;
type Handle = PrnsNodeHandle<'static, RawMutex, COMMAND_CAPACITY, COMMAND_CAPACITY>;

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

pub(super) struct Node {
    handle: Handle,
    pub status: BluetoothAutoStatus<MAX_PEERS>,
    received: Rc<RefCell<Vec<Received>>>,
    settled: Rc<RefCell<Vec<(CommandId, Settlement)>>>,
}

impl Node {
    pub fn start(
        tasks: &mut EmbassyTasks<'_>,
        lab: &VirtualBleLab,
        address: u8,
        endpoint: Endpoint,
    ) -> Self {
        let RadioFixture {
            supervisor,
            fleet,
            lanes,
            notify,
            lifecycle,
        } = RadioFixture::new(lab, address, endpoint);
        let status = supervisor.status();
        let commands = Box::leak(Box::new(
            Channel::<RawMutex, IssuedCommand, COMMAND_CAPACITY>::new(),
        ));
        let completions = Box::leak(Box::new(CompletionPool::<RawMutex, COMMAND_CAPACITY>::new()));
        let handle = Handle::new(commands.sender(), completions);
        let wiring = lanes.into_manifold_wiring(
            notify.receiver(),
            commands.receiver(),
            lifecycle.receiver(),
            handle,
        );
        let received = Rc::new(RefCell::new(Vec::with_capacity(EVENT_CAPACITY)));
        let settled = Rc::new(RefCell::new(Vec::with_capacity(EVENT_CAPACITY)));
        let received_events = received.clone();
        let settled_events = settled.clone();
        let entropy = Box::leak(Box::new(
            SharedRuntimeEntropy::<RawMutex, _>::try_new(TestEntropy(address)).unwrap(),
        ));
        let recipe = PrnsNodeRecipe {
            transport_identity: None,
            remote_control: RemoteControlService::Unavailable,
            pre_configured_destinations: [destination()],
            app_state: NoRemoteControlHostControls,
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: move |event, _: &NoRemoteControlHostControls| match event {
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
        > = PrnsNode::new(recipe, wiring, EmbassyHost::new(entropy.handle()));
        tasks.insert(node.run(supervisor.run(fleet)));
        Self {
            handle,
            status,
            received,
            settled,
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

    pub fn take_received(&self) -> Vec<Received> {
        let mut received: Vec<_> = self.received.borrow_mut().drain(..).collect();
        received.sort_by_key(|event| event.source);
        received
    }

    pub fn take_settled(&self) -> Vec<(CommandId, Settlement)> {
        self.settled.borrow_mut().drain(..).collect()
    }
}
