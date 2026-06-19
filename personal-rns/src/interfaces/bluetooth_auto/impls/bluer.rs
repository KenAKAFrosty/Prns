use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;

use bluer::adv::{Advertisement, AdvertisementHandle};
use bluer::gatt::local::{
    characteristic_control, service_control, Application, ApplicationHandle, Characteristic,
    CharacteristicControl, CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicWrite,
    CharacteristicWriteMethod, Service,
};
use bluer::gatt::remote::Characteristic as RemoteCharacteristic;
use bluer::l2cap::{SeqPacket, Socket, SocketAddr as L2capSocketAddr};
use bluer::{Adapter, AdapterEvent, Address, AddressType, Device, Session, Uuid};
use futures_util::{Stream, StreamExt};

use crate::interfaces::bluetooth_auto::core::{
    BleAddress, BleUuid, Control, Dialect, Transport, BLE_HW_MTU, BLE_SERVICE_UUID,
    CONTROL_MAX_LEN, NATIVE_CONTROL_UUID,
};
use crate::interfaces::bluetooth_auto::seam::{BleBackend, BleEvent, BleLink, BleSink, BleSource};

const ADVERTISED_NAME: &str = "Prns";

#[derive(Debug)]
pub enum BluerError {
    Bluez(bluer::Error),
    Io(std::io::Error),
    NoControlCharacteristic,
    ControlPduTooLarge,
    MalformedControl,
    GattDataUnsupported,
    NotUpgraded,
    Closed,
}

impl From<bluer::Error> for BluerError {
    fn from(error: bluer::Error) -> Self {
        BluerError::Bluez(error)
    }
}

impl From<std::io::Error> for BluerError {
    fn from(error: std::io::Error) -> Self {
        BluerError::Io(error)
    }
}

fn uuid_of(uuid: BleUuid) -> Uuid {
    match uuid {
        BleUuid::Bit128(bytes) => Uuid::from_bytes(bytes),
        BleUuid::Bit16(short) => {
            let mut bytes = [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b,
                0x34, 0xfb,
            ];
            bytes[2..4].copy_from_slice(&short.to_be_bytes());
            Uuid::from_bytes(bytes)
        }
    }
}

pub struct BluerBackend {
    adapter: Adapter,
    address: Address,
    seen: HashSet<Address>,
    discovery: Option<Pin<Box<dyn Stream<Item = AdapterEvent> + Send>>>,
    control: Option<Pin<Box<CharacteristicControl>>>,
    _advertisement: Option<AdvertisementHandle>,
    _application: Option<ApplicationHandle>,
}

impl BluerBackend {
    pub async fn open() -> Result<Self, BluerError> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;
        let address = adapter.address().await?;
        Ok(Self {
            adapter,
            address,
            seen: HashSet::new(),
            discovery: None,
            control: None,
            _advertisement: None,
            _application: None,
        })
    }

    async fn advertises_our_service(&self, address: Address) -> bool {
        let Ok(device) = self.adapter.device(address) else {
            return false;
        };
        match device.uuids().await {
            Ok(Some(uuids)) => uuids.contains(&uuid_of(BLE_SERVICE_UUID)),
            _ => false,
        }
    }
}

enum Observed {
    Candidate(Address),
    Idle,
}

async fn next_or_pending<S>(stream: Option<&mut S>) -> Option<S::Item>
where
    S: Stream + Unpin,
{
    match stream {
        Some(stream) => stream.next().await,
        None => core::future::pending().await,
    }
}

impl BleBackend for BluerBackend {
    const MAX_PEERS: usize = 8;
    type Error = BluerError;
    type Link = DialedLink;

    async fn advertise(&mut self) -> Result<(), BluerError> {
        let advertisement = Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids: [uuid_of(BLE_SERVICE_UUID)].into_iter().collect(),
            discoverable: Some(true),
            local_name: Some(ADVERTISED_NAME.to_string()),
            ..Default::default()
        };
        let advertisement = self.adapter.advertise(advertisement).await?;

        let (control, control_handle) = characteristic_control();
        let (_service_control, service_handle) = service_control();
        let application = Application {
            services: vec![Service {
                uuid: uuid_of(BLE_SERVICE_UUID),
                primary: true,
                characteristics: vec![Characteristic {
                    uuid: uuid_of(NATIVE_CONTROL_UUID),
                    write: Some(CharacteristicWrite {
                        write: true,
                        write_without_response: true,
                        method: CharacteristicWriteMethod::Io,
                        ..Default::default()
                    }),
                    notify: Some(CharacteristicNotify {
                        notify: true,
                        method: CharacteristicNotifyMethod::Io,
                        ..Default::default()
                    }),
                    control_handle,
                    ..Default::default()
                }],
                control_handle: service_handle,
                ..Default::default()
            }],
            ..Default::default()
        };
        let application = self.adapter.serve_gatt_application(application).await?;

