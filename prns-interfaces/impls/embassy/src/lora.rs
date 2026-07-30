use embassy_futures::select::{select, select4, Either, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::DynamicSender;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::SpiDevice;
use heapless::Vec as HeaplessVec;
use portable_atomic::{AtomicU32, Ordering};

use prns_core::engine::InstantMillis;
use prns_core::interfaces::lora::{
    self, air_frame_count, encode_air_frame_part, AirtimePolicy, AirtimePolicyError, CodingRate,
    LoRaReassembler, LoraBandwidth, Modulation, RadioProfile, RadioProfileError, SpreadingFactor,
    CHANNEL_TAG_CAP, LORA_MAX_PAYLOAD, LORA_SINGLE_FRAME_MAX, RNODE_LORA_SYNC_WORD,
};
use prns_core::interfaces::{
    AirtimeDutyCycle, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind,
    PacketPhyStats,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::{EmbassyInterfaceStatus, InterfaceLifecycle};
use prns_runtime::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use prns_runtime::manifold::throughput::ThroughputLedger;

use crate::radios::sx126x::{self, RadioEvent, Sx126x};

mod channel_access;

use channel_access::{
    ChannelAccess, ChannelAccessAction, ChannelObservation, ChannelTiming, DemodulatorActivity,
    NoiseFloor,
};

const IDLE_TICK: Duration = Duration::from_millis(250);
const SENSING_UNPUBLISHED: u32 = u32::MAX;

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

async fn deliver_rx<Seam: InterfaceSeam>(
    frame: ObservedAirFrame<'_>,
    status: &EmbassyInterfaceStatus,
    throughput: &mut ThroughputLedger,
    reassembler: &mut LoRaReassembler<LORA_MAX_PAYLOAD>,
    seam: &mut Seam,
) {
    status.add_rx(frame.bytes.len() as u64);
    throughput.record_rx(frame.arrived_at, frame.bytes.len() as u64);
    status.set_transfer_rates(throughput.rates());
    if let Some(packet) = reassembler.feed_with_phy(frame.bytes, frame.phy) {
        if !packet.bytes.is_empty() && packet.bytes.len() <= LORA_MAX_PAYLOAD {
            let mut phy = packet.phy;
            if let Some(snr) = phy.snr {
                phy.quality = frame.spreading_factor.signal_quality(snr);
            }
            seam.next_inbound_with_phy(packet.bytes, phy).await;
        }
    }
}

fn choose_backoff_entropy<Seam: InterfaceSeam>(
    access: &mut ChannelAccess,
    seam: &mut Seam,
) -> ChannelAccessAction {
    let mut entropy = [0u8; 2];
    loop {
        seam.fill_entropy(&mut entropy);
        if access.choose_backoff(u16::from_le_bytes(entropy)) {
            return access.after_entropy();
        }
    }
}

async fn observe_radio_event<Seam: InterfaceSeam>(
    event: RadioEvent,
    now: InstantMillis,
    receive: ReceivePath<'_, Seam>,
) -> ChannelObservation {
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
                    spreading_factor: receive.profile.modulation.spreading_factor(),
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
        RadioEvent::Other => ChannelObservation::Unknown,
    };
    receive.spectrum.record_channel(observation, None);
    observation
}

async fn sample_channel<SPI, BUSY, DIO1, RST, DLY>(
    radio: &mut Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    now: InstantMillis,
    activity: &mut DemodulatorActivity,
    spectrum: &LoRaSpectrumStatus,
    noise: &mut NoiseFloor,
) -> Result<ChannelObservation, sx126x::Error>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
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

/// The signal the app holds to reconfigure a running radio: it sends a whole new [`RadioProfile`] and the worker rebuilds the silicon's params from it.
pub type LoRaControl = Signal<CriticalSectionRawMutex, RadioProfile>;

fn sx126x_config(profile: &RadioProfile) -> sx126x::RadioConfig {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let spreading_factor = match spreading_factor {
        SpreadingFactor::Sf5 => sx126x::SpreadingFactor::Sf5,
        SpreadingFactor::Sf6 => sx126x::SpreadingFactor::Sf6,
        SpreadingFactor::Sf7 => sx126x::SpreadingFactor::Sf7,
        SpreadingFactor::Sf8 => sx126x::SpreadingFactor::Sf8,
        SpreadingFactor::Sf9 => sx126x::SpreadingFactor::Sf9,
        SpreadingFactor::Sf10 => sx126x::SpreadingFactor::Sf10,
        SpreadingFactor::Sf11 => sx126x::SpreadingFactor::Sf11,
        SpreadingFactor::Sf12 => sx126x::SpreadingFactor::Sf12,
    };
    let bandwidth = match bandwidth {
        LoraBandwidth::Bw125kHz => sx126x::Bandwidth::Bw125,
        LoraBandwidth::Bw250kHz => sx126x::Bandwidth::Bw250,
        LoraBandwidth::Bw500kHz => sx126x::Bandwidth::Bw500,
    };
    let coding_rate = match coding_rate {
        CodingRate::Cr45 => sx126x::CodingRate::Cr4_5,
        CodingRate::Cr46 => sx126x::CodingRate::Cr4_6,
        CodingRate::Cr47 => sx126x::CodingRate::Cr4_7,
        CodingRate::Cr48 => sx126x::CodingRate::Cr4_8,
    };
    sx126x::RadioConfig {
        frequency_hz: profile.frequency.hz(),
        modulation: sx126x::Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        packet: sx126x::LoraPacket {
            preamble_symbols: profile.preamble.count(),
            explicit_header: true,
            crc_on: true,
            invert_iq: false,
        },
        sync_word: RNODE_LORA_SYNC_WORD,
        tx_power_dbm: profile.tx_power.dbm(),
    }
}

/// The [`Retag`](InterfaceLifecycle::Retag) a reconfigure to `new_profile` warrants, or `None` when the change leaves the channel identity untouched — a local knob like transmit power or preamble. The channel_tag (frequency + modulation) is what mints the id, so only a change to it re-keys.
fn retag_message(
    current_id: InterfaceId,
    new_profile: &RadioProfile,
    duty: Option<AirtimeDutyCycle>,
) -> Option<InterfaceLifecycle> {
    let new_id =
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &lora::channel_tag(new_profile));
    (new_id != current_id).then(|| InterfaceLifecycle::Retag {
        old_id: current_id,
        new_id,
        descriptor: lora::descriptor(new_id, new_profile, duty),
    })
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

#[expect(
    clippy::too_many_arguments,
    reason = "embedded serve-loop internals pass the loop's split-borrowed locals; bundling awaits an on-hardware validation pass"
)]
async fn transmit_packet<SPI, BUSY, DIO1, RST, DLY>(
    radio: &mut Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    packet: &[u8],
    seq: &mut u8,
    airtime: &mut AirtimeLedger,
    throughput: &mut ThroughputLedger,
    status: &EmbassyInterfaceStatus,
    profile: &RadioProfile,
    now: InstantMillis,
    tx_frame: &mut [u8; LORA_SINGLE_FRAME_MAX],
) -> Result<(), sx126x::Error>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    for index in 0..air_frame_count(packet.len()) {
        let n = match encode_air_frame_part(packet, *seq, index, tx_frame) {
            Ok(n) => n,
            Err(e) => {
                crate::diagnostic_log::debug!("RNS_LORA frame {index} encode failed: {e:?}");
                break;
            }
        };
        if let Err(e) = radio.transmit(&tx_frame[..n]).await {
            crate::diagnostic_log::debug!("RNS_LORA tx failed: {e:?}");
            *seq = seq.wrapping_add(0x10);
            return Err(e);
        }
        status.add_tx(n as u64);
        throughput.record_tx(now, n as u64);
        status.set_transfer_rates(throughput.rates());
        status.set_airtime(airtime.record_tx(now, profile.time_on_air_us(n)));
    }
    *seq = seq.wrapping_add(0x10);
    Ok(())
}

