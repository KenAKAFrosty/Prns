//! Embedded SX126x LoRa transport with bounded spectrum access.
//!
//! [`LoRaInterfaceInput`] requires an [`AirtimePolicy`], a
//! [`LoRaSpectrumStatus`], and caller-owned transmit queue storage, with
//! [`LORA_TX_QUEUE_BYTES`] as the general-purpose default. Construction validates
//! frequency, transmit power, preamble,
//! and any fixed airtime limit before the radio task can start.
//! [`AirtimePolicy::Regional`] is the normal choice; a fixed policy may tighten
//! a regional limit but cannot weaken one.
//!
//! The radio runs in continuous receive and combines preamble/header IRQ
//! evidence, an adaptive RSSI noise floor, real two-slot DIFS, a fine-grained
//! randomized ticket, and a final IRQ-plus-RSSI check immediately before
//! transmit. Channel activity restarts DIFS but permanently freezes completed
//! ticket progress. Successfully decoded peer airtime earns bounded `1x..=3x`
//! countdown acceleration; preamble, RSSI, CRC-error, and header-error evidence
//! remains conservatively busy without earning priority.
//!
//! A winner drains complete FIFO packets into an airtime-bounded opportunity.
//! The profile-derived limit is an integral number of maximum split packets and
//! at least 42 contention slots. The radio releases immediately when the FIFO
//! empties or the next logical packet would cross that limit. Split Reticulum
//! packets remain indivisible and contiguous on air for RNode interoperability.
//! A capped winner re-contends from band zero with one quantum of earned age;
//! existing waiters retain their smaller residual tickets.
//!
//! [`LoRaSpectrumStatus::snapshot`] exposes sampled channel occupancy, noise and
//! CCA levels, deferrals, false preambles, contention and duty drops, and radio
//! recoveries. These diagnostics are observational; they do not provide a
//! listen-before-talk bypass.
//!
//! The active packet remains separate from the packed 6 KiB FIFO, and its
//! contention timeout begins only when it becomes active. Scheduler additions
//! are scalar state only: no heap allocation or additional packet buffers. A
//! one-slot manifold lane provides the ingress handoff. ESP32-S3 Hopspots place
//! the FIFO in PSRAM, while T-Echo supplies static SRAM storage.

use embassy_futures::select::{
    select, select3, select4, select5, Either, Either3, Either4, Either5,
};
use embassy_sync::channel::DynamicSender;
#[cfg(test)]
use embassy_time::Timer;
use embassy_time::{Duration, Instant};
use heapless::Vec as HeaplessVec;
use portable_atomic::{AtomicU32, Ordering};

use prns_core::engine::InstantMillis;
use prns_core::interfaces::lora::{
    self, air_frame_count, encode_air_frame_part, AirFrameError, AirtimePolicy, AirtimePolicyError,
    LoRaConfigurationState, LoRaProfile as RadioProfile, LoRaProfileError as RadioProfileError,
    LoRaReassembler, LoRaReassemblyError, LoRaReassemblyOutcome, LoRaReassemblyReset,
    RadioProfileCompatibilityError, SpreadingFactor, CHANNEL_TAG_CAP, LORA_MAX_PAYLOAD,
    LORA_SINGLE_FRAME_MAX,
};
use prns_core::interfaces::subghz::SubGConfigurationState;
use prns_core::interfaces::{
    AirtimeDutyCycle, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind,
    PacketPhyStats,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::{
    EmbassyInterfaceStatus, InterfaceLifecycle, InterfacePublicationOutcome,
};
use prns_runtime::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use prns_runtime::manifold::throughput::ThroughputLedger;

use crate::radios::{LoRaRadio, RadioEvent, RadioRecovery};

mod airtime_quantum;
mod channel_access;
mod clock;
mod configuration;
mod control;
#[cfg(all(test, feature = "lora-2g4"))]
mod run_tests;
#[cfg(test)]
mod simulation_tests;
mod task;
mod transmit_queue;

use clock::{EmbassyClock, RadioClock};
use control::LoRaConfigurationCommand;
pub use control::{LoRaApplyOutcome, LoRaControl, LoRaControlTarget, LoRaController};

use airtime_quantum::ServiceAge;
use channel_access::{
    ChannelAccess, ChannelAccessAction, ChannelObservation, ChannelTiming, ContentionPriority,
    DemodulatorActivity, NoiseFloor,
};
use transmit_queue::{TransmitQueue, TransmitQueueError};

const IDLE_TICK: Duration = Duration::from_millis(250);
const SENSING_UNPUBLISHED: u32 = u32::MAX;
const UNCONFIGURED_CHANNEL_TAG: &[u8] = b"SubGSetup";
pub const LORA_TX_QUEUE_BYTES: usize = 6 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum PacketPlacement {
    Active,
    Queued,
}

struct ActivePacket {
    bytes: [u8; LORA_MAX_PAYLOAD],
    len: Option<usize>,
    airtime_us: u64,
    activated_at_ms: u64,
}

impl ActivePacket {
    const fn new() -> Self {
        Self {
            bytes: [0; LORA_MAX_PAYLOAD],
            len: None,
            airtime_us: 0,
            activated_at_ms: 0,
        }
    }

    fn activate(&mut self, packet: &[u8], profile: &RadioProfile, now_ms: u64) {
        self.bytes[..packet.len()].copy_from_slice(packet);
        self.len = Some(packet.len());
        self.airtime_us = packet_airtime(packet, profile);
        self.activated_at_ms = now_ms;
    }

    fn clear(&mut self) -> bool {
        self.len.take().is_some()
    }

    fn recompute_airtime(&mut self, profile: &RadioProfile) {
        let Some(len) = self.len else {
            return;
        };
        self.airtime_us = packet_airtime(&self.bytes[..len], profile);
    }

    fn channel_access(
        &self,
        profile: RadioProfile,
        now_ms: u64,
        priority: ContentionPriority,
    ) -> Option<ChannelAccess> {
        self.len.map(|_| {
            ChannelAccess::new_at(
                profile,
                now_ms,
                self.activated_at_ms,
                self.airtime_us,
                priority,
            )
        })
    }
}

struct TransmitBacklog<'a> {
    queue: TransmitQueue<'a>,
    active: ActivePacket,
}

impl<'a> TransmitBacklog<'a> {
    fn new(storage: &'a mut [u8]) -> Self {
        Self {
            queue: TransmitQueue::new(storage),
            active: ActivePacket::new(),
        }
    }

    fn accept(
        &mut self,
        packet: &[u8],
        profile: &RadioProfile,
        now_ms: u64,
    ) -> Result<PacketPlacement, TransmitQueueError> {
        if packet.len() > LORA_MAX_PAYLOAD {
            return Err(TransmitQueueError::PacketTooLarge);
        }
        if self.active.len.is_none() && self.queue.is_empty() {
            self.active.activate(packet, profile, now_ms);
            return Ok(PacketPlacement::Active);
        }
        self.queue.push(packet)?;
        Ok(PacketPlacement::Queued)
    }

    fn activate_next(&mut self, profile: &RadioProfile, now_ms: u64) -> bool {
        if self.active.len.is_some() {
            return false;
        }
        let Some(len) = self.queue.pop(&mut self.active.bytes) else {
            return false;
        };
        self.active.len = Some(len);
        self.active.airtime_us = packet_airtime(&self.active.bytes[..len], profile);
        self.active.activated_at_ms = now_ms;
        true
    }

    const fn can_accept_outbound(&self) -> bool {
        if self.active.len.is_none() && self.queue.is_empty() {
            true
        } else {
            self.queue.can_push_max_packet()
        }
    }

    const fn has_pending(&self) -> bool {
        self.active.len.is_some() || !self.queue.is_empty()
    }

