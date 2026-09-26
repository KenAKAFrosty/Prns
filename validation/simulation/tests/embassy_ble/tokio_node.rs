use std::cell::RefCell;
use std::rc::Rc;

use personal_rns::engine::LinkClosedReason;
use personal_rns::interfaces::bluetooth_auto::{
    BleIdentity, Endpoint, LinkCapabilities, BLE_HW_MTU,
};
use personal_rns::remote_control::RemoteControlService;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    CryptoPoolConfig, Diagnostic, NoPersistence, NoRemoteControlHostControls, PrnsEvent, PrnsNode,
    PrnsNodeHandle, PrnsNodeRecipe,
};
use personal_rns::storage::GrowableHeap;
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::VirtualBleLab;
use tokio::sync::oneshot;

use super::clock::EmbassyTasks;
use super::echo::{self, Echo};
use super::fixture::{backend, MAX_PEERS};

const CLOSURE_CAPACITY: usize = 4;

pub(super) struct TokioNode {
    pub handle: PrnsNodeHandle,
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
    let supervisor = BluetoothAuto::<_, MAX_PEERS>::new(
        backend(lab, address),
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
    tasks.insert(async move {
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: RemoteControlService::Unavailable,
            pre_configured_destinations: [echo::destination(address)],
            app_state: NoRemoteControlHostControls,
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![Echo],
            interfaces: move |handle: &PrnsNodeHandle| {
                let _attached = handle.supervise(supervisor);
            },
            persistence: NoPersistence,
            on_event: move |event, _| {
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
                closed
            })
            .is_ok());
        let result = node.run().await;
        unreachable!("Tokio node must remain live: {result:?}");
    });
    handle
}
