use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, Control, DialOutcome,
    L2capPlan, Origin, PeerProtocol, RadioMode, ScanningMode,
};
use tokio::sync::{mpsc, watch};

use super::connection::{Connection, ConnectionEndpoint};
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
    ZeroConnectionCapacity,
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
            Self::ZeroConnectionCapacity => {
                formatter.write_str("connection capacity must be nonzero")
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
    connection_capacity: usize,
    link: VirtualBleLinkConfig,
}

impl VirtualBleBackendConfig {
    pub fn new(
        address: BleAddress,
        received_signal_strength_dbm: i8,
        role_capabilities: BleRoleCapabilities,
        advertising_interval: SimulationDurationInTicks,
        inbound_link_capacity: usize,
        connection_capacity: usize,
        link: VirtualBleLinkConfig,
    ) -> Result<Self, VirtualBleBackendConfigError> {
        if advertising_interval == SimulationDurationInTicks::ZERO {
            return Err(VirtualBleBackendConfigError::ZeroAdvertisingInterval);
        }
        if inbound_link_capacity == 0 {
            return Err(VirtualBleBackendConfigError::ZeroInboundLinkCapacity);
        }
        if connection_capacity == 0 {
            return Err(VirtualBleBackendConfigError::ZeroConnectionCapacity);
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
            connection_capacity,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "disconnect reports reveal whether an active connection was actually closed"]
pub struct VirtualBleDisconnectReport {
    pub connections_closed: usize,
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
        let mut network = self.lock_network();
        let radio = self
            .medium
            .attach(config.address, config.received_signal_strength_dbm)?;
        let (inbound, inbound_rx) = mpsc::channel(config.inbound_link_capacity);
        let replaced = network.peers.insert(
            config.address,
            RegisteredPeer {
                radio,
                inbound,
                connection_capacity: config.connection_capacity,
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

    #[must_use]
    pub fn active_connection_count(&self) -> usize {
        let mut network = self.lock_network();
        network.prune_closed();
        network.connections.len()
    }

    pub fn disconnect_between(
        &self,
        first: BleAddress,
        second: BleAddress,
    ) -> VirtualBleDisconnectReport {
        close_connections(&mut self.lock_network(), |connection| {
            connection.connects(first) && connection.connects(second)
        })
    }

    pub fn disconnect_radio(&self, address: BleAddress) -> VirtualBleDisconnectReport {
        close_connections(&mut self.lock_network(), |connection| {
            connection.connects(address)
        })
    }

    fn lock_network(&self) -> MutexGuard<'_, ConnectionNetwork> {
        self.network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

// Admission and radio transitions take this lock before the medium lock.
#[derive(Default)]
struct ConnectionNetwork {
    peers: BTreeMap<BleAddress, RegisteredPeer>,
    connections: Vec<Arc<Connection>>,
}

impl ConnectionNetwork {
    fn prune_closed(&mut self) {
        self.connections
            .retain(|connection| !connection.is_closed());
    }

    fn connection_count(&self, address: BleAddress) -> usize {
        self.connections
            .iter()
            .filter(|connection| connection.connects(address))
            .count()
    }
}

struct RegisteredPeer {
    radio: BleRadioId,
    inbound: mpsc::Sender<VirtualBleLink>,
    connection_capacity: usize,
    received_signal_strength_dbm: i8,
    link: VirtualBleLinkConfig,
}

pub struct VirtualBleBackend {
    medium: VirtualBleMedium,
    network: Arc<Mutex<ConnectionNetwork>>,
    config: VirtualBleBackendConfig,
    radio: BleRadioId,
    inbound_rx: mpsc::Receiver<VirtualBleLink>,
    known_peers: BTreeSet<BleAddress>,
    dialed: Option<VirtualBleLink>,
}

impl<const MAX_PEERS: usize> BleBackend<MAX_PEERS> for VirtualBleBackend {
    type Error = VirtualBleError;
    type Link = VirtualBleLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Self::Error> {
        let _network = self
            .network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parameters = mode.is_on().then_some(self.config.advertising);
        let _ = self.medium.set_advertising(self.radio, parameters)?;
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Self::Error> {
        let _ = self.medium.set_scanning(self.radio, mode)?;
        Ok(())
    }

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), Self::Error> {
        let mut network = self
            .network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = self.medium.set_radio_power(self.radio, mode)?;
        if !mode.is_on() {
            let _ = close_connections(&mut network, |connection| {
                connection.connects(self.config.address)
            });
            self.dialed = None;
            while self.inbound_rx.try_recv().is_ok() {}
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<Self::Link> {
        loop {
            if let Some(link) = self.dialed.take() {
                if link.endpoint.connection.is_closed() {
                    return BleEvent::DialFailed {
                        address: link.peer_address,
                    };
                }
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
                        if link.endpoint.connection.is_closed() { continue; }
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
        let mut network = self
            .network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.medium.is_powered(self.radio) {
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
        network.prune_closed();
        let Some(peer) = network.peers.get(&address) else {
            return DialOutcome::UnknownPeer;
        };
        if !self.medium.is_connectable(peer.radio) {
            return DialOutcome::UnknownPeer;
        }
        if network.connection_count(self.config.address) >= self.config.connection_capacity
            || network.connection_count(address) >= peer.connection_capacity
        {
            return DialOutcome::Busy;
        }
        let lifecycle = Arc::new(Connection::new(self.config.address, address));
        let (mine, theirs) = link_pair(
            self.config.address,
            address,
            self.config.link,
            peer.link,
            self.config.received_signal_strength_dbm,
            peer.received_signal_strength_dbm,
            lifecycle.clone(),
        );
        match peer.inbound.try_send(theirs) {
            Ok(()) => {
                network.connections.push(lifecycle);
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
        let mut network = self
            .network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = close_connections(&mut network, |connection| {
            connection.connects(self.config.address)
        });
        if network
            .peers
            .get(&self.config.address)
            .is_some_and(|peer| peer.radio == self.radio)
        {
            let _ = network.peers.remove(&self.config.address);
        }
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
    endpoint: Arc<ConnectionEndpoint>,
}

impl VirtualBleLink {
    const fn peer_signal_strength(&self) -> i8 {
        self.peer_signal_strength_dbm
    }

    fn closed(&self) -> watch::Receiver<bool> {
        self.endpoint.connection.subscribe()
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
        let mut closed = self.closed();
        if *closed.borrow_and_update() {
            return Err(VirtualBleError::LinkClosed);
        }
        tokio::select! {
            biased;
            _ = closed.changed() => Err(VirtualBleError::LinkClosed),
            result = self.control_tx.send(*message) => {
                result.map_err(|_| VirtualBleError::LinkClosed)
            }
        }
    }

    async fn control_recv(&mut self) -> Result<Control, Self::Error> {
        let mut closed = self.closed();
        if *closed.borrow_and_update() {
            return Err(VirtualBleError::LinkClosed);
        }
        tokio::select! {
            biased;
            _ = closed.changed() => Err(VirtualBleError::LinkClosed),
            message = self.control_rx.recv() => message.ok_or(VirtualBleError::LinkClosed),
        }
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), Self::Error> {
        if self.endpoint.connection.is_closed() {
            Err(VirtualBleError::LinkClosed)
        } else {
            Ok(())
        }
    }

    fn into_data(self) -> (Self::Source, Self::Sink) {
        (
            VirtualBleSource {
                receiver: self.data_rx,
                endpoint: self.endpoint.clone(),
            },
            VirtualBleSink {
                sender: self.data_tx,
                maximum_frame_length: self.maximum_frame_length,
                endpoint: self.endpoint,
            },
        )
    }
}

pub struct VirtualBleSource {
    receiver: mpsc::Receiver<Vec<u8>>,
    endpoint: Arc<ConnectionEndpoint>,
}

impl BleSource for VirtualBleSource {
    type Error = VirtualBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        let mut closed = self.endpoint.connection.subscribe();
        if *closed.borrow_and_update() {
            return Err(VirtualBleError::LinkClosed);
        }
        let frame = tokio::select! {
            biased;
            _ = closed.changed() => return Err(VirtualBleError::LinkClosed),
            frame = self.receiver.recv() => frame.ok_or(VirtualBleError::LinkClosed)?,
        };
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
    endpoint: Arc<ConnectionEndpoint>,
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
        let mut closed = self.endpoint.connection.subscribe();
        if *closed.borrow_and_update() {
            return Err(VirtualBleError::LinkClosed);
        }
        tokio::select! {
            biased;
            _ = closed.changed() => Err(VirtualBleError::LinkClosed),
            result = self.sender.send(frame.to_vec()) => {
                result.map_err(|_| VirtualBleError::LinkClosed)
            }
        }
    }
}

fn link_pair(
    address_a: BleAddress,
    address_b: BleAddress,
    config_a: VirtualBleLinkConfig,
    config_b: VirtualBleLinkConfig,
    signal_strength_a: i8,
    signal_strength_b: i8,
    lifecycle: Arc<Connection>,
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
            endpoint: Arc::new(ConnectionEndpoint {
                connection: lifecycle.clone(),
            }),
        },
        VirtualBleLink {
            peer_address: address_a,
            control_tx: control_b_tx,
            control_rx: control_a_rx,
            data_tx: data_b_tx,
            data_rx: data_a_rx,
            maximum_frame_length,
            peer_signal_strength_dbm: signal_strength_a,
            endpoint: Arc::new(ConnectionEndpoint {
                connection: lifecycle,
            }),
        },
    )
}

fn close_connections(
    network: &mut ConnectionNetwork,
    matches: impl Fn(&Connection) -> bool,
) -> VirtualBleDisconnectReport {
    let connections_closed = network
        .connections
        .iter()
        .filter(|connection| matches(connection) && connection.close())
        .count();
    network.prune_closed();
    VirtualBleDisconnectReport { connections_closed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_teardown_during_disconnect_does_not_relock_the_registry() {
        let (done, completion) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let connection = Arc::new(Connection::new(
                BleAddress::new([1; 6]),
                BleAddress::new([2; 6]),
            ));
            let endpoint = Mutex::new(Some(ConnectionEndpoint {
                connection: connection.clone(),
            }));
            let registry = Mutex::new(ConnectionNetwork {
                connections: vec![connection],
                ..ConnectionNetwork::default()
            });
            let mut network = registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let report = close_connections(&mut network, |_| {
                // Force teardown while the registry lock and traversal reference are held.
                drop(
                    endpoint
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take(),
                );
                false
            });
            let _ = done.send((report, network.connections.len()));
        });
        assert_eq!(
            completion.recv_timeout(std::time::Duration::from_secs(1)),
            Ok((
                VirtualBleDisconnectReport {
                    connections_closed: 0
                },
                0
            ))
        );
    }
}