fn is_radio_fault(e: &sx126x::Error) -> bool {
    matches!(
        e,
        sx126x::Error::Busy
            | sx126x::Error::Dio1
            | sx126x::Error::Spi
            | sx126x::Error::Timeout
            | sx126x::Error::Reset
    )
}

async fn reinit_radio<SPI, BUSY, DIO1, RST, DLY>(
    radio: &mut Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    profile: &RadioProfile,
    spectrum: &LoRaSpectrumStatus,
) -> bool
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    if let Err(e) = radio.init(sx126x_config(profile)).await {
        crate::diagnostic_log::warn!("RNS_LORA hard re-init failed: {e:?}");
        return false;
    }
    if let Err(e) = radio.arm_rx().await {
        crate::diagnostic_log::warn!("RNS_LORA re-init RX arm failed: {e:?}");
        return false;
    }
    crate::diagnostic_log::warn!("RNS_LORA radio recovered via hard re-init");
    spectrum.add_radio_recovery();
    true
}

#[expect(
    clippy::too_many_arguments,
    reason = "transactional radio reconfiguration owns the full old/new policy boundary"
)]
async fn apply_profile<SPI, BUSY, DIO1, RST, DLY>(
    radio: &mut Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    requested: RadioProfile,
    airtime_policy: AirtimePolicy,
    profile: &mut RadioProfile,
    duty: &mut Option<AirtimeDutyCycle>,
    current_id: &mut InterfaceId,
    status: &EmbassyInterfaceStatus,
    spectrum: &LoRaSpectrumStatus,
    lifecycle: DynamicSender<'_, InterfaceLifecycle>,
) -> bool
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    if requested.validate().is_err() {
        crate::diagnostic_log::warn!("RNS_LORA rejected invalid profile");
        return false;
    }
    let requested_duty = match airtime_policy.resolve(requested.region) {
        Ok(duty) => duty,
        Err(_) => {
            crate::diagnostic_log::warn!("RNS_LORA rejected airtime policy");
            return false;
        }
    };
    if requested == *profile && requested_duty == *duty {
        return true;
    }

    let previous = *profile;
    if let Err(error) = radio.init(sx126x_config(&requested)).await {
        crate::diagnostic_log::warn!(
            "RNS_LORA reconfigure init failed: {error:?}; restoring prior profile"
        );
        if !reinit_radio(radio, &previous, spectrum).await {
            status.set_connection(ConnectionState::Disconnected);
        }
        return false;
    }
    if let Err(error) = radio.arm_rx().await {
        crate::diagnostic_log::warn!(
            "RNS_LORA reconfigure RX arm failed: {error:?}; restoring prior profile"
        );
        if !reinit_radio(radio, &previous, spectrum).await {
            status.set_connection(ConnectionState::Disconnected);
        }
        return false;
    }

    *profile = requested;
    *duty = requested_duty;
    if let Some(message) = retag_message(*current_id, profile, requested_duty) {
        if let InterfaceLifecycle::Retag { new_id, .. } = &message {
            *current_id = *new_id;
            status.set_id(*new_id);
        }
        lifecycle.send(message).await;
    } else {
        lifecycle
            .send(InterfaceLifecycle::Update {
                descriptor: lora::descriptor(*current_id, profile, requested_duty),
            })
            .await;
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaConfigError {
    Profile(RadioProfileError),
    AirtimePolicy(AirtimePolicyError),
}

pub struct LoRaInterfaceInput<'a, SPI, BUSY, DIO1, RST, DLY> {
    pub radio: Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    pub profile: RadioProfile,
    pub airtime_policy: AirtimePolicy,
    pub control: &'a LoRaControl,
    pub status: &'a EmbassyInterfaceStatus,
    pub spectrum: &'a LoRaSpectrumStatus,
    pub lifecycle: DynamicSender<'a, InterfaceLifecycle>,
}

