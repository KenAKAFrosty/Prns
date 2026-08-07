use heapless09::Vec as GattValue;
use nrf_softdevice::ble::gatt_server::builder::ServiceBuilder;
use nrf_softdevice::ble::gatt_server::characteristic::{Attribute, Metadata, Properties};
use nrf_softdevice::ble::gatt_server::{self, CharacteristicHandles, WriteOp};
use nrf_softdevice::ble::{Connection, DeferredWriteReply, GattError, Uuid};
use nrf_softdevice::Softdevice;

const GATT_VALUE_CAPACITY: usize = 244;
const SERVICE_UUID: [u8; 16] = [
    0xe3, 0x28, 0xda, 0xc5, 0x42, 0x8f, 0x7f, 0x91, 0x94, 0x4a, 0x2d, 0x44, 0x00, 0x5b, 0x14, 0x37,
];
const COLUMBA_TX_UUID: [u8; 16] = [
    0xe4, 0x28, 0xda, 0xc5, 0x42, 0x8f, 0x7f, 0x91, 0x94, 0x4a, 0x2d, 0x44, 0x00, 0x5b, 0x14, 0x37,
];
const COLUMBA_RX_UUID: [u8; 16] = [
    0xe5, 0x28, 0xda, 0xc5, 0x42, 0x8f, 0x7f, 0x91, 0x94, 0x4a, 0x2d, 0x44, 0x00, 0x5b, 0x14, 0x37,
];
const COLUMBA_IDENTITY_UUID: [u8; 16] = [
    0xe6, 0x28, 0xda, 0xc5, 0x42, 0x8f, 0x7f, 0x91, 0x94, 0x4a, 0x2d, 0x44, 0x00, 0x5b, 0x14, 0x37,
];
const CONTROL_UUID: [u8; 16] = [
    0xe7, 0x28, 0xda, 0xc5, 0x42, 0x8f, 0x7f, 0x91, 0x94, 0x4a, 0x2d, 0x44, 0x00, 0x5b, 0x14, 0x37,
];
const DATA_UUID: [u8; 16] = [
    0xe8, 0x28, 0xda, 0xc5, 0x42, 0x8f, 0x7f, 0x91, 0x94, 0x4a, 0x2d, 0x44, 0x00, 0x5b, 0x14, 0x37,
];

