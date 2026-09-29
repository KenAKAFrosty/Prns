use std::sync::{Arc, Mutex};

use personal_rns::{
    interfaces::{InterfaceDescriptor, InterfaceKind, ReportsStatus},
    manifold::interface_seam::{Interface, InterfaceSeam},
    runtime::{PrnsNodeHandle, TokioHandleEntropy},
};
use prns_core::entropy::RuntimeEntropy;
use prns_simulation::VirtualInterface;

use super::BootId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Observation {
    Interface { boot: BootId, bytes: [u8; 64] },
    Handle { bytes: [u8; 64] },
    Path { boot: BootId, bytes: Vec<u8> },
}

#[derive(Clone)]
pub(super) struct Inputs {
    pub shared_seed: u8,
    pub path_seed: u8,
    observations: Arc<Mutex<Vec<Observation>>>,
}

impl Inputs {
    pub fn new(shared_seed: u8, path_seed: u8) -> Self {
        Self {
            shared_seed,
            path_seed,
            observations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn record(&self, observation: Observation) {
        let mut log = self
            .observations
            .lock()
            .unwrap_or_else(|_| unreachable!("fixture log poison"));
        assert!(log.len() < 16);
        log.push(observation);
    }

    pub fn snapshot(&self) -> Vec<Observation> {
        self.observations
            .lock()
            .unwrap_or_else(|_| unreachable!("fixture log poison"))
            .clone()
    }

    pub fn entropy(&self, boot: BootId) -> TokioHandleEntropy {
        let seed = self.shared_seed;
        let mut seed_reads = 0;
        let stream = RuntimeEntropy::try_new(move |output: &mut [u8]| {
            seed_reads += 1;
            assert_eq!(seed_reads, 1, "short shared-stream fixture never reseeds");
            output.fill(seed);
            Ok::<(), core::convert::Infallible>(())
        })
        .unwrap_or_else(|never| match never {});
        let inputs = self.clone();
        TokioHandleEntropy::from_sources(stream, move |output: &mut [u8]| {
            output.fill(inputs.path_seed);
            inputs.record(Observation::Path {
                boot,
                bytes: output.to_vec(),
            });
            Ok::<(), core::convert::Infallible>(())
        })
    }

    pub fn interface(&self, interface: VirtualInterface, boot: BootId) -> Probe {
        Probe {
            interface,
            boot,
            inputs: self.clone(),
        }
    }

    pub fn handle_bytes(&self, handle: &PrnsNodeHandle) -> [u8; 64] {
        let mut bytes = [0; 64];
        handle.fill_random(&mut bytes);
        self.record(Observation::Handle { bytes });
        let seed = self.shared_seed;
        let mut expected = RuntimeEntropy::try_new(move |output: &mut [u8]| {
            output.fill(seed);
            Ok::<(), core::convert::Infallible>(())
        })
        .unwrap_or_else(|never| match never {});
        let mut blocks = [[0; 64]; 2];
        for block in &mut blocks {
            expected.fill_random(block);
        }
        assert!(self.snapshot().contains(&Observation::Interface {
            boot: BootId {
                node: 0,
                generation: 0
            },
            bytes: blocks[0]
        }));
        assert_eq!(bytes, blocks[1]);
        bytes
    }
}

pub(super) struct Probe {
    interface: VirtualInterface,
    boot: BootId,
    inputs: Inputs,
}

impl ReportsStatus for Probe {}
impl Interface for Probe {
    const KIND: InterfaceKind = VirtualInterface::KIND;
    const HW_MTU: usize = VirtualInterface::HW_MTU;
    fn channel_tag(&self) -> &[u8] {
        self.interface.channel_tag()
    }
    fn descriptor(&self) -> InterfaceDescriptor {
        self.interface.descriptor()
    }
    async fn run<S: InterfaceSeam>(self, mut seam: S) {
        let mut bytes = [0; 64];
        seam.fill_random(&mut bytes);
        self.inputs.record(Observation::Interface {
            boot: self.boot,
            bytes,
        });
        self.interface.run(seam).await;
    }
}
