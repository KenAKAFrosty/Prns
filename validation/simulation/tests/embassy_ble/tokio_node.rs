use std::cell::RefCell;
use std::rc::Rc;

use personal_rns::engine::LinkClosedReason;
use personal_rns::interfaces::bluetooth_auto::{
    BleIdentity, Endpoint, LinkCapabilities, BLE_HW_MTU,
};
use personal_rns::remote_control::RemoteControlService;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::{
    CryptoPoolConfig, Diagnostic, NoPersistence, NoRemoteControlHostControls,
    PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
};
use personal_rns::storage::{GrowableHeap, StorageLayout};
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::VirtualBleLab;
use tokio::sync::oneshot;

use super::clock::EmbassyTasks;
use super::echo::{self, Echo};
use super::fixture::{backend, MAX_PEERS};
use super::request_probe::RequestProbe;
use super::respond_probe::RespondProbe;
use super::response_trace::ResponseTrace;
use super::wire_gate::{GatedBackend, WireGate};

const CLOSURE_CAPACITY: usize = 4;

pub(super) struct TokioNode {
    pub handle: PrnsNodeHandle,
    pub responses: ResponseTrace,
    pub requests: RequestProbe,
    pub responded: RespondProbe,
    pub wire: WireGate,
    closed: Rc<RefCell<Vec<(LinkId, LinkClosedReason)>>>,
}

impl TokioNode {
    pub fn take_closed(&self) -> Vec<(LinkId, LinkClosedReason)> {
        let mut closed: Vec<_> = self.closed.borrow_mut().drain(..).collect();
        closed.sort_by_key(|(link, _)| *link.as_bytes());
        closed
    }
}

pub(super) fn start(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    address: u8,
    endpoint: Endpoint,
) -> oneshot::Receiver<TokioNode> {
    with_endpoints(
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
) -> oneshot::Receiver<TokioNode> {
    with_storage(
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
) -> oneshot::Receiver<TokioNode> {
    let wire = WireGate::new();
    let supervisor = BluetoothAuto::<_, MAX_PEERS>::new(
        GatedBackend::new(backend(lab, address), wire.clone()),
        BleIdentity::new([address; 16]),
        endpoint,
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
    );
    let (ready, handle) = oneshot::channel();
    let closed = Rc::new(RefCell::new(Vec::with_capacity(CLOSURE_CAPACITY)));
    let closed_events = closed.clone();
    let responses = ResponseTrace::new();
    let requests = RequestProbe::new();
    let responded = RespondProbe::new();
    let respond_events = responded.clone();
    let request_events = requests.clone();
    let response_events = responses.clone();
    tasks.insert(async move {
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: RemoteControlService::Unavailable.into(),
            pre_configured_destinations: destinations,
            app_state: NoRemoteControlHostControls,
            storage,
            request_endpoints: endpoints,
            interfaces: move |handle: &PrnsNodeHandle| {
                let _attached = handle.supervise(supervisor);
            },
            persistence: NoPersistence,
            on_event: move |event, _| {
                response_events.observe(&event);
                request_events.observe(&event);
                respond_events.observe(&event);
                if let PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, reason }) = event {
                    let mut events = closed_events.borrow_mut();
                    assert!(events.len() < CLOSURE_CAPACITY, "bounded closure inventory");
                    events.push((link_id, reason));
                }
            },
        })
        .with_crypto_pool(CryptoPoolConfig::Inline);
        assert!(ready
            .send(TokioNode {
                handle: node.handle(),
                responses,
                requests,
                responded,
                wire,
                closed
            })
            .is_ok());
        let result = node.run().await;
        unreachable!("Tokio node must remain live: {result:?}");
    });
    handle
}