pub(super) type WriteValue = GattValue<u8, GATT_VALUE_CAPACITY>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteTarget {
    ColumbaRx,
    Control,
    Data,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteDelivery {
    Acknowledged,
    Unacknowledged,
}

pub(super) struct ServerWrite {
    target: WriteTarget,
    delivery: WriteDelivery,
    value: WriteValue,
    reply: DeferredWriteReply,
}

impl ServerWrite {
    pub(super) fn target(&self) -> WriteTarget {
        self.target
    }

    pub(super) fn delivery(&self) -> WriteDelivery {
        self.delivery
    }

    pub(super) fn value(&self) -> &[u8] {
        &self.value
    }

    pub(super) fn accept(self) {
        let _ = self.reply.reply(Ok(&self.value));
    }

    pub(super) fn reject(self, error: GattError) {
        let _ = self.reply.reply(Err(error));
    }
}

struct ReticulumService {
    columba_tx: CharacteristicHandles,
    columba_rx: CharacteristicHandles,
    columba_identity: CharacteristicHandles,
    control: CharacteristicHandles,
    data: CharacteristicHandles,
}

impl ReticulumService {
    fn new(sd: &mut Softdevice) -> Result<Self, gatt_server::RegisterError> {
        let mut service = ServiceBuilder::new(sd, Uuid::new_128(&SERVICE_UUID))?;
        let empty: [u8; 0] = [];
        let readable = Attribute::new(&empty).variable_len(GATT_VALUE_CAPACITY as u16);
        let writable = Attribute::new(&empty)
            .variable_len(GATT_VALUE_CAPACITY as u16)
            .deferred_write();
        let columba_tx = service
            .add_characteristic(
                Uuid::new_128(&COLUMBA_TX_UUID),
                readable,
                Metadata::new(Properties::new().read().notify()),
            )?
            .build();
        let columba_rx = service
            .add_characteristic(
                Uuid::new_128(&COLUMBA_RX_UUID),
                writable,
                Metadata::new(Properties::new().write().write_without_response()),
            )?
            .build();
        let columba_identity = service
            .add_characteristic(
                Uuid::new_128(&COLUMBA_IDENTITY_UUID),
                readable,
                Metadata::new(Properties::new().read()),
            )?
            .build();
        let control = service
            .add_characteristic(
                Uuid::new_128(&CONTROL_UUID),
                writable,
                Metadata::new(Properties::new().write().notify()),
            )?
            .build();
        let data = service
            .add_characteristic(
                Uuid::new_128(&DATA_UUID),
                writable,
                Metadata::new(Properties::new().write().notify()),
            )?
            .build();
        let _ = service.build();
        Ok(Self {
            columba_tx,
            columba_rx,
            columba_identity,
            control,
            data,
        })
    }

    fn target(&self, handle: u16) -> Option<WriteTarget> {
        if handle == self.columba_rx.value_handle {
            Some(WriteTarget::ColumbaRx)
        } else if handle == self.control.value_handle {
            Some(WriteTarget::Control)
        } else if handle == self.data.value_handle {
            Some(WriteTarget::Data)
        } else {
            None
        }
    }
}

pub(super) struct Server {
    rns: ReticulumService,
}

impl Server {
    pub(super) fn new(sd: &mut Softdevice) -> Result<Self, gatt_server::RegisterError> {
        Ok(Self {
            rns: ReticulumService::new(sd)?,
        })
    }

    pub(super) fn set_columba_identity(
        &self,
        sd: &Softdevice,
        identity: &[u8],
    ) -> Result<(), gatt_server::SetValueError> {
        gatt_server::set_value(sd, self.rns.columba_identity.value_handle, identity)
    }

    pub(super) fn notify_control(
        &self,
        conn: &Connection,
        value: &[u8],
    ) -> Result<(), gatt_server::NotifyValueError> {
        gatt_server::notify_value(conn, self.rns.control.value_handle, value)
    }

    pub(super) fn notify_native_data(
        &self,
        conn: &Connection,
        value: &[u8],
    ) -> Result<(), gatt_server::NotifyValueError> {
        gatt_server::notify_value(conn, self.rns.data.value_handle, value)
    }

    pub(super) fn notify_columba_data(
        &self,
        conn: &Connection,
        value: &[u8],
    ) -> Result<(), gatt_server::NotifyValueError> {
        gatt_server::notify_value(conn, self.rns.columba_tx.value_handle, value)
    }
}

impl gatt_server::Server for Server {
    type Event = ServerWrite;

    fn on_write(
        &self,
        _conn: &Connection,
        _handle: u16,
        _op: WriteOp,
        _offset: usize,
        _data: &[u8],
    ) -> Option<Self::Event> {
        None
    }

    fn on_deferred_write(
        &self,
        handle: u16,
        op: WriteOp,
        offset: usize,
        data: &[u8],
        reply: DeferredWriteReply,
    ) -> Option<Self::Event> {
        let Some(target) = self.rns.target(handle) else {
            let _ = reply.reply(Err(GattError::ATTERR_ATTRIBUTE_NOT_FOUND));
            return None;
        };
        if offset != 0 {
            let _ = reply.reply(Err(GattError::ATTERR_INVALID_OFFSET));
            return None;
        }
        let delivery = match op {
            WriteOp::Request => WriteDelivery::Acknowledged,
            WriteOp::Command | WriteOp::SignedWriteCommmand => WriteDelivery::Unacknowledged,
            WriteOp::PrepareWriteRequest
            | WriteOp::CancelPreparedWrites
            | WriteOp::ExecutePreparedWrites => {
                let _ = reply.reply(Err(GattError::ATTERR_REQUEST_NOT_SUPPORTED));
                return None;
            }
            _ => {
                let _ = reply.reply(Err(GattError::ATTERR_REQUEST_NOT_SUPPORTED));
                return None;
            }
        };
        let Ok(value) = WriteValue::from_slice(data) else {
            let _ = reply.reply(Err(GattError::ATTERR_INVALID_ATT_VAL_LENGTH));
            return None;
        };
        Some(ServerWrite {
            target,
            delivery,
            value,
            reply,
        })
    }
}
