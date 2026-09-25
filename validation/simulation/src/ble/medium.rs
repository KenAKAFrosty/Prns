use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use personal_rns::interfaces::bluetooth_auto::BleAddress;
use tokio::sync::Notify;

use super::advertisement::{BleAdvertisement, BleAdvertisingParameters};
use super::config::BleMediumConfig;
use super::radio_id::RadioIdSequence;
use super::trace::{
    BleObservationDropReason, BleSimulationEvent, BleTraceBuffer, BleTraceSnapshot,
};
use super::{BleRadioId, BleRadioPower, BleScanState};
use crate::topology::Topology;
use crate::{
    Reachability, SimulationDurationInTicks, SimulationTick, TopologyError, TopologyMutation,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleRadioMutation {
    Applied,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleSimulationError {
    RadioCapacityReached,
    RadioIdsExhausted,
    DuplicateAddress,
    UnknownRadio(BleRadioId),
}

impl fmt::Display for BleSimulationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RadioCapacityReached => formatter.write_str("BLE radio capacity reached"),
            Self::RadioIdsExhausted => formatter.write_str("BLE radio identifiers exhausted"),
            Self::DuplicateAddress => formatter.write_str("BLE address is already attached"),
            Self::UnknownRadio(radio) => write!(formatter, "BLE radio {} is unknown", radio.get()),
        }
    }
}

impl std::error::Error for BleSimulationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleAdvanceError {
    BeforeCurrent {
        current: SimulationTick,
        requested: SimulationTick,
    },
    Overflow {
        current: SimulationTick,
        by: SimulationDurationInTicks,
    },
    EmissionBudgetExceeded {
        maximum: usize,
    },
}

impl fmt::Display for BleAdvanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeCurrent { current, requested } => write!(
                formatter,
                "cannot move BLE simulation time backward from {} to {}",
                current.get(),
                requested.get(),
            ),
            Self::Overflow { current, by } => write!(
                formatter,
                "advancing BLE simulation time {} by {} ticks overflows",
                current.get(),
                by.get(),
            ),
            Self::EmissionBudgetExceeded { maximum } => write!(
                formatter,
                "BLE advance exceeds its bounded budget of {maximum} advertisement emissions",
            ),
        }
    }
}

