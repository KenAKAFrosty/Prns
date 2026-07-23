use objc2::Message;
use objc2_core_bluetooth::{
    CBCharacteristic, CBCharacteristicProperties, CBCharacteristicWriteType, CBPeripheral,
};
use tokio::sync::oneshot;

use super::{MacosBleError, SendCharacteristicRef};
use prns_core::interfaces::bluetooth_auto::{BLE_HW_MTU, FRAGMENT_HEADER_LEN};

const PORTABLE_GATT_FRAGMENT_MTU: usize = 180;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GattWriteMode {
    WithResponse,
    WithoutResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GattWriteAdmission {
    Issue,
    WaitForCapacity,
    Busy,
}

pub(super) const fn write_admission(
    mode: GattWriteMode,
    acknowledged_pending: bool,
    unacknowledged_pending: bool,
    can_send_without_response: bool,
) -> GattWriteAdmission {
    match mode {
        GattWriteMode::WithResponse if acknowledged_pending => GattWriteAdmission::Busy,
        GattWriteMode::WithResponse => GattWriteAdmission::Issue,
        GattWriteMode::WithoutResponse if unacknowledged_pending => GattWriteAdmission::Busy,
        GattWriteMode::WithoutResponse if can_send_without_response => GattWriteAdmission::Issue,
        GattWriteMode::WithoutResponse => GattWriteAdmission::WaitForCapacity,
    }
}

impl GattWriteMode {
    pub(super) const fn core_bluetooth_type(self) -> CBCharacteristicWriteType {
        match self {
            Self::WithResponse => CBCharacteristicWriteType::WithResponse,
            Self::WithoutResponse => CBCharacteristicWriteType::WithoutResponse,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GattWritePlan {
    mode: GattWriteMode,
    fragment_mtu: usize,
}

impl GattWritePlan {
    pub(super) fn discover(
        peripheral: &CBPeripheral,
        characteristic: &CBCharacteristic,
        mode: GattWriteMode,
    ) -> Result<Self, MacosBleError> {
        // SAFETY: both immutable queries run during characteristic discovery on the retained
        // peripheral's CoreBluetooth queue.
        let properties = unsafe { characteristic.properties() };
        // A with-response maximum may include prepared writes. The without-response maximum is
        // the connection's single-ATT-write ceiling, which keeps every Prns fragment atomic at the
        // peer's GATT server regardless of the selected write mode.
        // SAFETY: this immutable connection-limit query runs on the retained peripheral's
        // CoreBluetooth queue.
        let with_response_mtu = unsafe {
            peripheral.maximumWriteValueLengthForType(CBCharacteristicWriteType::WithResponse)
        };
        // SAFETY: CoreBluetooth exposes this connection-level limit independently of whether this
        // particular characteristic supports unacknowledged writes.
        let without_response_mtu = unsafe {
            peripheral.maximumWriteValueLengthForType(CBCharacteristicWriteType::WithoutResponse)
        };
        Self::from_discovery(mode, properties, with_response_mtu, without_response_mtu)
    }

    pub(super) fn from_discovery(
        mode: GattWriteMode,
        properties: CBCharacteristicProperties,
        with_response_mtu: usize,
        without_response_mtu: usize,
    ) -> Result<Self, MacosBleError> {
        let supported = match mode {
            GattWriteMode::WithResponse => properties.contains(CBCharacteristicProperties::Write),
            GattWriteMode::WithoutResponse => {
                properties.contains(CBCharacteristicProperties::WriteWithoutResponse)
            }
        };
        if !supported {
            return Err(MacosBleError::UnsupportedWriteMode);
        }
        let selected_mtu = match mode {
            GattWriteMode::WithResponse => with_response_mtu.min(without_response_mtu),
            GattWriteMode::WithoutResponse => without_response_mtu,
        };
        let fragment_mtu = selected_mtu.min(PORTABLE_GATT_FRAGMENT_MTU).min(BLE_HW_MTU);
        if fragment_mtu <= FRAGMENT_HEADER_LEN {
            return Err(MacosBleError::InvalidWriteMtu);
        }
        Ok(Self { mode, fragment_mtu })
    }

    pub(super) const fn mode(self) -> GattWriteMode {
        self.mode
    }

    pub(super) const fn fragment_mtu(self) -> usize {
        self.fragment_mtu
    }
}

pub(super) struct GattWriteTarget {
    pub(super) characteristic: SendCharacteristicRef,
    pub(super) plan: GattWritePlan,
}

impl GattWriteTarget {
    pub(super) fn discover(
        peripheral: &CBPeripheral,
        characteristic: &CBCharacteristic,
        mode: GattWriteMode,
    ) -> Result<Self, MacosBleError> {
        Ok(Self {
            characteristic: SendCharacteristicRef(characteristic.retain()),
            plan: GattWritePlan::discover(peripheral, characteristic, mode)?,
        })
    }
}

pub(super) type GattWriteCompletion = oneshot::Sender<Result<(), MacosBleError>>;

pub(super) struct GattWriteRequest {
    pub(super) characteristic: SendCharacteristicRef,
    pub(super) bytes: Box<[u8]>,
    pub(super) mode: GattWriteMode,
    completion: GattWriteCompletion,
}

impl GattWriteRequest {
    pub(super) fn new(
        characteristic: SendCharacteristicRef,
        bytes: Box<[u8]>,
        mode: GattWriteMode,
        completion: GattWriteCompletion,
    ) -> Self {
        Self {
            characteristic,
            bytes,
            mode,
            completion,
        }
    }

    pub(super) fn receiver_closed(&self) -> bool {
        self.completion.is_closed()
    }

    pub(super) fn complete(self, result: Result<(), MacosBleError>) {
        let _ = self.completion.send(result);
    }

    pub(super) fn into_acknowledged(self) -> PendingAcknowledgedWrite {
        PendingAcknowledgedWrite {
            characteristic: self.characteristic,
            completion: self.completion,
        }
    }
}

pub(super) struct PendingAcknowledgedWrite {
    pub(super) characteristic: SendCharacteristicRef,
    completion: GattWriteCompletion,
}

impl PendingAcknowledgedWrite {
    pub(super) fn complete(self, result: Result<(), MacosBleError>) {
        let _ = self.completion.send(result);
    }
}