        let discovery = self.adapter.discover_devices().await?;

        self.control = Some(Box::pin(control));
        self.discovery = Some(Box::pin(discovery));
        self._advertisement = Some(advertisement);
        self._application = Some(application);
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<DialedLink> {
        loop {
            let observed = {
                let discovery = self.discovery.as_mut();
                let control = self.control.as_mut();
                tokio::select! {
                    event = next_or_pending(discovery) => match event {
                        Some(AdapterEvent::DeviceAdded(address)) => Observed::Candidate(address),
                        _ => Observed::Idle,
                    },
                    _ = next_or_pending(control) => Observed::Idle,
                }
            };
            if let Observed::Candidate(address) = observed {
                if address != self.address
                    && !self.seen.contains(&address)
                    && self.advertises_our_service(address).await
                {
                    self.seen.insert(address);
                    return BleEvent::Sighting(BleAddress::new(address.0));
                }
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) -> Result<DialedLink, BluerError> {
        let target = Address::new(*address.octets());
        let device = self.adapter.device(target)?;
        if !device.is_connected().await? {
            device.connect().await?;
        }
        let peer_address_type = device.address_type().await?;
        let control = find_native_control(&device).await?;
        let notify = control.notify().await?;
        Ok(DialedLink {
            control,
            notify: Box::pin(notify),
            peer_address: target,
            peer_address_type,
            socket: None,
            _device: device,
        })
    }
}

async fn find_native_control(device: &Device) -> Result<RemoteCharacteristic, BluerError> {
    let service_uuid = uuid_of(BLE_SERVICE_UUID);
    let control_uuid = uuid_of(NATIVE_CONTROL_UUID);
    for service in device.services().await? {
        if service.uuid().await? == service_uuid {
            for characteristic in service.characteristics().await? {
                if characteristic.uuid().await? == control_uuid {
                    return Ok(characteristic);
                }
            }
        }
    }
    Err(BluerError::NoControlCharacteristic)
}

pub struct DialedLink {
    control: RemoteCharacteristic,
    notify: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
    peer_address: Address,
    peer_address_type: AddressType,
    socket: Option<Arc<SeqPacket>>,
    _device: Device,
}

impl BleLink for DialedLink {
    type Error = BluerError;
    type Source = L2capSource;
    type Sink = L2capSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.peer_address.0)
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), BluerError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(BluerError::ControlPduTooLarge)?;
        self.control.write(&buf[..len]).await?;
        Ok(())
    }

    async fn control_recv(&mut self) -> Result<Control, BluerError> {
        let value = self.notify.next().await.ok_or(BluerError::Closed)?;
        Control::decode(&value).ok_or(BluerError::MalformedControl)
    }

    async fn upgrade(&mut self, transport: &Transport) -> Result<(), BluerError> {
        match transport {
            Transport::L2cap { psm } => {
                let socket = Socket::<SeqPacket>::new_seq_packet()?;
                socket.set_recv_mtu(BLE_HW_MTU as u16)?;
                socket.bind(L2capSocketAddr::any_le())?;
                let target =
                    L2capSocketAddr::new(self.peer_address, self.peer_address_type, psm.get());
                let connected = socket.connect(target).await?;
                self.socket = Some(Arc::new(connected));
                Ok(())
            }
            Transport::Gatt => Err(BluerError::GattDataUnsupported),
        }
    }

    fn into_data(self) -> (L2capSource, L2capSink) {
        (L2capSource(self.socket.clone()), L2capSink(self.socket))
    }
}

pub struct L2capSource(Option<Arc<SeqPacket>>);

impl BleSource for L2capSource {
    type Error = BluerError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, BluerError> {
        match &self.0 {
            Some(socket) => Ok(socket.recv(out).await?),
            None => Err(BluerError::NotUpgraded),
        }
    }
}

pub struct L2capSink(Option<Arc<SeqPacket>>);

impl BleSink for L2capSink {
    type Error = BluerError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), BluerError> {
        match &self.0 {
            Some(socket) => {
                socket.send(frame).await?;
                Ok(())
            }
            None => Err(BluerError::NotUpgraded),
        }
    }
}