impl std::error::Error for BleAdvanceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleObservation {
    pub advertiser: BleRadioId,
    pub address: BleAddress,
    pub received_signal_strength_dbm: i8,
    pub at: SimulationTick,
    pub advertisement: BleAdvertisement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleAdvanceReport {
    pub from: SimulationTick,
    pub to: SimulationTick,
    pub advertisements_emitted: usize,
    pub observations_queued: usize,
    pub observations_dropped: usize,
}

struct ActiveAdvertisement {
    parameters: BleAdvertisingParameters,
    next_emission: Option<SimulationTick>,
}

struct Radio {
    address: BleAddress,
    received_signal_strength_dbm: i8,
    power: BleRadioPower,
    scanning: BleScanState,
    advertising: Option<ActiveAdvertisement>,
    observations: VecDeque<BleObservation>,
    observation_ready: Arc<Notify>,
}

struct BleMediumState {
    topology: Topology<BleRadioId>,
    max_radios: usize,
    observation_capacity: usize,
    max_emissions_per_advance: usize,
    now: SimulationTick,
    radio_ids: RadioIdSequence,
    addresses: BTreeSet<BleAddress>,
    radios: BTreeMap<BleRadioId, Radio>,
    trace: BleTraceBuffer,
}

#[derive(Clone)]
pub struct VirtualBleMedium {
    state: Arc<Mutex<BleMediumState>>,
}

impl VirtualBleMedium {
    #[must_use]
    pub fn new(config: BleMediumConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(BleMediumState {
                topology: Topology::new(config.topology),
                max_radios: config.max_radios,
                observation_capacity: config.observation_queue,
                max_emissions_per_advance: config.max_emissions_per_advance,
                now: SimulationTick::ZERO,
                radio_ids: RadioIdSequence::new(),
                addresses: BTreeSet::new(),
                radios: BTreeMap::new(),
                trace: BleTraceBuffer::new(config.trace_capacity),
            })),
        }
    }

    pub fn attach(
        &self,
        address: BleAddress,
        received_signal_strength_dbm: i8,
    ) -> Result<BleRadioId, BleSimulationError> {
        let mut state = self.lock_state();
        if state.radios.len() == state.max_radios {
            return Err(BleSimulationError::RadioCapacityReached);
        }
        if state.addresses.contains(&address) {
            return Err(BleSimulationError::DuplicateAddress);
        }
        let radio = state.radio_ids.issue()?;
        let _ = state.addresses.insert(address);
        let observation_capacity = state.observation_capacity;
        let replaced = state.radios.insert(
            radio,
            Radio {
                address,
                received_signal_strength_dbm,
                power: BleRadioPower::Off,
                scanning: BleScanState::Off,
                advertising: None,
                observations: VecDeque::with_capacity(observation_capacity),
                observation_ready: Arc::new(Notify::new()),
            },
        );
        debug_assert!(replaced.is_none(), "fresh BLE radio id must be vacant");
        state.topology.attach(radio);
        state
            .trace
            .push(BleSimulationEvent::RadioAttached { radio });
        Ok(radio)
    }

    #[must_use]
    pub fn now(&self) -> SimulationTick {
        self.lock_state().now
    }

    pub(crate) fn is_powered(&self, radio: BleRadioId) -> bool {
        self.lock_state()
            .radios
            .get(&radio)
            .is_some_and(|radio| radio.power == BleRadioPower::On)
    }

    pub(crate) fn is_connectable(&self, from: BleRadioId, radio: BleRadioId) -> bool {
        let state = self.lock_state();
        state.topology.reaches(from, radio)
            && state.radios.get(&radio).is_some_and(|radio| {
                radio.power == BleRadioPower::On && radio.advertising.is_some()
            })
    }

    pub fn set_reachability(
        &self,
        first: BleRadioId,
        second: BleRadioId,
        reachability: Reachability,
    ) -> Result<TopologyMutation, TopologyError<BleRadioId>> {
        let mut state = self.lock_state();
        let mutation = state
            .topology
            .set_reachability(first, second, reachability)?;
        if mutation == TopologyMutation::Applied {
            let at = state.now;
            state.trace.push(BleSimulationEvent::ReachabilityChanged {
                first,
                second,
                reachability,
                at,
            });
        }
        Ok(mutation)
    }

    pub fn set_radio_power(
        &self,
        radio: BleRadioId,
        power: BleRadioPower,
    ) -> Result<BleRadioMutation, BleSimulationError> {
        let mut state = self.lock_state();
        let now = state.now;
        let attached = state
            .radios
            .get_mut(&radio)
            .ok_or(BleSimulationError::UnknownRadio(radio))?;
        if attached.power == power {
            return Ok(BleRadioMutation::Unchanged);
        }
        attached.power = power;
        if power == BleRadioPower::On {
            if let Some(advertising) = &mut attached.advertising {
                advertising.next_emission = Some(now);
            }
        } else {
            attached.observations.clear();
        }
        state
            .trace
            .push(BleSimulationEvent::RadioPowerChanged { radio, power });
        Ok(BleRadioMutation::Applied)
    }

    pub fn set_scanning(
        &self,
        radio: BleRadioId,
        scanning: BleScanState,
    ) -> Result<BleRadioMutation, BleSimulationError> {
        let mut state = self.lock_state();
        let attached = state
            .radios
            .get_mut(&radio)
            .ok_or(BleSimulationError::UnknownRadio(radio))?;
        if attached.scanning == scanning {
            return Ok(BleRadioMutation::Unchanged);
        }
        attached.scanning = scanning;
        state
            .trace
            .push(BleSimulationEvent::ScanningChanged { radio, scanning });
        Ok(BleRadioMutation::Applied)
    }

    pub fn set_advertising(
        &self,
        radio: BleRadioId,
        parameters: Option<BleAdvertisingParameters>,
    ) -> Result<BleRadioMutation, BleSimulationError> {
        let mut state = self.lock_state();
        let now = state.now;
        let attached = state
            .radios
            .get_mut(&radio)
            .ok_or(BleSimulationError::UnknownRadio(radio))?;
        if attached
            .advertising
            .as_ref()
            .map(|active| active.parameters)
            == parameters
        {
            return Ok(BleRadioMutation::Unchanged);
        }
        attached.advertising = parameters.map(|parameters| ActiveAdvertisement {
            parameters,
            next_emission: Some(now),
        });
        state.trace.push(BleSimulationEvent::AdvertisingChanged {
            radio,
            advertisement: parameters.map(BleAdvertisingParameters::advertisement),
            interval: parameters.map(BleAdvertisingParameters::interval),
        });
        Ok(BleRadioMutation::Applied)
    }

    pub fn take_observation(
        &self,
        radio: BleRadioId,
    ) -> Result<Option<BleObservation>, BleSimulationError> {
        self.lock_state()
            .radios
            .get_mut(&radio)
            .ok_or(BleSimulationError::UnknownRadio(radio))
            .map(|attached| attached.observations.pop_front())
    }

    pub async fn next_observation(
        &self,
        radio: BleRadioId,
    ) -> Result<BleObservation, BleSimulationError> {
        loop {
            let notified = {
                let mut state = self.lock_state();
                let attached = state
                    .radios
                    .get_mut(&radio)
                    .ok_or(BleSimulationError::UnknownRadio(radio))?;
                if let Some(observation) = attached.observations.pop_front() {
                    return Ok(observation);
                }
                attached.observation_ready.clone().notified_owned()
            };
            notified.await;
        }
    }

    pub(crate) fn detach(&self, radio: BleRadioId) {
        let mut state = self.lock_state();
        let Some(detached) = state.radios.remove(&radio) else {
            return;
        };
        let _ = state.addresses.remove(&detached.address);
        state.topology.detach(radio);
        state
            .trace
            .push(BleSimulationEvent::RadioDetached { radio });
        drop(state);
        detached.observation_ready.notify_waiters();
    }

    pub fn advance_by(
        &self,
        by: SimulationDurationInTicks,
    ) -> Result<BleAdvanceReport, BleAdvanceError> {
        let mut state = self.lock_state();
        let requested = state.now.checked_add(by).ok_or(BleAdvanceError::Overflow {
            current: state.now,
            by,
        })?;
        advance_locked(&mut state, requested)
    }

    pub fn advance_to(
        &self,
        requested: SimulationTick,
    ) -> Result<BleAdvanceReport, BleAdvanceError> {
        advance_locked(&mut self.lock_state(), requested)
    }

    #[must_use]
    pub fn trace(&self) -> BleTraceSnapshot {
        self.lock_state().trace.snapshot()
    }

    fn lock_state(&self) -> MutexGuard<'_, BleMediumState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn advance_locked(
    state: &mut BleMediumState,
    requested: SimulationTick,
) -> Result<BleAdvanceReport, BleAdvanceError> {
    if requested < state.now {
        return Err(BleAdvanceError::BeforeCurrent {
            current: state.now,
            requested,
        });
    }
    let emissions = planned_emissions(state, requested)?;
    let from = state.now;
    if requested > from {
        state.now = requested;
        state.trace.push(BleSimulationEvent::TimeAdvanced {
            from,
            to: requested,
        });
    }
    let mut report = BleAdvanceReport {
        from,
        to: requested,
        advertisements_emitted: emissions.len(),
        observations_queued: 0,
        observations_dropped: 0,
    };
    for (at, advertiser) in emissions {
        settle_emission(state, advertiser, at, &mut report);
    }
    Ok(report)
}

