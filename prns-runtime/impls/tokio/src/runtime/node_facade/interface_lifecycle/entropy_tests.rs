#![allow(clippy::unwrap_used)]

use super::*;
use crate::manifold::interface_seam::InterfaceSeam;
use crate::runtime::TokioEntropy;

type Observations = Arc<Mutex<Vec<[u8; 64]>>>;

struct Probe {
    interface: tests::StatusInterface,
    observations: Observations,
}

impl ReportsStatus for Probe {}

impl Interface for Probe {
    const KIND: InterfaceKind = InterfaceKind::Pipe;
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;

    fn channel_tag(&self) -> &[u8] {
        self.interface.channel_tag()
    }
    fn descriptor(&self) -> crate::interfaces::InterfaceDescriptor {
        self.interface.descriptor()
    }
    async fn run<S: InterfaceSeam>(self, mut seam: S) {
        let mut bytes = [0; 64];
        seam.fill_random(&mut bytes);
        self.observations.lock().unwrap().push(bytes);
    }
}

struct Supervisor(Observations);
impl ReportsStatus for Supervisor {}
impl InterfaceSupervisor for Supervisor {
    const KIND: InterfaceKind = InterfaceKind::Pipe;
    fn channel_tag(&self) -> &[u8] {
        b"entropy-supervisor"
    }
    fn policy(&self) -> crate::interfaces::EffectiveInterfacePolicy {
        let descriptor = tests::StatusInterface::new(b"policy").descriptor();
        crate::interfaces::EffectiveInterfacePolicy {
            capabilities: descriptor.capabilities,
            mode: descriptor.mode,
            gravity: descriptor.gravity,
            bitrate: descriptor.bitrate,
            mtu: crate::interfaces::MtuPolicy::fixed(crate::wire::BROADCAST_MTU),
            announce_rate_limit: descriptor.announce_rate_limit,
            announce_bandwidth_cap: descriptor.announce_bandwidth_cap,
            airtime_duty_cycle: descriptor.airtime_duty_cycle,
            common: descriptor.common,
        }
    }
    async fn run(self, fleet: Fleet) {
        let mut bytes = [0; 64];
        fleet.fill_random(&mut bytes);
        self.0.lock().unwrap().push(bytes);
        fleet.add(Probe {
            interface: tests::StatusInterface::new(b"member"),
            observations: self.0,
        });
    }
}

async fn run_next(receiver: &mut UnboundedReceiver<DriverMsg>) {
    let DriverMsg::Add { build, .. } = receiver.try_recv().unwrap() else {
        unreachable!("queued attachment")
    };
    build().await;
}

#[tokio::test]
async fn handles_supervisors_and_interface_seams_share_only_their_node_entropy() {
    use crate::runtime::{
        ManuallyAttached, NoPersistence, NoRemoteControlHostControls, PreConfiguredDestination,
        PrnsNode, PrnsNodeRecipe,
    };
    let observations = Arc::new(Mutex::new(Vec::new()));
    let node = PrnsNode::new_with_entropy_sources(
        |handle| {
            let mut first = [0; 64];
            handle.fill_random(&mut first);
            observations.lock().unwrap().push(first);
            PrnsNodeRecipe {
                transport_identity: None,
                remote_control: crate::remote_control::RemoteControlService::Unavailable.into(),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: NoRemoteControlHostControls,
                storage: crate::storage::GrowableHeap,
                request_endpoints: crate::request_endpoints![],
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: |_event, _state: &NoRemoteControlHostControls| {},
            }
        },
        crate::manifold::driver::TokioHost::new(),
        TokioEntropy::from_test_seed(0x57),
    );
    let mut handle = node.handle();
    let (builds, mut receiver) = mpsc::unbounded_channel();
    handle.iface_build = builds;
    let cloned = handle.clone();
    let mut first = [0; 64];
    cloned.fill_random(&mut first);
    observations.lock().unwrap().push(first);
    handle.add_interface(Probe {
        interface: tests::StatusInterface::new(b"direct"),
        observations: Arc::clone(&observations),
    });
    run_next(&mut receiver).await;
    handle.supervise(Supervisor(Arc::clone(&observations)));
    run_next(&mut receiver).await;
    run_next(&mut receiver).await;
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    let (other_commands, _other_rx) = mpsc::unbounded_channel();
    let mut other = PrnsNodeHandle::over(other_commands);
    other.entropy = TokioEntropy::from_test_seed(0x57);
    let mut independent = [0; 64];
    other.fill_random(&mut independent);
    assert_eq!(independent, observations.lock().unwrap()[0]);

    let expected_owner = TokioEntropy::from_test_seed(0x57);
    let mut expected = vec![[0; 64]; 5];
    for bytes in &mut expected {
        expected_owner.fill(bytes);
    }
    assert_eq!(*observations.lock().unwrap(), expected);
}