    fn clear(&mut self) -> usize {
        let mut cleared = usize::from(self.active.clear());
        while self.queue.pop(&mut self.active.bytes).is_some() {
            cleared += 1;
        }
        cleared
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoRaSpectrumSnapshot {
    pub channel_busy_per_mille: u16,
    pub noise_floor_dbm: Option<i16>,
    pub cca_threshold_dbm: Option<i16>,
    pub deferrals: u32,
    pub false_preambles: u32,
    pub contention_timeouts: u32,
    pub duty_holds: u32,
    pub duty_timeouts: u32,
    pub radio_recoveries: u32,
    pub channel_change_drops: u32,
}

/// Lock-free spectrum-stewardship diagnostics for one LoRa interface.
pub struct LoRaSpectrumStatus {
    channel_observations: AtomicU32,
    busy_observations: AtomicU32,
    sensing: AtomicU32,
    deferrals: AtomicU32,
    false_preambles: AtomicU32,
    contention_timeouts: AtomicU32,
    duty_holds: AtomicU32,
    duty_timeouts: AtomicU32,
    radio_recoveries: AtomicU32,
    channel_change_drops: AtomicU32,
}

impl Default for LoRaSpectrumStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl LoRaSpectrumStatus {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            channel_observations: AtomicU32::new(0),
            busy_observations: AtomicU32::new(0),
            sensing: AtomicU32::new(SENSING_UNPUBLISHED),
            deferrals: AtomicU32::new(0),
            false_preambles: AtomicU32::new(0),
            contention_timeouts: AtomicU32::new(0),
            duty_holds: AtomicU32::new(0),
            duty_timeouts: AtomicU32::new(0),
            radio_recoveries: AtomicU32::new(0),
            channel_change_drops: AtomicU32::new(0),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> LoRaSpectrumSnapshot {
        let observations = self.channel_observations.load(Ordering::Relaxed);
        let busy = self.busy_observations.load(Ordering::Relaxed);
        let channel_busy_per_mille = if observations == 0 {
            0
        } else {
            busy.saturating_mul(1_000).saturating_div(observations) as u16
        };
        let sensing = self.sensing.load(Ordering::Relaxed);
        let (noise_floor_dbm, cca_threshold_dbm) = if sensing == SENSING_UNPUBLISHED {
            (None, None)
        } else {
            (
                Some((sensing >> 16) as u16 as i16),
                Some(sensing as u16 as i16),
            )
        };
        LoRaSpectrumSnapshot {
            channel_busy_per_mille,
            noise_floor_dbm,
            cca_threshold_dbm,
            deferrals: self.deferrals.load(Ordering::Relaxed),
            false_preambles: self.false_preambles.load(Ordering::Relaxed),
            contention_timeouts: self.contention_timeouts.load(Ordering::Relaxed),
            duty_holds: self.duty_holds.load(Ordering::Relaxed),
            duty_timeouts: self.duty_timeouts.load(Ordering::Relaxed),
            radio_recoveries: self.radio_recoveries.load(Ordering::Relaxed),
            channel_change_drops: self.channel_change_drops.load(Ordering::Relaxed),
        }
    }

    fn record_channel(&self, observation: ChannelObservation, noise: Option<&NoiseFloor>) {
        match observation {
            ChannelObservation::Clear => {
                self.channel_observations.fetch_add(1, Ordering::Relaxed);
            }
            ChannelObservation::Busy => {
                self.channel_observations.fetch_add(1, Ordering::Relaxed);
                self.busy_observations.fetch_add(1, Ordering::Relaxed);
            }
            ChannelObservation::Unknown => {}
        }
        if let Some((floor, threshold)) =
            noise.and_then(|noise| noise.noise_floor_dbm().zip(noise.cca_threshold_dbm()))
        {
            let packed = (u32::from(floor as u16) << 16) | u32::from(threshold as u16);
            self.sensing.store(packed, Ordering::Relaxed);
        }
    }

    fn add_deferrals(&self, count: u32) {
        self.deferrals.fetch_add(count, Ordering::Relaxed);
    }

    fn add_false_preamble(&self) {
        self.false_preambles.fetch_add(1, Ordering::Relaxed);
    }

    fn add_contention_timeout(&self) {
        self.contention_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    fn add_duty_hold(&self) {
        self.duty_holds.fetch_add(1, Ordering::Relaxed);
    }

    fn add_duty_timeout(&self) {
        self.duty_timeouts.fetch_add(1, Ordering::Relaxed);
    }

    fn add_radio_recovery(&self) {
        self.radio_recoveries.fetch_add(1, Ordering::Relaxed);
    }
}

struct ObservedAirFrame<'a> {
    bytes: &'a [u8],
    phy: PacketPhyStats,
    spreading_factor: SpreadingFactor,
    arrived_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChannelEvidence {
    observation: ChannelObservation,
    decoded_airtime_us: Option<u64>,
}

struct ReceivePath<'a, Seam> {
    profile: &'a RadioProfile,
    activity: &'a mut DemodulatorActivity,
    spectrum: &'a LoRaSpectrumStatus,
    rx_buf: &'a [u8],
    status: &'a EmbassyInterfaceStatus,
    throughput: &'a mut ThroughputLedger,
    reassembler: &'a mut LoRaReassembler<LORA_MAX_PAYLOAD>,
    seam: &'a mut Seam,
}

fn decoded_peer_airtime_us(event: RadioEvent, profile: &RadioProfile) -> Option<u64> {
    match event {
        RadioEvent::Frame(received) => Some(profile.time_on_air_us(received.len)),
        RadioEvent::PreambleDetected
        | RadioEvent::HeaderValid
        | RadioEvent::HeaderError
        | RadioEvent::CrcError
        | RadioEvent::Timeout
        | RadioEvent::SpuriousInterrupt => None,
    }
}

fn reset_reassembly(
    reassembler: &mut LoRaReassembler<LORA_MAX_PAYLOAD>,
    status: &EmbassyInterfaceStatus,
) {
    if reassembler.reset() == LoRaReassemblyReset::DiscardedPartial {
        status.count_frame_undecodable();
    }
}

async fn deliver_rx<Seam: InterfaceSeam>(
    frame: ObservedAirFrame<'_>,
    status: &EmbassyInterfaceStatus,
    throughput: &mut ThroughputLedger,
    reassembler: &mut LoRaReassembler<LORA_MAX_PAYLOAD>,
    seam: &mut Seam,
) {
    status.count_frame_in();
    status.add_rx(frame.bytes.len() as u64);
    throughput.record_rx(frame.arrived_at, frame.bytes.len() as u64);
    status.set_transfer_rates(throughput.rates());
    let packet = match reassembler.feed_with_phy(frame.bytes, frame.phy) {
        LoRaReassemblyOutcome::AwaitingSecond => return,
        LoRaReassemblyOutcome::Delivered(packet) => packet,
        LoRaReassemblyOutcome::ReplacedPartialAndAwaitingSecond => {
            status.count_frame_undecodable();
            return;
        }
        LoRaReassemblyOutcome::DeliveredAfterReplacingPartial(packet) => {
            status.count_frame_undecodable();
            packet
        }
        LoRaReassemblyOutcome::Rejected(LoRaReassemblyError::EmptyAirFrame) => {
            status.count_frame_malformed();
            return;
        }
        LoRaReassemblyOutcome::Rejected(LoRaReassemblyError::CapacityExceeded) => {
            status.count_frame_undecodable();
            return;
        }
    };
    if packet.bytes.is_empty() {
        status.count_frame_malformed();
        return;
    }
    debug_assert!(packet.bytes.len() <= LORA_MAX_PAYLOAD);
    let mut phy = packet.phy;
    if let Some(snr) = phy.snr {
        phy.quality = frame.spreading_factor.signal_quality(snr);
    }
    seam.next_inbound_with_phy(packet.bytes, phy).await;
    status.count_frame_delivered();
}

fn choose_backoff_entropy<Seam: InterfaceSeam>(
    access: &mut ChannelAccess,
    seam: &mut Seam,
) -> ChannelAccessAction {
    let mut entropy = [0u8; 2];
    seam.fill_random(&mut entropy);
    access.choose_backoff(u16::from_le_bytes(entropy));
    access.after_entropy()
}

async fn observe_radio_event<Seam: InterfaceSeam>(
    event: RadioEvent,
    now: InstantMillis,
    receive: ReceivePath<'_, Seam>,
) -> ChannelEvidence {
    let decoded_airtime_us = decoded_peer_airtime_us(event, receive.profile);
    let observation = match event {
        RadioEvent::PreambleDetected => {
            receive.activity.preamble_detected(now.0, *receive.profile);
            ChannelObservation::Busy
        }
        RadioEvent::HeaderValid => {
            receive.activity.header_valid(now.0, *receive.profile);
            ChannelObservation::Busy
        }
        RadioEvent::Frame(received) => {
            deliver_rx(
                ObservedAirFrame {
                    bytes: &receive.rx_buf[..received.len],
                    phy: received.phy,
                    spreading_factor: receive.profile.modulation().spreading_factor(),
                    arrived_at: now,
                },
                receive.status,
                receive.throughput,
                receive.reassembler,
                receive.seam,
            )
            .await;
            receive.activity.frame_finished();
            ChannelObservation::Busy
        }
        RadioEvent::HeaderError => {
            receive.spectrum.add_false_preamble();
            receive.activity.frame_finished();
            ChannelObservation::Busy
        }
        RadioEvent::CrcError => {
            receive.activity.frame_finished();
            ChannelObservation::Busy
        }
        RadioEvent::Timeout => {
            receive.activity.frame_finished();
            ChannelObservation::Unknown
        }
        RadioEvent::SpuriousInterrupt => ChannelObservation::Unknown,
    };
    receive.spectrum.record_channel(observation, None);
    ChannelEvidence {
        observation,
        decoded_airtime_us,
    }
}

async fn sample_channel<R: LoRaRadio>(
    radio: &mut R,
    now: InstantMillis,
    activity: &mut DemodulatorActivity,
    spectrum: &LoRaSpectrumStatus,
    noise: &mut NoiseFloor,
) -> Result<ChannelObservation, R::Error> {
    let (demodulator_busy, false_preamble) = activity.observe(now.0);
    if false_preamble {
        spectrum.add_false_preamble();
    }
    if demodulator_busy {
        let observation = ChannelObservation::Busy;
        spectrum.record_channel(observation, Some(noise));
        return Ok(observation);
    }
    let rssi_dbm = radio.channel_rssi_dbm().await?;
    let observation = noise.observe(now.0, rssi_dbm, false);
    spectrum.record_channel(observation, Some(noise));
    Ok(observation)
}

fn unconfigured_interface_id() -> InterfaceId {
    InterfaceId::from_channel_tag(InterfaceKind::LoRa, UNCONFIGURED_CHANNEL_TAG)
}

fn packet_airtime(packet: &[u8], profile: &RadioProfile) -> u64 {
    let mut scratch = [0u8; LORA_SINGLE_FRAME_MAX];
    let mut total = 0;
    for index in 0..air_frame_count(packet.len()) {
        if let Ok(n) = encode_air_frame_part(packet, 0, index, &mut scratch) {
            total += profile.time_on_air_us(n);
        }
    }
    total
}

fn take_contention_priority(
    continuation: &mut bool,
    airtime: &mut AirtimeLedger,
    now: InstantMillis,
) -> ContentionPriority {
    if core::mem::take(continuation) {
        ContentionPriority::Continuation
    } else {
        ContentionPriority::Fresh {
            short_airtime_per_mille: airtime.utilization(now).short_per_mille,
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "embedded serve-loop internals pass the loop's split-borrowed locals; bundling awaits an on-hardware validation pass"
)]
async fn transmit_packet<R: LoRaRadio, C: RadioClock>(
    radio: &mut R,
    packet: &[u8],
    seq: &mut u8,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    status: &EmbassyInterfaceStatus,
    profile: &RadioProfile,
    started: &Instant,
    clock: &C,
    tx_frame: &mut [u8; LORA_SINGLE_FRAME_MAX],
) -> Result<(), LoRaTransmitError<R::Error>> {
    for index in 0..air_frame_count(packet.len()) {
        let n = match encode_air_frame_part(packet, *seq, index, tx_frame) {
            Ok(n) => n,
            Err(e) => {
                crate::diagnostic_log::debug!("RNS_LORA frame {index} encode failed: {e:?}");
                *seq = seq.wrapping_add(0x10);
                return Err(LoRaTransmitError::Framing(e));
            }
        };
        if let Err(e) = radio.transmit(&tx_frame[..n]).await {
            crate::diagnostic_log::debug!("RNS_LORA tx failed: {e:?}");
            *seq = seq.wrapping_add(0x10);
            return Err(LoRaTransmitError::Radio(e));
        }
        let completed_at = InstantMillis(clock.now().duration_since(*started).as_millis());
        status.add_tx(n as u64);
        throughput.record_tx(completed_at, n as u64);
        status.set_transfer_rates(throughput.rates());
        status.set_airtime(airtime.record_tx(completed_at, profile.time_on_air_us(n)));
    }
    *seq = seq.wrapping_add(0x10);
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoRaTransmitError<E> {
    Framing(AirFrameError),
    Radio(E),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadioReinitialization {
    Recovered,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadioActivation {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoRaRadioState {
    Unknown,
    Idle,
    Receiving,
}

async fn reinit_radio<R: LoRaRadio>(
    radio: &mut R,
    profile: &RadioProfile,
    spectrum: &LoRaSpectrumStatus,
) -> RadioReinitialization {
    if let Err(e) = radio.initialize_band(*profile).await {
        crate::diagnostic_log::warn!("RNS_LORA hard re-init failed: {e:?}");
        return RadioReinitialization::Failed;
    }
    if let Err(e) = radio.arm_rx().await {
        crate::diagnostic_log::warn!("RNS_LORA re-init RX arm failed: {e:?}");
        return RadioReinitialization::Failed;
    }
    crate::diagnostic_log::warn!("RNS_LORA radio recovered via hard re-init");
    spectrum.add_radio_recovery();
    RadioReinitialization::Recovered
}

fn validate_profile_request<R: LoRaRadio>(
    radio: &R,
    profile: RadioProfile,
    airtime_policy: AirtimePolicy,
) -> Result<Option<AirtimeDutyCycle>, LoRaConfigError> {
    profile.validate().map_err(LoRaConfigError::Profile)?;
    radio
        .validate_band_profile(profile)
        .map_err(LoRaConfigError::RadioCompatibility)?;
    profile
        .airtime_policy(airtime_policy)
        .map_err(LoRaConfigError::AirtimePolicy)
}

#[expect(
    clippy::too_many_arguments,
    reason = "transactional radio reconfiguration owns the full old/new policy boundary"
)]
async fn apply_profile<'a, R: LoRaRadio>(
    radio: &mut R,
    requested: RadioProfile,
    airtime_policy: AirtimePolicy,
    profile: &mut RadioProfile,
    duty: &mut Option<AirtimeDutyCycle>,
    current_id: &mut InterfaceId,
    status: &EmbassyInterfaceStatus,
    spectrum: &LoRaSpectrumStatus,
    lifecycle: DynamicSender<'_, InterfaceLifecycle<'a>>,
    control: &LoRaControlTarget<'a>,
    activation: RadioActivation,
    radio_state: &mut LoRaRadioState,
) -> LoRaApplyOutcome {
    let requested_duty = match validate_profile_request(radio, requested, airtime_policy) {
        Ok(duty) => duty,
        Err(error) => {
            crate::diagnostic_log::warn!("RNS_LORA rejected profile: {error:?}");
            return LoRaApplyOutcome::Rejected;
        }
    };
    if requested == *profile && requested_duty == *duty {
        return LoRaApplyOutcome::Applied;
    }

    let previous = *profile;
    if matches!(activation, RadioActivation::Active) {
        if let Err(error) = radio.initialize_band(requested).await {
            *radio_state = LoRaRadioState::Unknown;
            crate::diagnostic_log::warn!(
                "RNS_LORA reconfigure init failed: {error:?}; restoring prior profile"
            );
            record_reinitialization(
                reinit_radio(radio, &previous, spectrum).await,
                status,
                radio_state,
            );
            return LoRaApplyOutcome::Rejected;
        }
        if let Err(error) = radio.arm_rx().await {
            crate::diagnostic_log::warn!(
                "RNS_LORA reconfigure RX arm failed: {error:?}; restoring prior profile"
            );
            record_reinitialization(
                reinit_radio(radio, &previous, spectrum).await,
                status,
                radio_state,
            );
            return LoRaApplyOutcome::Rejected;
        }
        *radio_state = LoRaRadioState::Receiving;
    }

    let new_id = LoRaInterface::<R>::interface_id(&requested);
    if configuration::publish(
        control,
        lifecycle,
        *current_id,
        lora::descriptor(new_id, &requested, requested_duty),
    )
    .await
        != InterfacePublicationOutcome::Published
    {
        if matches!(activation, RadioActivation::Active) {
            record_reinitialization(
                reinit_radio(radio, &previous, spectrum).await,
                status,
                radio_state,
            );
        }
        return LoRaApplyOutcome::Rejected;
    }
    *profile = requested;
    *duty = requested_duty;
    *current_id = new_id;
    status.set_id(new_id);
    LoRaApplyOutcome::Applied
}

fn record_reinitialization(
    recovery: RadioReinitialization,
    status: &EmbassyInterfaceStatus,
    radio_state: &mut LoRaRadioState,
) {
    match recovery {
        RadioReinitialization::Recovered => {
            *radio_state = LoRaRadioState::Receiving;
            status.set_connection(ConnectionState::Connected);
        }
        RadioReinitialization::Failed => {
            *radio_state = LoRaRadioState::Unknown;
            status.set_connection(ConnectionState::Failed);
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "clearing owns radio, queue, and acknowledged topology publication"
)]
async fn clear_configuration<'a, R: LoRaRadio, Seam: InterfaceSeam>(
    radio: &mut R,
    current_id: &mut InterfaceId,
    status: &EmbassyInterfaceStatus,
    lifecycle: DynamicSender<'_, InterfaceLifecycle<'a>>,
    control: &LoRaControlTarget<'a>,
    backlog: &mut TransmitBacklog<'_>,
    seam: &mut Seam,
    radio_state: &mut LoRaRadioState,
) -> LoRaApplyOutcome {
    if !matches!(radio_state, LoRaRadioState::Idle) {
        if let Err(error) = radio.idle().await {
            *radio_state = LoRaRadioState::Unknown;
            crate::diagnostic_log::warn!("RNS_LORA clear failed to idle radio: {error:?}");
            return LoRaApplyOutcome::Rejected;
        }
        *radio_state = LoRaRadioState::Idle;
    }
    let new_id = unconfigured_interface_id();
    if configuration::publish(
        control,
        lifecycle,
        *current_id,
        lora::unconfigured_descriptor(new_id),
    )
    .await
        != InterfacePublicationOutcome::Published
    {
        status.set_connection(ConnectionState::Failed);
        return LoRaApplyOutcome::Rejected;
    }
    for _ in 0..backlog.clear() {
        seam.complete_outbound(OutboundDisposition::Dropped(
            OutboundDropReason::NotConfigured,
        ));
    }
    *current_id = new_id;
    status.set_id(new_id);
    status.set_connection(ConnectionState::Disabled);
    LoRaApplyOutcome::Applied
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaConfigError {
    Profile(RadioProfileError),
    RadioCompatibility(RadioProfileCompatibilityError),
    AirtimePolicy(AirtimePolicyError),
}

pub struct LoRaInterfaceInput<'a, 'publication, R: LoRaRadio, C = SubGConfigurationState> {
    pub radio: R,
    pub configuration: C,
    pub airtime_policy: AirtimePolicy,
    pub tx_queue: &'a mut [u8],
    pub control: LoRaControlTarget<'publication>,
    pub status: &'a EmbassyInterfaceStatus,
    pub spectrum: &'a LoRaSpectrumStatus,
    pub lifecycle: DynamicSender<'a, InterfaceLifecycle<'publication>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoRaRuntimeConfiguration {
    Unconfigured,
    Configured {
        profile: RadioProfile,
        duty: Option<AirtimeDutyCycle>,
    },
}

pub struct LoRaInterface<'a, 'publication, R: LoRaRadio> {
    id: InterfaceId,
    radio: R,
    configuration: LoRaRuntimeConfiguration,
    airtime_policy: AirtimePolicy,
    tag: HeaplessVec<u8, CHANNEL_TAG_CAP>,
    tx_queue: &'a mut [u8],
    control: LoRaControlTarget<'publication>,
    status: &'a EmbassyInterfaceStatus,
    spectrum: &'a LoRaSpectrumStatus,
    lifecycle: DynamicSender<'a, InterfaceLifecycle<'publication>>,
}

impl<'a, 'publication, R: LoRaRadio> LoRaInterface<'a, 'publication, R> {
    #[must_use]
    pub fn interface_id<P: Copy + Into<RadioProfile>>(profile: &P) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &(*profile).into().channel_tag())
    }

    #[must_use]
    pub fn unconfigured_interface_id() -> InterfaceId {
        unconfigured_interface_id()
    }

    pub fn new<C: Into<LoRaConfigurationState>>(
        input: LoRaInterfaceInput<'a, 'publication, R, C>,
    ) -> Result<Self, LoRaConfigError> {
        let LoRaInterfaceInput {
            radio,
            configuration,
            airtime_policy,
            tx_queue,
            control,
            status,
            spectrum,
            lifecycle,
        } = input;
        let (configuration, tag, id) = match configuration.into() {
            LoRaConfigurationState::Unconfigured => {
                let mut tag = HeaplessVec::new();
                let _ = tag.extend_from_slice(UNCONFIGURED_CHANNEL_TAG);
                (
                    LoRaRuntimeConfiguration::Unconfigured,
                    tag,
                    Self::unconfigured_interface_id(),
                )
            }
            LoRaConfigurationState::Configured(configuration) => {
                let profile = configuration.profile();
                let duty = validate_profile_request(&radio, profile, airtime_policy)?;
                let tag = profile.channel_tag();
                let id = Self::interface_id(&profile);
                (
                    LoRaRuntimeConfiguration::Configured { profile, duty },
                    tag,
                    id,
                )
            }
        };
        Ok(Self {
            id,
            radio,
            configuration,
            airtime_policy,
            tag,
            tx_queue,
            control,
            status,
            spectrum,
            lifecycle,
        })
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }
}

impl<R: LoRaRadio> Interface for LoRaInterface<'_, '_, R> {
    const HW_MTU: usize = LORA_MAX_PAYLOAD;
    const KIND: InterfaceKind = InterfaceKind::LoRa;

    fn descriptor(&self) -> InterfaceDescriptor {
        match self.configuration {
            LoRaRuntimeConfiguration::Unconfigured => lora::unconfigured_descriptor(self.id),
            LoRaRuntimeConfiguration::Configured { profile, duty } => {
                lora::descriptor(self.id, &profile, duty)
            }
        }
    }

    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }

    fn run<Seam: InterfaceSeam>(self, seam: Seam) -> impl core::future::Future<Output = ()> {
        self.run_with_clock(seam, EmbassyClock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use core::future::Future;
    use core::task::{Context, Poll};
    use embassy_futures::select::{select, Either};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use prns_core::interfaces::lora::{
        CodingRate, LoraBandwidth, Modulation, PreambleSymbols, TxPower,
    };
    use prns_core::interfaces::subghz::regions::us915::Us915;
    const US915_AUTO_LORA_PROFILE: RadioProfile =
        RadioProfile::SubG(prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE);
    use prns_core::interfaces::{FrameAccounting, FrameSink, InterfaceStatus};
    use std::boxed::Box;
    use std::rc::Rc;
    use std::task::Waker;

    use crate::radios::ReceivedAirFrame;

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "radio test failed to settle"
            );
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn id_of(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &profile.channel_tag())
    }

    struct RecordingInboundSeam {
        sink: std::vec::Vec<u8>,
        delivered: std::vec::Vec<std::vec::Vec<u8>>,
    }

    #[derive(Default)]
    pub(super) struct RadioCalls {
        pub(super) initialize: Cell<usize>,
        pub(super) initialize_failures: Cell<usize>,
        pub(super) idle: Cell<usize>,
        pub(super) idle_failures: Cell<usize>,
        pub(super) arm_rx: Cell<usize>,
        pub(super) arm_rx_failures: Cell<usize>,
        pub(super) transmit: Cell<usize>,
        pub(super) channel_rssi: Cell<usize>,
        pub(super) read_event: Cell<usize>,
        pub(super) poll_event: Cell<usize>,
    }

    pub(super) struct RecordingRadio {
        pub(super) calls: Rc<RadioCalls>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct RecordingRadioError;

    impl LoRaRadio for RecordingRadio {
        type Error = RecordingRadioError;

        fn validate_profile(
            &self,
            _profile: prns_core::interfaces::lora::RadioProfile,
        ) -> Result<(), RadioProfileCompatibilityError> {
            Ok(())
        }

        fn recovery(_error: &Self::Error) -> RadioRecovery {
            RadioRecovery::Continue
        }

        async fn initialize(
            &mut self,
            _profile: prns_core::interfaces::lora::RadioProfile,
        ) -> Result<(), Self::Error> {
            self.calls.initialize.set(self.calls.initialize.get() + 1);
            let remaining = self.calls.initialize_failures.get();
            if remaining > 0 {
                self.calls.initialize_failures.set(remaining - 1);
                return Err(RecordingRadioError);
            }
            Ok(())
        }

        async fn idle(&mut self) -> Result<(), Self::Error> {
            self.calls.idle.set(self.calls.idle.get() + 1);
            let remaining = self.calls.idle_failures.get();
            if remaining > 0 {
                self.calls.idle_failures.set(remaining - 1);
                return Err(RecordingRadioError);
            }
            Ok(())
        }

        async fn arm_rx(&mut self) -> Result<(), Self::Error> {
            self.calls.arm_rx.set(self.calls.arm_rx.get() + 1);
            let remaining = self.calls.arm_rx_failures.get();
            if remaining > 0 {
                self.calls.arm_rx_failures.set(remaining - 1);
                return Err(RecordingRadioError);
            }
            Ok(())
        }

        async fn transmit(&mut self, _payload: &[u8]) -> Result<(), Self::Error> {
            self.calls.transmit.set(self.calls.transmit.get() + 1);
            Ok(())
        }

        async fn channel_rssi_dbm(&mut self) -> Result<i16, Self::Error> {
            self.calls
                .channel_rssi
                .set(self.calls.channel_rssi.get() + 1);
            Ok(-120)
        }

        async fn read_event(&mut self, _buffer: &mut [u8]) -> Result<RadioEvent, Self::Error> {
            self.calls.read_event.set(self.calls.read_event.get() + 1);
            core::future::pending().await
        }

        async fn poll_event(
            &mut self,
            _buffer: &mut [u8],
        ) -> Result<Option<RadioEvent>, Self::Error> {
            self.calls.poll_event.set(self.calls.poll_event.get() + 1);
            Ok(None)
        }
    }

    fn recording_interface<'a, 'publication>(
        calls: Rc<RadioCalls>,
        configuration: SubGConfigurationState,
        tx_queue: &'a mut [u8],
        control: LoRaControlTarget<'publication>,
        status: &'a EmbassyInterfaceStatus,
        spectrum: &'a LoRaSpectrumStatus,
        lifecycle: DynamicSender<'a, InterfaceLifecycle<'publication>>,
    ) -> LoRaInterface<'a, 'publication, RecordingRadio> {
        LoRaInterface::new(LoRaInterfaceInput {
            radio: RecordingRadio { calls },
            configuration,
            airtime_policy: AirtimePolicy::Regional,
            tx_queue,
            control,
            status,
            spectrum,
            lifecycle,
        })
        .unwrap()
    }

    impl InterfaceSeam for RecordingInboundSeam {
        fn fill_random(&mut self, bytes: &mut [u8]) {
            bytes.fill(0);
        }

        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.sink
        }

        async fn commit_inbound(&mut self) {
            if !self.sink.is_empty() {
                self.delivered.push(self.sink.clone());
                self.sink.clear();
            }
        }

        async fn next_inbound_with_phy(&mut self, frame: &[u8], _phy: PacketPhyStats) {
            self.delivered.push(frame.to_vec());
        }

        async fn next_outbound(&mut self) -> &[u8] {
            core::future::pending().await
        }
    }

    #[test]
    fn unconfigured_interface_idles_once_without_arming_or_sensing() {
        let calls = Rc::new(RadioCalls::default());
        let mut tx_queue = [0; LORA_TX_QUEUE_BYTES];
        let mut control = LoRaControl::new();
        let (_controller, target) = control.split();
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id(),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
        let interface = recording_interface(
            calls.clone(),
            SubGConfigurationState::Unconfigured,
            &mut tx_queue,
            target,
            &status,
            &spectrum,
            lifecycle.dyn_sender(),
        );
        status.set_id(interface.id());
        let seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };
        let mut run = Box::pin(interface.run(seam));
        let mut context = Context::from_waker(Waker::noop());

        assert_eq!(run.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(calls.initialize.get(), 0);
        assert_eq!(calls.idle.get(), 1);
        assert_eq!(calls.arm_rx.get(), 0);
        assert_eq!(calls.transmit.get(), 0);
        assert_eq!(calls.channel_rssi.get(), 0);
        assert_eq!(calls.read_event.get(), 0);
        assert_eq!(calls.poll_event.get(), 0);
    }

    #[test]
    fn unconfigured_interface_retries_a_failed_idle_transition() {
        let calls = Rc::new(RadioCalls::default());
        calls.idle_failures.set(1);
        let mut tx_queue = [0; LORA_TX_QUEUE_BYTES];
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id(),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
        let interface = recording_interface(
            calls.clone(),
            SubGConfigurationState::Unconfigured,
            &mut tx_queue,
            target,
            &status,
            &spectrum,
            lifecycle.dyn_sender(),
        );
        status.set_id(interface.id());
        let seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };

        block_on(async {
            match select(
                async {
                    while calls.idle.get() < 2 {
                        Timer::after(Duration::from_millis(1)).await;
                    }
                    assert_eq!(controller.clear().await, LoRaApplyOutcome::Applied);
                },
                async {
                    select(interface.run(seam), async {
                        loop {
                            if let InterfaceLifecycle::Publish { completion, .. } =
                                lifecycle.receive().await
                            {
                                completion.signal(InterfacePublicationOutcome::Published);
                            }
                        }
                    })
                    .await;
                },
            )
            .await
            {
                Either::First(()) => {}
                Either::Second(()) => panic!("interface run loop returned"),
            }
        });

        assert_eq!(calls.idle.get(), 2);
        assert_eq!(calls.initialize.get(), 0);
        assert_eq!(calls.arm_rx.get(), 0);
    }

    #[test]
    fn configuration_activates_and_clear_returns_the_same_interface_to_idle() {
        let calls = Rc::new(RadioCalls::default());
        let mut tx_queue = [0; LORA_TX_QUEUE_BYTES];
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id(),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 4>::new();
        let interface = recording_interface(
            calls.clone(),
            SubGConfigurationState::Unconfigured,
            &mut tx_queue,
            target,
            &status,
            &spectrum,
            lifecycle.dyn_sender(),
        );
        status.set_id(interface.id());
        let seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };

        block_on(async {
            match select(
                async {
                    let configuration = SubGConfigurationState::Configured(Us915::auto_lora());
                    assert_eq!(
                        controller.apply_configuration(configuration).await,
                        LoRaApplyOutcome::Applied
                    );
                    assert_eq!(controller.clear().await, LoRaApplyOutcome::Applied);
                },
                async {
                    select(interface.run(seam), async {
                        loop {
                            if let InterfaceLifecycle::Publish { completion, .. } =
                                lifecycle.receive().await
                            {
                                completion.signal(InterfacePublicationOutcome::Published);
                            }
                        }
                    })
                    .await;
                },
            )
            .await
            {
                Either::First(()) => {}
                Either::Second(()) => panic!("interface run loop returned"),
            }
        });

        assert_eq!(calls.initialize.get(), 1);
        assert_eq!(calls.arm_rx.get(), 1);
        assert_eq!(calls.idle.get(), 2);
        assert_eq!(calls.transmit.get(), 0);
        assert_eq!(calls.channel_rssi.get(), 0);
        assert_eq!(calls.poll_event.get(), 0);
        assert_eq!(
            status.id(),
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id()
        );
    }

    #[test]
    fn clear_is_serviced_while_the_configured_interface_is_power_disabled() {
        let calls = Rc::new(RadioCalls::default());
        let mut tx_queue = [0; LORA_TX_QUEUE_BYTES];
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let configuration = SubGConfigurationState::Configured(Us915::auto_lora());
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id(),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 4>::new();
        let interface = recording_interface(
            calls.clone(),
            configuration,
            &mut tx_queue,
            target,
            &status,
            &spectrum,
            lifecycle.dyn_sender(),
        );
        let seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };

        block_on(async {
            match select(
                async {
                    status.disable();
                    assert_eq!(controller.clear().await, LoRaApplyOutcome::Applied);
                },
                async {
                    select(interface.run(seam), async {
                        loop {
                            if let InterfaceLifecycle::Publish { completion, .. } =
                                lifecycle.receive().await
                            {
                                completion.signal(InterfacePublicationOutcome::Published);
                            }
                        }
                    })
                    .await;
                },
            )
            .await
            {
                Either::First(()) => {}
                Either::Second(()) => panic!("interface run loop returned"),
            }
        });

        assert_eq!(calls.initialize.get(), 0);
        assert_eq!(calls.arm_rx.get(), 0);
        assert_eq!(calls.idle.get(), 1);
        assert_eq!(calls.transmit.get(), 0);
        assert_eq!(
            status.id(),
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id()
        );
    }