pub struct LoRaInterface<'a, SPI, BUSY, DIO1, RST, DLY> {
    id: InterfaceId,
    radio: Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    profile: RadioProfile,
    airtime_policy: AirtimePolicy,
    duty: Option<AirtimeDutyCycle>,
    tag: HeaplessVec<u8, CHANNEL_TAG_CAP>,
    control: &'a LoRaControl,
    status: &'a EmbassyInterfaceStatus,
    spectrum: &'a LoRaSpectrumStatus,
    lifecycle: DynamicSender<'a, InterfaceLifecycle>,
}

impl<'a, SPI, BUSY, DIO1, RST, DLY> LoRaInterface<'a, SPI, BUSY, DIO1, RST, DLY> {
    /// The id a radio on `profile` will carry — for the caller that stands its [`EmbassyInterfaceStatus`] up under the same key before building the interface.
    #[must_use]
    pub fn interface_id(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &lora::channel_tag(profile))
    }

    pub fn new(
        input: LoRaInterfaceInput<'a, SPI, BUSY, DIO1, RST, DLY>,
    ) -> Result<Self, LoRaConfigError> {
        let LoRaInterfaceInput {
            radio,
            profile,
            airtime_policy,
            control,
            status,
            spectrum,
            lifecycle,
        } = input;
        profile.validate().map_err(LoRaConfigError::Profile)?;
        let tag = lora::channel_tag(&profile);
        let id = Self::interface_id(&profile);
        let duty = airtime_policy
            .resolve(profile.region)
            .map_err(LoRaConfigError::AirtimePolicy)?;
        Ok(Self {
            id,
            radio,
            profile,
            airtime_policy,
            duty,
            tag,
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

impl<SPI, BUSY, DIO1, RST, DLY> Interface for LoRaInterface<'_, SPI, BUSY, DIO1, RST, DLY>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    const HW_MTU: usize = LORA_MAX_PAYLOAD;
    const KIND: InterfaceKind = InterfaceKind::LoRa;

    fn descriptor(&self) -> InterfaceDescriptor {
        lora::descriptor(self.id, &self.profile, self.duty)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let LoRaInterface {
            id,
            mut radio,
            mut profile,
            airtime_policy,
            duty,
            tag: _,
            control,
            status,
            spectrum,
            lifecycle,
        } = self;
        let mut current_id = id;

        if let Err(e) = radio.init(sx126x_config(&profile)).await {
            crate::diagnostic_log::error!("RNS_LORA radio init failed: {e:?}; interface offline");
            status.set_connection(ConnectionState::Disconnected);
            return;
        }

        let mut reassembler = LoRaReassembler::<LORA_MAX_PAYLOAD>::new();
        let mut rx_buf = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut tx_frame = [0u8; LORA_SINGLE_FRAME_MAX];
        let mut seq: u8 = 0;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let mut duty_cycle = duty;
        let mut noise = NoiseFloor::new();
        let mut activity = DemodulatorActivity::new();
        let started = Instant::now();
        status.set_connection(ConnectionState::Connected);
        if let Err(e) = radio.arm_rx().await {
            crate::diagnostic_log::debug!("RNS_LORA initial RX arm failed: {e:?}");
        }

        let mut pending_buf = [0u8; LORA_MAX_PAYLOAD];
        let mut pending_len: Option<usize> = None;
        let mut pending_airtime_us = 0u64;
        let mut pending_enqueued_at_ms = 0u64;
        let mut access: Option<ChannelAccess> = None;
        let mut access_suspended = true;
        let mut duty_was_held = false;
        let mut reported_deferrals = 0u32;
        let mut post_tx_yield_until_ms = 0u64;

        loop {
            if !status.is_enabled() {
                status.set_connection(ConnectionState::Disabled);
                reassembler = LoRaReassembler::new();
                activity.frame_finished();
                noise = NoiseFloor::new();
                if pending_len.take().is_some() {
                    access = None;
                    seam.complete_outbound(OutboundDisposition::Dropped(
                        OutboundDropReason::Disabled,
                    ));
                }
                status.wait_until_enabled().await;
                status.set_connection(ConnectionState::Connected);
                if let Err(e) = radio.arm_rx().await {
                    crate::diagnostic_log::debug!("RNS_LORA RX re-arm after enable failed: {e:?}");
                    if is_radio_fault(&e) {
                        reinit_radio(&mut radio, &profile, spectrum).await;
                    }
                }
            }

            if let Some(len) = pending_len {
                let before_wait = InstantMillis(started.elapsed().as_millis());
                let projected = airtime.projected_utilization(before_wait, pending_airtime_us);
                let duty_permits = duty_cycle.is_none_or(|duty| duty.permits(projected));
                if !duty_permits && !duty_was_held {
                    spectrum.add_duty_hold();
                }
                duty_was_held = !duty_permits;
                let yield_complete = before_wait.0 >= post_tx_yield_until_ms;
                let suspended_now = !duty_permits || !yield_complete;
                if access_suspended && !suspended_now {
                    if let Some(access) = access.as_mut() {
                        access.restart_contention(before_wait.0);
                    }
                }
                access_suspended = suspended_now;

                let expired = access
                    .as_ref()
                    .is_some_and(|access| access.is_expired(before_wait.0));
                if expired {
                    let reason = if duty_permits {
                        spectrum.add_contention_timeout();
                        OutboundDropReason::ContentionTimeout
                    } else {
                        spectrum.add_duty_timeout();
                        OutboundDropReason::DutyLimited
                    };
                    pending_len = None;
                    access = None;
                    seam.complete_outbound(OutboundDisposition::Dropped(reason));
                    continue;
                }

                let tick_ms = if access_suspended {
                    if !yield_complete {
                        post_tx_yield_until_ms
                            .saturating_sub(before_wait.0)
                            .min(IDLE_TICK.as_millis())
                            .max(1)
                    } else {
                        IDLE_TICK.as_millis()
                    }
                } else {
                    access
                        .as_ref()
                        .map_or(IDLE_TICK.as_millis(), |access| access.timing().sample_ms())
                };

                let mut next_action = None;
                match select4(
                    control.wait(),
                    status.wait_until_disabled(),
                    radio.read_event(&mut rx_buf),
                    Timer::after(Duration::from_millis(tick_ms)),
                )
                .await
                {
                    Either4::First(new_profile) => {
                        let changed = apply_profile(
                            &mut radio,
                            new_profile,
                            airtime_policy,
                            &mut profile,
                            &mut duty_cycle,
                            &mut current_id,
                            status,
                            spectrum,
                            lifecycle,
                        )
                        .await;
                        if changed {
                            let now = InstantMillis(started.elapsed().as_millis());
                            reassembler = LoRaReassembler::new();
                            activity.frame_finished();
                            noise = NoiseFloor::new();
                            pending_airtime_us = packet_airtime(&pending_buf[..len], &profile);
                            access = Some(ChannelAccess::new_at(
                                profile,
                                now.0,
                                pending_enqueued_at_ms,
                                pending_airtime_us,
                            ));
                            access_suspended = true;
                            duty_was_held = false;
                            reported_deferrals = 0;
                        }
                    }
                    Either4::Second(()) => continue,
                    Either4::Third(Ok(event)) => {
                        let now = InstantMillis(started.elapsed().as_millis());
                        let observation = observe_radio_event(
                            event,
                            now,
                            ReceivePath {
                                profile: &profile,
                                activity: &mut activity,
                                spectrum,
                                rx_buf: &rx_buf,
                                status,
                                throughput: &mut throughput,
                                reassembler: &mut reassembler,
                                seam: &mut seam,
                            },
                        )
                        .await;
                        if !access_suspended {
                            let utilization = airtime.utilization(now);
                            if let Some(access) = access.as_mut() {
                                let action =
                                    access.observe(now.0, observation, utilization.short_per_mille);
                                next_action = Some(
                                    if matches!(action, ChannelAccessAction::NeedBackoffEntropy) {
                                        choose_backoff_entropy(access, &mut seam)
                                    } else {
                                        action
                                    },
                                );
                            }
                        }
                    }
                    Either4::Third(Err(error)) => {
                        crate::diagnostic_log::debug!("RNS_LORA rx event error: {error:?}");
                        activity.frame_finished();
                        if is_radio_fault(&error) {
                            reinit_radio(&mut radio, &profile, spectrum).await;
                        }
                        if !access_suspended {
                            let now = InstantMillis(started.elapsed().as_millis());
                            let utilization = airtime.utilization(now);
                            if let Some(access) = access.as_mut() {
                                next_action = Some(access.observe(
                                    now.0,
                                    noise.fail_closed(),
                                    utilization.short_per_mille,
                                ));
                            }
                        }
                    }
                    Either4::Fourth(()) => {
                        let now = InstantMillis(started.elapsed().as_millis());
                        let observation = match sample_channel(
                            &mut radio,
                            now,
                            &mut activity,
                            spectrum,
                            &mut noise,
                        )
                        .await
                        {
                            Ok(observation) => observation,
                            Err(error) => {
                                crate::diagnostic_log::debug!(
                                    "RNS_LORA channel sample failed: {error:?}"
                                );
                                activity.frame_finished();
                                if is_radio_fault(&error) {
                                    reinit_radio(&mut radio, &profile, spectrum).await;
                                }
                                noise.fail_closed()
                            }
                        };
                        if !access_suspended {
                            let utilization = airtime.utilization(now);
                            if let Some(access) = access.as_mut() {
                                let action =
                                    access.observe(now.0, observation, utilization.short_per_mille);
                                next_action = Some(
                                    if matches!(action, ChannelAccessAction::NeedBackoffEntropy) {
                                        choose_backoff_entropy(access, &mut seam)
                                    } else {
                                        action
                                    },
                                );
                            }
                        }
                    }
                }

                if matches!(next_action, Some(ChannelAccessAction::ReadyForFinalCheck)) {
                    let now = InstantMillis(started.elapsed().as_millis());
                    let observation = match radio.poll_event(&mut rx_buf).await {
                        Ok(Some(event)) => {
                            observe_radio_event(
                                event,
                                now,
                                ReceivePath {
                                    profile: &profile,
                                    activity: &mut activity,
                                    spectrum,
                                    rx_buf: &rx_buf,
                                    status,
                                    throughput: &mut throughput,
                                    reassembler: &mut reassembler,
                                    seam: &mut seam,
                                },
                            )
                            .await
                        }
                        Ok(None) => match sample_channel(
                            &mut radio,
                            now,
                            &mut activity,
                            spectrum,
                            &mut noise,
                        )
                        .await
                        {
                            Ok(observation) => observation,
                            Err(error) => {
                                crate::diagnostic_log::debug!(
                                    "RNS_LORA final channel check failed: {error:?}"
                                );
                                activity.frame_finished();
                                if is_radio_fault(&error) {
                                    reinit_radio(&mut radio, &profile, spectrum).await;
                                }
                                noise.fail_closed()
                            }
                        },
                        Err(error) => {
                            crate::diagnostic_log::debug!(
                                "RNS_LORA final IRQ check failed: {error:?}"
                            );
                            activity.frame_finished();
                            if is_radio_fault(&error) {
                                reinit_radio(&mut radio, &profile, spectrum).await;
                            }
                            noise.fail_closed()
                        }
                    };
                    if let Some(access) = access.as_mut() {
                        next_action = Some(access.final_check(now.0, observation));
                    }
                }

                if let Some(access) = access.as_ref() {
                    let deferrals = access.deferrals();
                    if deferrals > reported_deferrals {
                        spectrum.add_deferrals(deferrals - reported_deferrals);
                        reported_deferrals = deferrals;
                    }
                }

                match next_action {
                    Some(ChannelAccessAction::Transmit) => {
                        let now = InstantMillis(started.elapsed().as_millis());
                        let timing = access
                            .as_ref()
                            .map_or(ChannelTiming::for_profile(profile), ChannelAccess::timing);
                        let tx = transmit_packet(
                            &mut radio,
                            &pending_buf[..len],
                            &mut seq,
                            &mut airtime,
                            &mut throughput,
                            status,
                            &profile,
                            now,
                            &mut tx_frame,
                        )
                        .await;
                        let disposition = match tx {
                            Ok(()) => OutboundDisposition::Sent,
                            Err(error) => {
                                if is_radio_fault(&error) {
                                    reinit_radio(&mut radio, &profile, spectrum).await;
                                }
                                OutboundDisposition::Dropped(OutboundDropReason::TransportFailure)
                            }
                        };
                        if let Err(error) = radio.arm_rx().await {
                            crate::diagnostic_log::debug!(
                                "RNS_LORA RX re-arm after tx failed: {error:?}"
                            );
                            if is_radio_fault(&error) {
                                reinit_radio(&mut radio, &profile, spectrum).await;
                            }
                        }
                        activity.frame_finished();
                        let completed_at_ms = started.elapsed().as_millis();
                        post_tx_yield_until_ms =
                            completed_at_ms.saturating_add(timing.post_tx_yield_ms());
                        pending_len = None;
                        access = None;
                        access_suspended = true;
                        duty_was_held = false;
                        reported_deferrals = 0;
                        seam.complete_outbound(disposition);
                    }
                    Some(ChannelAccessAction::Expired) => {
                        spectrum.add_contention_timeout();
                        pending_len = None;
                        access = None;
                        seam.complete_outbound(OutboundDisposition::Dropped(
                            OutboundDropReason::ContentionTimeout,
                        ));
                    }
                    Some(
                        ChannelAccessAction::Wait
                        | ChannelAccessAction::NeedBackoffEntropy
                        | ChannelAccessAction::ReadyForFinalCheck,
                    )
                    | None => {}
                }
            } else {
                let idle_tick = if noise.is_calibrated() {
                    IDLE_TICK
                } else {
                    Duration::from_millis(ChannelTiming::for_profile(profile).sample_ms())
                };
                match select4(
                    control.wait(),
                    status.wait_until_disabled(),
                    radio.read_event(&mut rx_buf),
                    select(seam.next_outbound(), Timer::after(idle_tick)),
                )
                .await
                {
                    Either4::First(new_profile) => {
                        let changed = apply_profile(
                            &mut radio,
                            new_profile,
                            airtime_policy,
                            &mut profile,
                            &mut duty_cycle,
                            &mut current_id,
                            status,
                            spectrum,
                            lifecycle,
                        )
                        .await;
                        if changed {
                            reassembler = LoRaReassembler::new();
                            activity.frame_finished();
                            noise = NoiseFloor::new();
                        }
                    }
                    Either4::Second(()) => continue,
                    Either4::Third(Ok(event)) => {
                        let now = InstantMillis(started.elapsed().as_millis());
                        let _ = observe_radio_event(
                            event,
                            now,
                            ReceivePath {
                                profile: &profile,
                                activity: &mut activity,
                                spectrum,
                                rx_buf: &rx_buf,
                                status,
                                throughput: &mut throughput,
                                reassembler: &mut reassembler,
                                seam: &mut seam,
                            },
                        )
                        .await;
                    }
                    Either4::Third(Err(e)) => {
                        crate::diagnostic_log::debug!("RNS_LORA rx event error: {e:?}");
                        activity.frame_finished();
                        if is_radio_fault(&e) {
                            reinit_radio(&mut radio, &profile, spectrum).await;
                        }
                    }
                    Either4::Fourth(Either::First(outbound)) => {
                        if outbound.len() > pending_buf.len() {
                            seam.complete_outbound(OutboundDisposition::Dropped(
                                OutboundDropReason::Rejected,
                            ));
                            continue;
                        }
                        let now = InstantMillis(started.elapsed().as_millis());
                        let len = outbound.len();
                        pending_buf[..len].copy_from_slice(&outbound[..len]);
                        pending_airtime_us = packet_airtime(&pending_buf[..len], &profile);
                        pending_enqueued_at_ms = now.0;
                        pending_len = Some(len);
                        access = Some(ChannelAccess::new(profile, now.0, pending_airtime_us));
                        access_suspended = true;
                        duty_was_held = false;
                        reported_deferrals = 0;
                    }
                    Either4::Fourth(Either::Second(())) => {
                        let now = InstantMillis(started.elapsed().as_millis());
                        if let Err(error) =
                            sample_channel(&mut radio, now, &mut activity, spectrum, &mut noise)
                                .await
                        {
                            crate::diagnostic_log::debug!(
                                "RNS_LORA idle channel sample failed: {error:?}"
                            );
                            activity.frame_finished();
                            if is_radio_fault(&error) {
                                reinit_radio(&mut radio, &profile, spectrum).await;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::lora::{PreambleSymbols, TxPower, DEFAULT_915_PROFILE};

    fn id_of(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &lora::channel_tag(profile))
    }

    #[test]
    fn a_channel_change_re_keys_to_the_new_id() {
        let current = id_of(&DEFAULT_915_PROFILE);
        let mut next = DEFAULT_915_PROFILE;
        next.modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf10,
            bandwidth: LoraBandwidth::Bw125kHz,
            coding_rate: CodingRate::Cr45,
        };
        let message = retag_message(current, &next, None).expect("a channel change re-keys");
        let InterfaceLifecycle::Retag { old_id, new_id, .. } = message else {
            panic!("expected a Retag");
        };
        assert_eq!(old_id, current);
        assert_eq!(new_id, id_of(&next));
        assert_ne!(new_id, current);
    }

    #[test]
    fn a_local_only_change_does_not_re_key() {
        let current = id_of(&DEFAULT_915_PROFILE);
        let mut next = DEFAULT_915_PROFILE;
        next.tx_power = TxPower::new(2);
        next.preamble = PreambleSymbols::new(24);
        assert!(
            retag_message(current, &next, None).is_none(),
            "transmit power and preamble are local knobs, not channel identity"
        );
    }

    #[test]
    fn packet_airtime_sums_both_frames_of_a_split() {
        let profile = lora::DEFAULT_915_PROFILE;
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
    fn sx126x_packet_uses_the_rnode_sync_word() {
        let config = sx126x_config(&DEFAULT_915_PROFILE);
        assert_eq!(config.sync_word, RNODE_LORA_SYNC_WORD);
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
            }
        );
    }
}