fn planned_emissions(
    state: &BleMediumState,
    requested: SimulationTick,
) -> Result<Vec<(SimulationTick, BleRadioId)>, BleAdvanceError> {
    let mut emissions = Vec::new();
    for (radio_id, radio) in &state.radios {
        if radio.power != BleRadioPower::On {
            continue;
        }
        let Some(active) = &radio.advertising else {
            continue;
        };
        let Some(mut at) = active.next_emission else {
            continue;
        };
        while at <= requested {
            if emissions.len() == state.max_emissions_per_advance {
                return Err(BleAdvanceError::EmissionBudgetExceeded {
                    maximum: state.max_emissions_per_advance,
                });
            }
            emissions.push((at, *radio_id));
            let Some(next) = at.checked_add(active.parameters.interval()) else {
                break;
            };
            at = next;
        }
    }
    emissions.sort_unstable();
    Ok(emissions)
}

fn settle_emission(
    state: &mut BleMediumState,
    advertiser: BleRadioId,
    at: SimulationTick,
    report: &mut BleAdvanceReport,
) {
    let Some(source) = state.radios.get(&advertiser) else {
        return;
    };
    let Some(active) = &source.advertising else {
        return;
    };
    let parameters = active.parameters;
    let advertisement = parameters.advertisement();
    let address = source.address;
    let received_signal_strength_dbm = source.received_signal_strength_dbm;
    state.trace.push(BleSimulationEvent::AdvertisementEmitted {
        radio: advertiser,
        at,
        advertisement,
    });

    let neighbors: Vec<_> = state.topology.neighbors(advertiser).collect();
    for scanner in neighbors {
        let Some(receiver) = state.radios.get_mut(&scanner) else {
            continue;
        };
        if receiver.power != BleRadioPower::On || receiver.scanning != BleScanState::On {
            continue;
        }
        if receiver.observations.len() == state.observation_capacity {
            report.observations_dropped = report.observations_dropped.saturating_add(1);
            state.trace.push(BleSimulationEvent::ObservationDropped {
                advertiser,
                scanner,
                at,
                reason: BleObservationDropReason::ObservationQueueFull,
            });
        } else {
            receiver.observations.push_back(BleObservation {
                advertiser,
                address,
                received_signal_strength_dbm,
                at,
                advertisement,
            });
            receiver.observation_ready.notify_one();
            report.observations_queued = report.observations_queued.saturating_add(1);
            state.trace.push(BleSimulationEvent::ObservationQueued {
                advertiser,
                scanner,
                at,
            });
        }
    }

    if let Some(active) = state
        .radios
        .get_mut(&advertiser)
        .and_then(|source| source.advertising.as_mut())
    {
        active.next_emission = at.checked_add(parameters.interval());
    }
}