    #[test]
    fn applying_while_power_disabled_changes_configuration_without_rf_activity() {
        let calls = Rc::new(RadioCalls::default());
        let mut tx_queue = [0; LORA_TX_QUEUE_BYTES];
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let configuration = SubGConfigurationState::Configured(Us915::auto_lora());
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id(),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 4>::new();
        let interface = recording_interface(
            calls.clone(),
            configuration,
            &mut tx_queue,
            target,
            &status,
            &spectrum,
            lifecycle.dyn_sender(),
        );
        let seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };
        let requested = US915_AUTO_LORA_PROFILE
            .with_tx_power(TxPower::new(12))
            .unwrap();

        block_on(async {
            match select(
                async {
                    status.disable();
                    assert_eq!(controller.apply(requested).await, LoRaApplyOutcome::Applied);
                },
                async {
                    select(interface.run(seam), async {
                        loop {
                            if let InterfaceLifecycle::Publish { completion, .. } =
                                lifecycle.receive().await
                            {
                                completion.signal(InterfacePublicationOutcome::Published);
                            }
                        }
                    })
                    .await;
                },
            )
            .await
            {
                Either::First(()) => {}
                Either::Second(()) => panic!("interface run loop returned"),
            }
        });

        assert_eq!(calls.initialize.get(), 0);
        assert_eq!(calls.arm_rx.get(), 0);
        assert_eq!(calls.idle.get(), 1);
        assert_eq!(calls.transmit.get(), 0);
    }

    #[test]
    fn enabled_configuration_retries_initialization_until_the_radio_is_ready() {
        let calls = Rc::new(RadioCalls::default());
        calls.initialize_failures.set(1);
        let mut tx_queue = [0; LORA_TX_QUEUE_BYTES];
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let configuration = SubGConfigurationState::Configured(Us915::auto_lora());
        let status = EmbassyInterfaceStatus::new_accounted(
            LoRaInterface::<RecordingRadio>::unconfigured_interface_id(),
            ConnectionState::Initializing,
        );
        let spectrum = LoRaSpectrumStatus::new();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 4>::new();
        let interface = recording_interface(
            calls.clone(),
            configuration,
            &mut tx_queue,
            target,
            &status,
            &spectrum,
            lifecycle.dyn_sender(),
        );
        let seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };

        block_on(async {
            match select(
                async {
                    while calls.arm_rx.get() == 0 {
                        Timer::after(Duration::from_millis(1)).await;
                    }
                    assert_eq!(controller.clear().await, LoRaApplyOutcome::Applied);
                },
                async {
                    select(interface.run(seam), async {
                        loop {
                            if let InterfaceLifecycle::Publish { completion, .. } =
                                lifecycle.receive().await
                            {
                                completion.signal(InterfacePublicationOutcome::Published);
                            }
                        }
                    })
                    .await;
                },
            )
            .await
            {
                Either::First(()) => {}
                Either::Second(()) => panic!("interface run loop returned"),
            }
        });

        assert_eq!(calls.initialize.get(), 2);
        assert_eq!(calls.arm_rx.get(), 1);
        assert_eq!(calls.idle.get(), 1);
    }

    #[test]
    fn receive_accounting_distinguishes_reassembly_resync_from_delivery() {
        let status = EmbassyInterfaceStatus::new_accounted(
            id_of(&US915_AUTO_LORA_PROFILE),
            ConnectionState::Connected,
        );
        let mut throughput = ThroughputLedger::new();
        let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();
        let mut seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };
        let first_payload = [0xA1; 300];
        let delivered_payload = [0xB2; 300];
        let mut first_fragment = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut replacement_first = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut replacement_second = [0u8; LORA_SINGLE_FRAME_MAX];
        let first_len =
            encode_air_frame_part(&first_payload, 0x10, 0, &mut first_fragment).unwrap();
        let replacement_first_len =
            encode_air_frame_part(&delivered_payload, 0x20, 0, &mut replacement_first).unwrap();
        let replacement_second_len =
            encode_air_frame_part(&delivered_payload, 0x20, 1, &mut replacement_second).unwrap();

        block_on(async {
            for (bytes, arrived_at) in [
                (&first_fragment[..first_len], 1),
                (&replacement_first[..replacement_first_len], 2),
                (&replacement_second[..replacement_second_len], 3),
                (&[][..], 4),
            ] {
                deliver_rx(
                    ObservedAirFrame {
                        bytes,
                        phy: PacketPhyStats::default(),
                        spreading_factor: SpreadingFactor::Sf7,
                        arrived_at: InstantMillis(arrived_at),
                    },
                    &status,
                    &mut throughput,
                    &mut reassembler,
                    &mut seam,
                )
                .await;
            }
        });

        assert_eq!(seam.delivered.len(), 1);
        assert_eq!(seam.delivered[0], delivered_payload);
        assert_eq!(
            status.frame_accounting(),
            Some(FrameAccounting {
                frames_in: 4,
                malformed: 1,
                protocol_violations: 2,
                undecodable: 1,
                delivered: 1,
            })
        );
    }

    #[test]
    fn a_channel_change_re_keys_to_the_new_id() {
        let current = id_of(&US915_AUTO_LORA_PROFILE);
        let next = US915_AUTO_LORA_PROFILE
            .with_modulation(Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf10,
                bandwidth: LoraBandwidth::Bw125kHz,
                coding_rate: CodingRate::Cr45,
            })
            .unwrap();
        let new_id = id_of(&next);
        assert_ne!(new_id, current);
    }

    #[test]
    fn a_local_only_change_does_not_re_key() {
        let current = id_of(&US915_AUTO_LORA_PROFILE);
        let next = US915_AUTO_LORA_PROFILE
            .with_tx_power(TxPower::new(2))
            .unwrap()
            .with_preamble(PreambleSymbols::new(24))
            .unwrap();
        assert!(
            id_of(&next) == current,
            "transmit power and preamble are local knobs, not channel identity"
        );
    }

    #[test]
    fn packet_airtime_sums_both_frames_of_a_split() {
        let profile = US915_AUTO_LORA_PROFILE;
        let one_frame = packet_airtime(&[0u8; 100], &profile);
        let two_frames = packet_airtime(&[0u8; 400], &profile);
        assert_eq!(
            one_frame,
            profile.time_on_air_us(101),
            "one frame: the header plus 100 payload bytes on air"
        );
        assert_eq!(
            two_frames,
            profile.time_on_air_us(255) + profile.time_on_air_us(147),
            "two frames: a full 255-byte frame plus the 147-byte remainder"
        );
        assert!(two_frames > one_frame);
    }

    #[test]
    fn only_successfully_decoded_frames_earn_service_age() {
        for event in [
            RadioEvent::PreambleDetected,
            RadioEvent::HeaderValid,
            RadioEvent::HeaderError,
            RadioEvent::CrcError,
            RadioEvent::Timeout,
            RadioEvent::SpuriousInterrupt,
        ] {
            assert_eq!(
                decoded_peer_airtime_us(event, &US915_AUTO_LORA_PROFILE),
                None
            );
        }
        assert_eq!(
            decoded_peer_airtime_us(
                RadioEvent::Frame(ReceivedAirFrame {
                    len: 100,
                    phy: PacketPhyStats::default(),
                }),
                &US915_AUTO_LORA_PROFILE,
            ),
            Some(US915_AUTO_LORA_PROFILE.time_on_air_us(100))
        );
    }

    #[test]
    fn backlog_admission_and_pending_follow_active_and_queued_ownership() {
        let profile = US915_AUTO_LORA_PROFILE;
        let mut storage = [0; LORA_MAX_PAYLOAD + 2];
        let mut backlog = TransmitBacklog::new(&mut storage);
        assert!(!backlog.has_pending());
        assert!(backlog.can_accept_outbound());
        backlog.accept(&[1], &profile, 10).unwrap();
        assert!(backlog.has_pending());
        assert!(backlog.can_accept_outbound());
        backlog
            .accept(&[2; LORA_MAX_PAYLOAD], &profile, 11)
            .unwrap();
        assert!(backlog.has_pending());
        assert!(!backlog.can_accept_outbound());
        backlog.active.clear();
        assert!(backlog.has_pending());
        assert!(!backlog.can_accept_outbound());
        assert!(backlog.activate_next(&profile, 12));
        assert!(backlog.has_pending());
        assert!(backlog.can_accept_outbound());
        backlog.active.clear();
        assert!(!backlog.has_pending());
        assert!(backlog.can_accept_outbound());
        let mut no_queue = [];
        let mut backlog = TransmitBacklog::new(&mut no_queue);
        backlog.accept(&[3], &profile, 13).unwrap();
        assert!(backlog.has_pending());
        assert!(!backlog.can_accept_outbound());
    }

    #[test]
    fn changing_preamble_recomputes_active_airtime_without_replacing_its_packet_or_age() {
        let old = US915_AUTO_LORA_PROFILE;
        let new = old.with_preamble(PreambleSymbols::new(200)).unwrap();
        let mut packet = ActivePacket::new();
        packet.activate(&[0xab; LORA_MAX_PAYLOAD], &old, 42);
        let old_airtime = packet.airtime_us;
        packet.recompute_airtime(&new);
        assert!(packet.airtime_us > old_airtime);
        assert_eq!(
            (packet.len, packet.activated_at_ms, packet.airtime_us),
            (Some(LORA_MAX_PAYLOAD), 42, new.time_on_air_us(255) * 2)
        );
        assert_eq!(packet.bytes, [0xab; LORA_MAX_PAYLOAD]);
    }

    #[test]
    fn an_active_packet_accepts_a_fifo_burst_until_maximum_record_backpressure() {
        let mut storage = [0; LORA_TX_QUEUE_BYTES];
        let mut backlog = TransmitBacklog::new(&mut storage);
        let profile = US915_AUTO_LORA_PROFILE;

        assert_eq!(
            backlog.accept(&[0; LORA_MAX_PAYLOAD], &profile, 10),
            Ok(PacketPlacement::Active)
        );
        for value in 1..=12 {
            assert_eq!(
                backlog.accept(&[value; LORA_MAX_PAYLOAD], &profile, 10),
                Ok(PacketPlacement::Queued)
            );
        }
        assert!(!backlog.can_accept_outbound());

        for value in 0..=12 {
            assert_eq!(
                backlog.active.bytes[..backlog.active.len.unwrap()],
                [value; LORA_MAX_PAYLOAD]
            );
            backlog.active.clear();
            if value < 12 {
                assert!(backlog.activate_next(&profile, 20 + u64::from(value)));
            }
        }
        assert!(backlog.queue.is_empty());
    }

    #[test]
    fn disable_drops_only_the_active_packet_and_activation_starts_a_fresh_profile_timeout() {
        let mut storage = [0; LORA_TX_QUEUE_BYTES];
        let mut backlog = TransmitBacklog::new(&mut storage);
        let original_profile = US915_AUTO_LORA_PROFILE;
        let queued = [0xA5; 400];
        backlog.accept(b"active", &original_profile, 1_000).unwrap();
        backlog.accept(&queued, &original_profile, 1_001).unwrap();

        assert!(backlog.active.clear());
        assert!(!backlog.queue.is_empty());

        let current_profile = original_profile
            .with_modulation(Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf12,
                bandwidth: LoraBandwidth::Bw125kHz,
                coding_rate: CodingRate::Cr48,
            })
            .unwrap();
        let activated_at_ms = 200_000;
        assert!(backlog.activate_next(&current_profile, activated_at_ms));
        assert_eq!(
            backlog.active.airtime_us,
            packet_airtime(&queued, &current_profile)
        );
        assert_eq!(backlog.active.activated_at_ms, activated_at_ms);

        let access = backlog
            .active
            .channel_access(
                current_profile,
                activated_at_ms,
                ContentionPriority::Fresh {
                    short_airtime_per_mille: 0,
                },
            )
            .unwrap();
        let ttl_ms = channel_access::pending_ttl_ms(backlog.active.airtime_us);
        assert!(!access.is_expired(activated_at_ms + ttl_ms - 1));
        assert!(access.is_expired(activated_at_ms + ttl_ms));
        assert!(backlog.active.clear());
        assert!(!backlog.activate_next(&current_profile, activated_at_ms + ttl_ms));
    }

    #[test]
    fn spectrum_status_publishes_sensing_and_stewardship_counters() {
        let status = LoRaSpectrumStatus::new();
        let mut noise = NoiseFloor::new();
        for index in 0..32 {
            let observation = noise.observe(index, -120, false);
            status.record_channel(observation, Some(&noise));
        }
        status.record_channel(ChannelObservation::Busy, Some(&noise));
        status.add_deferrals(3);
        status.add_false_preamble();
        status.add_contention_timeout();
        status.add_duty_hold();
        status.add_duty_timeout();
        status.add_radio_recovery();

        assert_eq!(
            status.snapshot(),
            LoRaSpectrumSnapshot {
                channel_busy_per_mille: 500,
                noise_floor_dbm: Some(-120),
                cca_threshold_dbm: Some(-109),
                deferrals: 3,
                false_preambles: 1,
                contention_timeouts: 1,
                duty_holds: 1,
                duty_timeouts: 1,
                radio_recoveries: 1,
                channel_change_drops: 0,
            }
        );
    }
    #[test]
    fn staged_configuration_pauses_traffic_and_waits_for_publication_acknowledgement() {
        use embassy_futures::join::join3;
        use prns_core::interfaces::lora::LoRaConfiguration;
        let previous = LoRaRuntimeConfiguration::Configured {
            profile: US915_AUTO_LORA_PROFILE,
            duty: None,
        };
        for requested in [
            LoRaConfigurationState::Configured(LoRaConfiguration::manual(
                US915_AUTO_LORA_PROFILE
                    .with_tx_power(TxPower::new(12))
                    .unwrap(),
            )),
            LoRaConfigurationState::Unconfigured,
        ] {
            let mut control = LoRaControl::new();
            let (mut controller, target) = control.split();
            let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 4>::new();
            let calls = Rc::new(RadioCalls::default());
            let mut radio = RecordingRadio {
                calls: calls.clone(),
            };
            let mut id = id_of(&US915_AUTO_LORA_PROFILE);
            let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Connected);
            let mut state = LoRaRadioState::Receiving;
            let mut bytes = [0; 1024];
            let mut backlog = TransmitBacklog::new(&mut bytes);
            backlog
                .accept(b"pending one", &US915_AUTO_LORA_PROFILE, 0)
                .unwrap();
            backlog
                .accept(b"pending two", &US915_AUTO_LORA_PROFILE, 0)
                .unwrap();
            let mut seam = RecordingInboundSeam {
                sink: std::vec::Vec::new(),
                delivered: std::vec::Vec::new(),
            };
            let (settled, (), ()) = block_on(join3(
                async {
                    let request = target.wait().await;
                    assert_eq!(request.command, LoRaConfigurationCommand::Quiesce);
                    configuration::transact(
                        &mut radio,
                        &target,
                        request.id,
                        &previous,
                        AirtimePolicy::Regional,
                        &mut id,
                        &status,
                        lifecycle.dyn_sender(),
                        &mut state,
                        &mut backlog,
                        &mut seam,
                    )
                    .await
                },
                async {
                    assert_eq!(controller.quiesce().await, LoRaApplyOutcome::Applied);
                    assert_eq!(
                        controller.stage_configuration(requested).await,
                        LoRaApplyOutcome::Applied
                    );
                    assert_eq!(calls.transmit.get(), 0);
                    assert_eq!(calls.read_event.get(), 0);
                    assert_eq!(calls.arm_rx.get(), 0);
                    assert!(lifecycle.try_receive().is_err());
                    assert_eq!(
                        controller.publish_configuration().await,
                        LoRaApplyOutcome::Applied
                    );
                },
                async {
                    match lifecycle.receive().await {
                        InterfaceLifecycle::Publish { completion, .. } => {
                            completion.signal(InterfacePublicationOutcome::Published)
                        }
                        _ => panic!("expected acknowledged publication"),
                    }
                },
            ));
            match requested {
                LoRaConfigurationState::Unconfigured => {
                    assert_eq!(settled, LoRaRuntimeConfiguration::Unconfigured);
                    assert!(!backlog.has_pending());
                    assert_eq!(state, LoRaRadioState::Idle);
                }
                LoRaConfigurationState::Configured(configuration) => {
                    assert_eq!(
                        settled,
                        LoRaRuntimeConfiguration::Configured {
                            profile: configuration.profile(),
                            duty: None
                        }
                    );
                    assert!(
                        backlog.has_pending(),
                        "local power changes preserve pending work"
                    );
                    assert_eq!(state, LoRaRadioState::Receiving);
                }
            }
        }
    }

    #[test]
    fn disabled_radio_can_stage_a_desired_configuration_without_initializing_rf() {
        use embassy_futures::join::join3;
        use prns_core::interfaces::lora::LoRaConfiguration;
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 4>::new();
        let calls = Rc::new(RadioCalls::default());
        let mut radio = RecordingRadio {
            calls: calls.clone(),
        };
        let mut id = unconfigured_interface_id();
        let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Disabled);
        status.disable();
        let mut state = LoRaRadioState::Idle;
        let mut bytes = [0; 1024];
        let mut backlog = TransmitBacklog::new(&mut bytes);
        let mut seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };
        let requested =
            LoRaConfigurationState::Configured(LoRaConfiguration::manual(US915_AUTO_LORA_PROFILE));
        block_on(join3(
            async {
                let request = target.wait().await;
                configuration::transact(
                    &mut radio,
                    &target,
                    request.id,
                    &LoRaRuntimeConfiguration::Unconfigured,
                    AirtimePolicy::Regional,
                    &mut id,
                    &status,
                    lifecycle.dyn_sender(),
                    &mut state,
                    &mut backlog,
                    &mut seam,
                )
                .await
            },
            async {
                assert_eq!(controller.quiesce().await, LoRaApplyOutcome::Applied);
                assert_eq!(
                    controller.stage_configuration(requested).await,
                    LoRaApplyOutcome::Applied
                );
                assert_eq!(
                    controller.publish_configuration().await,
                    LoRaApplyOutcome::Applied
                );
            },
            async {
                match lifecycle.receive().await {
                    InterfaceLifecycle::Publish { completion, .. } => {
                        completion.signal(InterfacePublicationOutcome::Published)
                    }
                    _ => panic!("expected acknowledged publication"),
                }
            },
        ));
        assert_eq!(calls.initialize.get(), 0);
        assert_eq!(calls.arm_rx.get(), 0);
        assert_eq!(calls.transmit.get(), 0);
        assert_eq!(state, LoRaRadioState::Idle);
        assert_eq!(status.connection(), ConnectionState::Disabled);
    }
    #[test]
    fn enabling_after_staging_initializes_the_staged_configuration_before_receiving() {
        use embassy_futures::join::join3;
        use prns_core::interfaces::lora::LoRaConfiguration;
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 4>::new();
        let calls = Rc::new(RadioCalls::default());
        let mut radio = RecordingRadio {
            calls: calls.clone(),
        };
        let mut id = unconfigured_interface_id();
        let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Disabled);
        status.disable();
        let mut state = LoRaRadioState::Idle;
        let mut bytes = [0; 1024];
        let mut backlog = TransmitBacklog::new(&mut bytes);
        let mut seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };
        let requested =
            LoRaConfigurationState::Configured(LoRaConfiguration::manual(US915_AUTO_LORA_PROFILE));
        block_on(join3(
            async {
                let request = target.wait().await;
                configuration::transact(
                    &mut radio,
                    &target,
                    request.id,
                    &LoRaRuntimeConfiguration::Unconfigured,
                    AirtimePolicy::Regional,
                    &mut id,
                    &status,
                    lifecycle.dyn_sender(),
                    &mut state,
                    &mut backlog,
                    &mut seam,
                )
                .await
            },
            async {
                assert_eq!(controller.quiesce().await, LoRaApplyOutcome::Applied);
                assert_eq!(
                    controller.stage_configuration(requested).await,
                    LoRaApplyOutcome::Applied
                );
                assert_eq!(calls.initialize.get(), 0);
                status.enable();
                assert_eq!(
                    controller.publish_configuration().await,
                    LoRaApplyOutcome::Applied
                );
            },
            async {
                match lifecycle.receive().await {
                    InterfaceLifecycle::Publish { completion, .. } => {
                        completion.signal(InterfacePublicationOutcome::Published)
                    }
                    _ => panic!("expected acknowledged publication"),
                }
            },
        ));
        assert_eq!(calls.initialize.get(), 1);
        assert_eq!(calls.arm_rx.get(), 1);
        assert_eq!(calls.transmit.get(), 0);
        assert_eq!(state, LoRaRadioState::Receiving);
        assert_eq!(status.connection(), ConnectionState::Connected);
    }
    #[test]
    fn rejected_publication_keeps_old_backlog_available_for_restoration() {
        use embassy_futures::join::join3;
        use prns_core::interfaces::lora::LoRaConfiguration;
        let previous = LoRaRuntimeConfiguration::Configured {
            profile: US915_AUTO_LORA_PROFILE,
            duty: None,
        };
        let mut control = LoRaControl::new();
        let (mut controller, target) = control.split();
        let lifecycle = Channel::<CriticalSectionRawMutex, InterfaceLifecycle, 1>::new();
        let calls = Rc::new(RadioCalls::default());
        let mut radio = RecordingRadio {
            calls: calls.clone(),
        };
        let original_id = id_of(&US915_AUTO_LORA_PROFILE);
        let mut id = original_id;
        let status = EmbassyInterfaceStatus::new_accounted(id, ConnectionState::Connected);
        let mut state = LoRaRadioState::Receiving;
        let mut bytes = [0; 1024];
        let mut backlog = TransmitBacklog::new(&mut bytes);
        backlog
            .accept(b"retained one", &US915_AUTO_LORA_PROFILE, 0)
            .unwrap();
        backlog
            .accept(b"retained two", &US915_AUTO_LORA_PROFILE, 0)
            .unwrap();
        let mut seam = RecordingInboundSeam {
            sink: std::vec::Vec::new(),
            delivered: std::vec::Vec::new(),
        };
        let (settled, (), ()) = block_on(join3(
            async {
                let request = target.wait().await;
                configuration::transact(
                    &mut radio,
                    &target,
                    request.id,
                    &previous,
                    AirtimePolicy::Regional,
                    &mut id,
                    &status,
                    lifecycle.dyn_sender(),
                    &mut state,
                    &mut backlog,
                    &mut seam,
                )
                .await
            },
            async {
                assert_eq!(controller.quiesce().await, LoRaApplyOutcome::Applied);
                assert_eq!(
                    controller
                        .stage_configuration(LoRaConfigurationState::Unconfigured)
                        .await,
                    LoRaApplyOutcome::Applied
                );
                assert_eq!(
                    controller.publish_configuration().await,
                    LoRaApplyOutcome::Rejected
                );
                assert_eq!(status.id(), original_id);
                assert_eq!(status.connection(), ConnectionState::Failed);
                assert_eq!(calls.transmit.get(), 0);
                assert_eq!(
                    controller.resume_configuration().await,
                    LoRaApplyOutcome::Rejected
                );
                assert_eq!(
                    controller
                        .stage_configuration(LoRaConfigurationState::Configured(
                            LoRaConfiguration::manual(US915_AUTO_LORA_PROFILE)
                        ))
                        .await,
                    LoRaApplyOutcome::Applied
                );
                assert_eq!(
                    controller.resume_configuration().await,
                    LoRaApplyOutcome::Applied
                );
            },
            async {
                for outcome in [
                    InterfacePublicationOutcome::IdentityConflict,
                    InterfacePublicationOutcome::Published,
                ] {
                    match lifecycle.receive().await {
                        InterfaceLifecycle::Publish {
                            old_id, completion, ..
                        } => {
                            assert_eq!(old_id, original_id);
                            completion.signal(outcome);
                        }
                        _ => panic!("expected acknowledged publication"),
                    }
                }
            },
        ));
        assert_eq!(settled, previous);
        assert_eq!(id, original_id);
        assert_eq!(state, LoRaRadioState::Receiving);
        assert_eq!(
            backlog.clear(),
            2,
            "failed publication must retain the complete old backlog"
        );
        assert!(seam.delivered.is_empty());
    }
    #[test]
    fn resetting_partial_receive_counts_one_undecodable_frame() {
        let status = EmbassyInterfaceStatus::new_accounted(
            id_of(&US915_AUTO_LORA_PROFILE),
            ConnectionState::Connected,
        );
        let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();
        reset_reassembly(&mut reassembler, &status);
        assert_eq!(
            reassembler.feed(&[0x11, 1]),
            LoRaReassemblyOutcome::AwaitingSecond
        );
        reset_reassembly(&mut reassembler, &status);
        reset_reassembly(&mut reassembler, &status);
        assert_eq!(status.frame_accounting().unwrap().undecodable, 1);
    }
}
