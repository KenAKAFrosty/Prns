use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, Control, DialOutcome,
    L2capPlan, Origin, PeerProtocol, RadioMode, ScanningMode,
};
use tokio::sync::mpsc;

use super::{
    BleAddress, BleAdvanceError, BleAdvanceReport, BleAdvertisement, BleAdvertisementError,
    BleAdvertisingParameters, BleMediumConfig, BleRadioId, BleRoleCapabilities, BleSimulationError,
    BleTraceSnapshot, VirtualBleMedium,
};
use crate::{SimulationDurationInTicks, SimulationTick};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualBleBackendConfigError {
    Advertisement(BleAdvertisementError),
    ZeroAdvertisingInterval,
    ZeroInboundLinkCapacity,
    ZeroControlCapacity,
    ZeroDataCapacity,
    ZeroMaximumFrameLength,
}

impl fmt::Display for VirtualBleBackendConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Advertisement(error) => write!(formatter, "invalid BLE advertisement: {error}"),
            Self::ZeroAdvertisingInterval => {
                formatter.write_str("advertising interval must be nonzero")
            }
            Self::ZeroInboundLinkCapacity => {
                formatter.write_str("inbound link capacity must be nonzero")
            }
            Self::ZeroControlCapacity => {
                formatter.write_str("control message capacity must be nonzero")
            }
            Self::ZeroDataCapacity => formatter.write_str("data frame capacity must be nonzero"),
            Self::ZeroMaximumFrameLength => {
                formatter.write_str("maximum frame length must be nonzero")
            }
        }
    }
}

impl std::error::Error for VirtualBleBackendConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualBleBackendConfig {
    address: BleAddress,
    received_signal_strength_dbm: i8,
    advertising: BleAdvertisingParameters,
    inbound_link_capacity: usize,
    link: VirtualBleLinkConfig,
}

