#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::radios::{BandRadioError, ReceivedAirFrame};
use embassy_futures::{block_on, join::join, yield_now};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use prns_core::interfaces::lora::{LoRaConfiguration, GHZ24_BALANCED_PROFILE};
use prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;
use prns_core::interfaces::{FrameSink, InterfaceStatus};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    vec::Vec,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Initialize,
    Idle,
    ArmRx,
    Transmit,
    Rssi,
    Read,
    Poll,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Recoverable,
    ResetRequired,
}

struct Lab {
    events: Channel<CriticalSectionRawMutex, (RadioEvent, Vec<u8>), 4>,
    outbound: Channel<CriticalSectionRawMutex, Vec<u8>, 4>,
    tx_started: Signal<CriticalSectionRawMutex, ()>,
    tx_release: Signal<CriticalSectionRawMutex, ()>,
    pause_tx: Cell<bool>,
    initialized: Cell<Option<RadioProfile>>,
    receiving: Cell<bool>,
    transmissions: RefCell<Vec<(RadioProfile, Vec<u8>)>>,
    transmission_times: RefCell<Vec<u64>>,
    clock_ms: Cell<u64>,
    channel_ids: RefCell<Vec<InterfaceId>>,
    dispositions: RefCell<Vec<OutboundDisposition>>,
    delivered: RefCell<Vec<Vec<u8>>>,
    received_phy: RefCell<Vec<PacketPhyStats>>,
    accepted: Cell<usize>,
    fault: Cell<Option<(Operation, Fault)>>,
    initializations: Cell<usize>,
    idle_failures: Cell<usize>,
    reject_profile: Cell<bool>,
    seam_drops: Cell<u32>,
    fault_after_poll: Cell<Option<Fault>>,
    polled_frame: RefCell<Option<Vec<u8>>>,
}
impl Lab {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            events: Channel::new(),
            outbound: Channel::new(),
            tx_started: Signal::new(),
            tx_release: Signal::new(),
            pause_tx: Cell::new(false),
            initialized: Cell::new(None),
            receiving: Cell::new(false),
            transmissions: RefCell::new(Vec::new()),
            transmission_times: RefCell::new(Vec::new()),
            clock_ms: Cell::new(0),
            channel_ids: RefCell::new(Vec::new()),
            dispositions: RefCell::new(Vec::new()),
            delivered: RefCell::new(Vec::new()),
            received_phy: RefCell::new(Vec::new()),
            accepted: Cell::new(0),
            fault: Cell::new(None),
            initializations: Cell::new(0),
            idle_failures: Cell::new(0),
            reject_profile: Cell::new(false),
            seam_drops: Cell::new(0),
            fault_after_poll: Cell::new(None),
            polled_frame: RefCell::new(None),
        })
    }
    fn check(&self, operation: Operation) -> Result<(), Fault> {
        if operation == Operation::Idle && self.idle_failures.get() > 0 {
            self.idle_failures.set(self.idle_failures.get() - 1);
            return Err(Fault::ResetRequired);
        }
        if let Some((expected, fault)) = self.fault.get() {
            if expected == operation {
                self.fault.set(None);
                return Err(fault);
            }
        }
        Ok(())
    }
    async fn receive(&self, bytes: &[u8]) {
        self.events
            .send((
                RadioEvent::Frame(ReceivedAirFrame {
                    len: bytes.len(),
                    phy: PacketPhyStats::default(),
                }),
                bytes.to_vec(),
            ))
            .await;
    }
}
struct Radio(Rc<Lab>);
impl LoRaRadio for Radio {
    type Error = Fault;
    fn validate_profile(
        &self,
        _: prns_core::interfaces::lora::RadioProfile,
    ) -> Result<(), RadioProfileCompatibilityError> {
        Ok(())
    }
    fn validate_band_profile(&self, _: RadioProfile) -> Result<(), RadioProfileCompatibilityError> {
        if self.0.reject_profile.get() {
            Err(RadioProfileCompatibilityError::UnsupportedBand)
        } else {
            Ok(())
        }
    }
    fn recovery(error: &Self::Error) -> RadioRecovery {
        match error {
            Fault::Recoverable => RadioRecovery::Continue,
            Fault::ResetRequired => RadioRecovery::Reinitialize,
        }
    }
    async fn initialize(
        &mut self,
        profile: prns_core::interfaces::lora::RadioProfile,
    ) -> Result<(), Self::Error> {
        self.0.check(Operation::Initialize)?;
        self.0.initializations.set(self.0.initializations.get() + 1);
        self.0.initialized.set(Some(profile.into()));
        Ok(())
    }
    async fn initialize_band(
        &mut self,
        profile: RadioProfile,
    ) -> Result<(), BandRadioError<Self::Error>> {
        self.0
            .check(Operation::Initialize)
            .map_err(BandRadioError::Radio)?;
        self.0.initializations.set(self.0.initializations.get() + 1);
        self.0.initialized.set(Some(profile));
        self.0.receiving.set(false);
        Ok(())
    }
    async fn idle(&mut self) -> Result<(), Self::Error> {
        self.0.check(Operation::Idle)?;
        self.0.receiving.set(false);
        Ok(())
    }
    async fn arm_rx(&mut self) -> Result<(), Self::Error> {
        self.0.check(Operation::ArmRx)?;
        self.0.receiving.set(true);
        Ok(())
    }
    async fn transmit(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.check(Operation::Transmit)?;
        let Some(profile) = self.0.initialized.get() else {
            panic!("transmit requires initialization");
        };
        self.0
            .transmissions
            .borrow_mut()
            .push((profile, bytes.to_vec()));
        self.0
            .transmission_times
            .borrow_mut()
            .push(self.0.clock_ms.get());
        self.0.receiving.set(false);
        if self.0.pause_tx.replace(false) {
            self.0.tx_started.signal(());
            self.0.tx_release.wait().await;
        }
        Ok(())
    }
    async fn channel_rssi_dbm(&mut self) -> Result<i16, Self::Error> {
        self.0.check(Operation::Rssi)?;
        Ok(-120)
    }
    async fn read_event(&mut self, buffer: &mut [u8]) -> Result<RadioEvent, Self::Error> {
        let (event, bytes) = self.0.events.receive().await;
        self.0.check(Operation::Read)?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(event)
    }
    async fn poll_event(&mut self, buffer: &mut [u8]) -> Result<Option<RadioEvent>, Self::Error> {
        self.0.check(Operation::Poll)?;
        if let Some(fault) = self.0.fault_after_poll.take() {
            self.0.fault.set(Some((Operation::Rssi, fault)));
        }
        if let Some(bytes) = self.0.polled_frame.borrow_mut().take() {
            buffer[..bytes.len()].copy_from_slice(&bytes);
            return Ok(Some(RadioEvent::Frame(ReceivedAirFrame {
                len: bytes.len(),
                phy: PacketPhyStats::default(),
            })));
        }
        let Ok((event, bytes)) = self.0.events.try_receive() else {
            return Ok(None);
        };
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(Some(event))
    }
}
struct Seam {
    lab: Rc<Lab>,
    current: Vec<u8>,
    sink: Vec<u8>,
}
impl InterfaceSeam for Seam {
    fn set_channel_id(&mut self, id: InterfaceId) {
        self.lab.channel_ids.borrow_mut().push(id);
    }
    fn take_channel_change_drops(&mut self) -> u32 {
        self.lab.seam_drops.replace(0)
    }
    fn fill_random(&mut self, output: &mut [u8]) {
        output.fill(0);
    }
    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        &mut self.sink
    }
    async fn commit_inbound(&mut self) {
        self.lab
            .delivered
            .borrow_mut()
            .push(core::mem::take(&mut self.sink));
    }
    async fn next_inbound_with_phy(&mut self, frame: &[u8], phy: PacketPhyStats) {
        self.lab.delivered.borrow_mut().push(frame.to_vec());
        self.lab.received_phy.borrow_mut().push(phy);
    }
    async fn next_outbound(&mut self) -> &[u8] {
        self.current = self.lab.outbound.receive().await;
        &self.current
    }
    fn accept_outbound_custody(&mut self) {
        self.lab.accepted.set(self.lab.accepted.get() + 1);
    }
    fn complete_outbound(&mut self, disposition: OutboundDisposition) {
        self.lab.dispositions.borrow_mut().push(disposition);
    }
}

mod configuration;
mod traffic;

mod deadlines;
