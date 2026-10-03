use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableGetOutcome};

use super::{DeviceRegistry, DeviceSnapshot};
use crate::domain_primitives::DeviceId;

#[derive(Debug, PartialEq, Eq)]
pub struct ReadDevice {
    pub device: DeviceId,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
#[expect(clippy::large_enum_variant)]
pub enum ReadDeviceOutcome {
    Found { device: DeviceSnapshot },
    MissingDevice { device: DeviceId },
}

impl StepInputOf<DeviceRegistry> for ReadDevice {
    type Outcome = ReadDeviceOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry.devices.get(WarpId::new(self.device.0)) {
            WarpTableGetOutcome::Present { row } => ReadDeviceOutcome::Found {
                device: DeviceSnapshot {
                    id: self.device,
                    label: row.label.clone(),
                    enrollment: row.enrollment.clone(),
                },
            },
            WarpTableGetOutcome::Absent { .. } => ReadDeviceOutcome::MissingDevice {
                device: self.device,
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ListDevices<const CAPACITY: usize>;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ListDevicesOutcome<const CAPACITY: usize> {
    Listed {
        devices: heapless::Vec<DeviceId, CAPACITY>,
    },
    InsufficientCapacity {
        required: usize,
    },
}

impl<const CAPACITY: usize> StepInputOf<DeviceRegistry> for ListDevices<CAPACITY> {
    type Outcome = ListDevicesOutcome<CAPACITY>;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        let mut devices = heapless::Vec::new();
        for row in registry.devices.rows() {
            if devices.push(DeviceId(row.id.value())).is_err() {
                return ListDevicesOutcome::InsufficientCapacity {
                    required: registry.devices.rows().len(),
                };
            }
        }
        ListDevicesOutcome::Listed { devices }
    }
}