impl VirtualBleBackendConfig {
    pub fn new(
        address: BleAddress,
        received_signal_strength_dbm: i8,
        role_capabilities: BleRoleCapabilities,
        advertising_interval: SimulationDurationInTicks,
        inbound_link_capacity: usize,
        link: VirtualBleLinkConfig,
    ) -> Result<Self, VirtualBleBackendConfigError> {
        if advertising_interval == SimulationDurationInTicks::ZERO {
            return Err(VirtualBleBackendConfigError::ZeroAdvertisingInterval);
        }
        if inbound_link_capacity == 0 {
            return Err(VirtualBleBackendConfigError::ZeroInboundLinkCapacity);
        }
        let advertisement = BleAdvertisement::reticulum(role_capabilities)
            .map_err(VirtualBleBackendConfigError::Advertisement)?;
        let advertising = BleAdvertisingParameters::new(advertisement, advertising_interval)
            .map_err(|_| VirtualBleBackendConfigError::ZeroAdvertisingInterval)?;
        Ok(Self {
            address,
            received_signal_strength_dbm,
            advertising,
            inbound_link_capacity,
            link,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualBleLinkConfig {
    control_capacity: usize,
    data_capacity: usize,
    maximum_frame_length: usize,
}

impl VirtualBleLinkConfig {
    pub fn new(
        control_capacity: usize,
        data_capacity: usize,
        maximum_frame_length: usize,
    ) -> Result<Self, VirtualBleBackendConfigError> {
        for (capacity, error) in [
            (
                control_capacity,
                VirtualBleBackendConfigError::ZeroControlCapacity,
            ),
            (
                data_capacity,
                VirtualBleBackendConfigError::ZeroDataCapacity,
            ),
            (
                maximum_frame_length,
                VirtualBleBackendConfigError::ZeroMaximumFrameLength,
            ),
        ] {
            if capacity == 0 {
                return Err(error);
            }
        }
        Ok(Self {
            control_capacity,
            data_capacity,
            maximum_frame_length,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualBleError {
    Simulation(BleSimulationError),
    LinkClosed,
    FrameTooLong { length: usize, maximum: usize },
    ReceiveBufferTooSmall { frame: usize, buffer: usize },
}

impl fmt::Display for VirtualBleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Simulation(error) => error.fmt(formatter),
            Self::LinkClosed => formatter.write_str("virtual BLE link is closed"),
            Self::FrameTooLong { length, maximum } => {
                write!(
                    formatter,
                    "BLE frame is {length} bytes; maximum is {maximum}"
                )
            }
            Self::ReceiveBufferTooSmall { frame, buffer } => write!(
                formatter,
                "BLE receive buffer is {buffer} bytes but the queued frame is {frame} bytes",
            ),
        }
    }
}

impl std::error::Error for VirtualBleError {}

impl From<BleSimulationError> for VirtualBleError {
    fn from(error: BleSimulationError) -> Self {
        Self::Simulation(error)
    }
}

#[derive(Clone)]
pub struct VirtualBleLab {
    medium: VirtualBleMedium,
    network: Arc<Mutex<ConnectionNetwork>>,
}

impl VirtualBleLab {
    #[must_use]
    pub fn new(config: BleMediumConfig) -> Self {
        Self {
            medium: VirtualBleMedium::new(config),
            network: Arc::new(Mutex::new(ConnectionNetwork::default())),
        }
    }

    pub fn attach_backend(
        &self,
        config: VirtualBleBackendConfig,
    ) -> Result<VirtualBleBackend, BleSimulationError> {
        let radio = self
            .medium
            .attach(config.address, config.received_signal_strength_dbm)?;
        let (inbound, inbound_rx) = mpsc::channel(config.inbound_link_capacity);
        let powered = Arc::new(AtomicBool::new(false));
        let replaced = self.lock_network().peers.insert(
            config.address,
            RegisteredPeer {
                radio,
                inbound,
                powered: powered.clone(),
                received_signal_strength_dbm: config.received_signal_strength_dbm,
                link: config.link,
            },
        );
        debug_assert!(
            replaced.is_none(),
            "the medium rejected duplicate addresses"
        );
        Ok(VirtualBleBackend {
            medium: self.medium.clone(),
            network: self.network.clone(),
            config,
            radio,
            powered,
            inbound_rx,
            known_peers: BTreeSet::new(),
            dialed: None,
        })
    }

    #[must_use]
    pub fn now(&self) -> SimulationTick {
        self.medium.now()
    }

    pub fn advance_to(
        &self,
        requested: SimulationTick,
    ) -> Result<BleAdvanceReport, BleAdvanceError> {
        self.medium.advance_to(requested)
    }

    pub fn advance_by(
        &self,
        by: SimulationDurationInTicks,
    ) -> Result<BleAdvanceReport, BleAdvanceError> {
        self.medium.advance_by(by)
    }

    #[must_use]
    pub fn trace(&self) -> BleTraceSnapshot {
        self.medium.trace()
    }

    fn lock_network(&self) -> MutexGuard<'_, ConnectionNetwork> {
        self.network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Default)]
struct ConnectionNetwork {
    peers: BTreeMap<BleAddress, RegisteredPeer>,
}

struct RegisteredPeer {
    radio: BleRadioId,
    inbound: mpsc::Sender<VirtualBleLink>,
    powered: Arc<AtomicBool>,
    received_signal_strength_dbm: i8,
    link: VirtualBleLinkConfig,
}

pub struct VirtualBleBackend {
    medium: VirtualBleMedium,
    network: Arc<Mutex<ConnectionNetwork>>,
    config: VirtualBleBackendConfig,
    radio: BleRadioId,
    powered: Arc<AtomicBool>,
    inbound_rx: mpsc::Receiver<VirtualBleLink>,
    known_peers: BTreeSet<BleAddress>,
    dialed: Option<VirtualBleLink>,
}

impl VirtualBleBackend {
    fn lock_network(&self) -> MutexGuard<'_, ConnectionNetwork> {
        self.network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<const MAX_PEERS: usize> BleBackend<MAX_PEERS> for VirtualBleBackend {
    type Error = VirtualBleError;
    type Link = VirtualBleLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Self::Error> {
        let parameters = mode.is_on().then_some(self.config.advertising);
        let _ = self.medium.set_advertising(self.radio, parameters)?;
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Self::Error> {
        let _ = self.medium.set_scanning(self.radio, mode)?;
        Ok(())
    }

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), Self::Error> {
        let _ = self.medium.set_radio_power(self.radio, mode)?;
        self.powered.store(mode.is_on(), Ordering::Release);
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<Self::Link> {
        loop {
            if let Some(link) = self.dialed.take() {
                let peer_rssi = Some(link.peer_signal_strength());
                return BleEvent::LinkReady {
                    link,
                    origin: Origin::Dialed,
                    peer_rssi,
                };
            }
            let medium = self.medium.clone();
            let radio = self.radio;
            tokio::select! {
                biased;
                inbound = self.inbound_rx.recv() => match inbound {
                    Some(link) => {
                        let peer_rssi = Some(link.peer_signal_strength());
                        return BleEvent::LinkReady {
                            link,
                            origin: Origin::Accepted,
                            peer_rssi,
                        };
                    }
                    None => std::future::pending().await,
                },
                observation = medium.next_observation(radio) => match observation {
                    Ok(observation) if observation.advertisement.contains_reticulum_service() => {
                        let _ = self.known_peers.insert(observation.address);
                        return BleEvent::Sighting {
                            address: observation.address,
                            rssi: Some(observation.received_signal_strength_dbm),
                        };
                    }
                    Ok(_) | Err(_) => {}
                },
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        if !self.powered.load(Ordering::Acquire) {
            return DialOutcome::RadioOff;
        }
        if address == self.config.address {
            return DialOutcome::InvariantViolation;
        }
        if self.dialed.is_some() {
            return DialOutcome::Busy;
        }
        if !self.known_peers.contains(&address) {
            return DialOutcome::UnknownPeer;
        }
        let peer = {
            let network = self.lock_network();
            network.peers.get(&address).map(|peer| {
                (
                    peer.inbound.clone(),
                    peer.powered.clone(),
                    peer.received_signal_strength_dbm,
                    peer.link,
                )
            })
        };
        let Some((inbound, powered, peer_rssi, peer_config)) = peer else {
            return DialOutcome::UnknownPeer;
        };
        if !powered.load(Ordering::Acquire) {
            return DialOutcome::UnknownPeer;
        }
        let (mine, theirs) = link_pair(
            self.config.address,
            address,
            self.config.link,
            peer_config,
            self.config.received_signal_strength_dbm,
            peer_rssi,
        );
        match inbound.try_send(theirs) {
            Ok(()) => {
                self.dialed = Some(mine);
                DialOutcome::Started
            }
            Err(mpsc::error::TrySendError::Full(_)) => DialOutcome::Busy,
            Err(mpsc::error::TrySendError::Closed(_)) => DialOutcome::UnknownPeer,
        }
    }
}

impl Drop for VirtualBleBackend {
    fn drop(&mut self) {
        self.powered.store(false, Ordering::Release);
        let mut network = self
            .network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if network
            .peers
            .get(&self.config.address)
            .is_some_and(|peer| peer.radio == self.radio)
        {
            let _ = network.peers.remove(&self.config.address);
        }
        drop(network);
        self.medium.detach(self.radio);
    }
}

pub struct VirtualBleLink {
    peer_address: BleAddress,
    control_tx: mpsc::Sender<Control>,
    control_rx: mpsc::Receiver<Control>,
    data_tx: mpsc::Sender<Vec<u8>>,
    data_rx: mpsc::Receiver<Vec<u8>>,
    maximum_frame_length: usize,
    peer_signal_strength_dbm: i8,
}

impl VirtualBleLink {
    const fn peer_signal_strength(&self) -> i8 {
        self.peer_signal_strength_dbm
    }
}

impl BleLink for VirtualBleLink {
    type Error = VirtualBleError;
    type Source = VirtualBleSource;
    type Sink = VirtualBleSink;

    fn peer_protocol(&self) -> PeerProtocol {
        PeerProtocol::Native
    }

    fn address(&self) -> BleAddress {
        self.peer_address
    }

    async fn control_send(&mut self, message: &Control) -> Result<(), Self::Error> {
        self.control_tx
            .send(*message)
            .await
            .map_err(|_| VirtualBleError::LinkClosed)
    }

    async fn control_recv(&mut self) -> Result<Control, Self::Error> {
        self.control_rx
            .recv()
            .await
            .ok_or(VirtualBleError::LinkClosed)
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), Self::Error> {
        Ok(())
    }

    fn into_data(self) -> (Self::Source, Self::Sink) {
        (
            VirtualBleSource {
                receiver: self.data_rx,
            },
            VirtualBleSink {
                sender: self.data_tx,
                maximum_frame_length: self.maximum_frame_length,
            },
        )
    }
}

pub struct VirtualBleSource {
    receiver: mpsc::Receiver<Vec<u8>>,
}

impl BleSource for VirtualBleSource {
    type Error = VirtualBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        let frame = self
            .receiver
            .recv()
            .await
            .ok_or(VirtualBleError::LinkClosed)?;
        if frame.len() > out.len() {
            return Err(VirtualBleError::ReceiveBufferTooSmall {
                frame: frame.len(),
                buffer: out.len(),
            });
        }
        out[..frame.len()].copy_from_slice(&frame);
        Ok(frame.len())
    }
}

pub struct VirtualBleSink {
    sender: mpsc::Sender<Vec<u8>>,
    maximum_frame_length: usize,
}

impl BleSink for VirtualBleSink {
    type Error = VirtualBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        if frame.len() > self.maximum_frame_length {
            return Err(VirtualBleError::FrameTooLong {
                length: frame.len(),
                maximum: self.maximum_frame_length,
            });
        }
        self.sender
            .send(frame.to_vec())
            .await
            .map_err(|_| VirtualBleError::LinkClosed)
    }
}

fn link_pair(
    address_a: BleAddress,
    address_b: BleAddress,
    config_a: VirtualBleLinkConfig,
    config_b: VirtualBleLinkConfig,
    signal_strength_a: i8,
    signal_strength_b: i8,
) -> (VirtualBleLink, VirtualBleLink) {
    let control_capacity = config_a.control_capacity.min(config_b.control_capacity);
    let data_capacity = config_a.data_capacity.min(config_b.data_capacity);
    let maximum_frame_length = config_a
        .maximum_frame_length
        .min(config_b.maximum_frame_length);
    let (control_a_tx, control_a_rx) = mpsc::channel(control_capacity);
    let (control_b_tx, control_b_rx) = mpsc::channel(control_capacity);
    let (data_a_tx, data_a_rx) = mpsc::channel(data_capacity);
    let (data_b_tx, data_b_rx) = mpsc::channel(data_capacity);
    (
        VirtualBleLink {
            peer_address: address_b,
            control_tx: control_a_tx,
            control_rx: control_b_rx,
            data_tx: data_a_tx,
            data_rx: data_b_rx,
            maximum_frame_length,
            peer_signal_strength_dbm: signal_strength_b,
        },
        VirtualBleLink {
            peer_address: address_a,
            control_tx: control_b_tx,
            control_rx: control_a_rx,
            data_tx: data_b_tx,
            data_rx: data_a_rx,
            maximum_frame_length,
            peer_signal_strength_dbm: signal_strength_a,
        },
    )
}
