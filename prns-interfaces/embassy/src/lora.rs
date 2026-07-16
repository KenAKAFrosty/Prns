//! The embassy SX1262 worker: owns the radio over our own `sx126x` driver, bridges its RX/TX
//! to the reactor seam, and reconfigures the radio in place when the app signals a new profile.
//! Generic over the driver's `embedded-hal-async` bounds, so the same body drives a Heltec V4
//! (esp-hal) or an nRF board and compile-checks on the host. A reconfigure that changes the
//! channel identity emits a `Retag` so the reactor re-keys the interface in place.

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

use prns_core::engine::InstantMillis;
use prns_core::interfaces::lora::core::{
    self, air_frame_count, encode_air_frame_part, CodingRate, LoRaReassembler, LoraBandwidth,
    Modulation, RadioProfile, SpreadingFactor, CHANNEL_TAG_CAP, LORA_MAX_PAYLOAD,
    LORA_SINGLE_FRAME_MAX,
};
use prns_core::interfaces::radios::sx126x::{self, Sx126x};
use prns_core::interfaces::{
    AirtimeDutyCycle, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_core::reactor::airtime::AirtimeLedger;
use prns_core::reactor::duty_gate::{DutyGate, DutyVerdict, FixedDutyQueue};
use prns_core::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_core::reactor::throughput::ThroughputLedger;
use prns_runtime::reactor::impls::embassy_reactor::{EmbassyInterfaceStatus, InterfaceLifecycle};

/// How often a serving radio re-checks its enabled gate, so a "Power" toggle from the UI takes
/// effect within a beat rather than waiting on traffic.
pub const ENABLED_POLL: Duration = Duration::from_millis(250);
/// How many held packets the duty gate buffers before dropping the oldest. The region's airtime
/// budget caps the queue sooner on a slow link, so this only bounds the buffer's memory.
const DUTY_QUEUE_FRAMES: usize = 4;

/// CSMA/CA channel-access timing, matching the RNode firmware (`Config.h`): hold the channel clear
/// for DIFS before contending, then back off a random number of slots in `[0, CW_MAX]`. A peer
/// transmitting trips the carrier sense and restarts the backoff, so two nodes don't talk over each
/// other. These pace channel ACCESS (collision avoidance); the [`DutyGate`] paces airtime USE
/// (regulatory) — orthogonal gates.
const CSMA_DIFS_MS: u64 = 48;
const CSMA_SLOT_MS: u64 = 24;
const CSMA_CW_MAX: u32 = 15;
/// After this many backoff restarts on a channel that stays busy, transmit anyway — a stuck or
/// jammed channel mustn't starve the node forever. ~16 restarts ≈ several seconds of deferral.
const CSMA_MAX_RESTARTS: u32 = 16;
/// RSSI (dBm) at or above which a frame is judged to be on air, so the transmit holds off. Well
/// over the BW125 noise floor (~-120 dBm), under a desk-adjacent peer (~-40 dBm).
const CHANNEL_BUSY_DBM: i16 = -95;

/// One CSMA contention budget: DIFS plus a random `[0, CW_MAX]`-slot backoff, in ms. The channel
/// must stay clear this long before the frame goes out. The `now`-mix de-phases two un-synced nodes
/// so they don't pick the same backoff every contest.
fn csma_budget_ms(rng: &mut u32, now: InstantMillis) -> u64 {
    if *rng == 0 {
        *rng = (now.0 as u32) | 1;
    }
    let mut x = *rng;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *rng = x;
    let slots = (x ^ now.0 as u32) % (CSMA_CW_MAX + 1);
    CSMA_DIFS_MS + slots as u64 * CSMA_SLOT_MS
}

/// Carrier sense: is a frame on air right now? A transient RSSI-read error reads as clear — never
/// wedge a transmit on an SPI hiccup. Requires the radio armed in RX (the interface always is).
async fn channel_busy<SPI, BUSY, DIO1, RST, DLY>(
    radio: &mut Sx126x<SPI, BUSY, DIO1, RST, DLY>,
) -> bool
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    matches!(radio.channel_rssi_dbm().await, Ok(rssi) if rssi >= CHANNEL_BUSY_DBM)
}

/// Deliver one received frame: count it, update rates, and hand a reassembled packet to the seam.
/// Shared by the idle and contending paths so a frame heard while we're backing off to transmit is
/// received the same as any other — the whole point of the integrated contention.
async fn deliver_rx<Seam: InterfaceSeam>(
    rx: &[u8],
    now: InstantMillis,
    status: &EmbassyInterfaceStatus,
    throughput: &mut ThroughputLedger,
    reassembler: &mut LoRaReassembler<LORA_MAX_PAYLOAD>,
    seam: &mut Seam,
) {
    status.add_rx(rx.len() as u64);
    throughput.record_rx(now, rx.len() as u64);
    status.set_transfer_rates(throughput.rates());
    if let Some(packet) = reassembler.feed(rx) {
        if !packet.is_empty() && packet.len() <= LORA_MAX_PAYLOAD {
            seam.next_inbound(packet).await;
        }
    }
}

/// The signal the app holds to reconfigure a running radio: it sends a whole new [`RadioProfile`]
/// and the worker rebuilds the silicon's params from it. The granular, menu-shaped control surface
/// (set-frequency, set-modulation, …) lands with the reconfigure arc.
pub type LoRaControl = Signal<CriticalSectionRawMutex, RadioProfile>;

/// Map a profile's LoRa channel onto our `sx126x` driver's `init` arguments —
/// `(frequency_hz, modulation, packet shape, tx power dBm)`.
fn subghz_params(
    profile: &RadioProfile,
) -> Option<(u32, sx126x::Modulation, sx126x::LoraPacket, i8)> {
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
    let packet = sx126x::LoraPacket {
        preamble_symbols: profile.preamble.count(),
        explicit_header: true,
        crc_on: true,
        invert_iq: false,
    };
    Some((
        profile.frequency.hz(),
        sx126x::Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        packet,
        profile.tx_power.dbm(),
    ))
}

/// The [`Retag`](InterfaceLifecycle::Retag) a reconfigure to `new_profile` warrants, or `None` when
/// the change leaves the channel identity untouched — a local knob like transmit power or preamble.
/// The channel_tag (frequency + modulation) is what mints the id, so only a change to it re-keys.
fn retag_message(
    current_id: InterfaceId,
    new_profile: &RadioProfile,
    duty: Option<AirtimeDutyCycle>,
) -> Option<InterfaceLifecycle> {
    let new_id =
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(new_profile));
    (new_id != current_id).then(|| InterfaceLifecycle::Retag {
        old_id: current_id,
        new_id,
        descriptor: core::descriptor(new_id, new_profile, duty),
    })
}

/// The summed on-air airtime of a packet's frames (1 or 2) — the currency the duty gate budgets in.
fn packet_airtime(packet: &[u8], profile: &core::RadioProfile) -> u64 {
    let mut scratch = [0u8; LORA_SINGLE_FRAME_MAX];
    let mut total = 0;
    for index in 0..air_frame_count(packet.len()) {
        if let Ok(n) = encode_air_frame_part(packet, 0, index, &mut scratch) {
            total += profile.time_on_air_us(n);
        }
    }
    total
}

/// Split a packet into its RNode frames and transmit each, recording airtime and throughput. Shared
/// by the immediate path and the duty gate's release, so a held packet rides out byte-for-byte the
/// same as an unheld one.
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
    profile: &core::RadioProfile,
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
        // TX power, modulation, and packet shape are held in the radio from `init`; only the
        // framed payload changes per frame. A transmit error is returned so the worker can hard-reset
        // a faulted radio instead of re-arming a wedged one.
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

/// A radio error that means the silicon has wedged (vs. ordinary RF outcomes like a CRC miss or a
/// frame too big for the buffer), so the worker should hard-reset and re-init it rather than just
/// re-arm. Bounded waits in the driver surface a stuck BUSY/IRQ line as one of these.
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

/// Recover a wedged radio: hard-reset and re-run the full init from the live profile, then re-arm RX.
/// The driver's waits are all bounded, so this can't itself hang — if the chip is truly dead it just
/// returns `false` and the worker keeps looping (so a later retune, re-enable, or fault retries it).
async fn reinit_radio<SPI, BUSY, DIO1, RST, DLY>(
    radio: &mut Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    profile: &RadioProfile,
) -> bool
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    let Some((frequency_hz, modulation, packet, power_dbm)) = subghz_params(profile) else {
        return false;
    };
    if let Err(e) = radio
        .init(frequency_hz, modulation, packet, power_dbm)
        .await
    {
        crate::diagnostic_log::warn!("RNS_LORA hard re-init failed: {e:?}");
        return false;
    }
    if let Err(e) = radio.arm_rx().await {
        crate::diagnostic_log::warn!("RNS_LORA re-init RX arm failed: {e:?}");
        return false;
    }
    crate::diagnostic_log::warn!("RNS_LORA radio recovered via hard re-init");
    true
}

/// One SX1262 spoken as an RNode-compatible LoRa interface. Owns the radio for its whole life; the
/// `control` signal lets the app retune it live, the `status` handle carries its enable gate and
/// counters. `tag` is the channel identity the id derived from, recomputed only on a re-key.
pub struct LoRaInterface<'a, SPI, BUSY, DIO1, RST, DLY> {
    id: InterfaceId,
    radio: Sx126x<SPI, BUSY, DIO1, RST, DLY>,
    profile: RadioProfile,
    duty: Option<AirtimeDutyCycle>,
    tag: HeaplessVec<u8, CHANNEL_TAG_CAP>,
    control: &'a LoRaControl,
    status: &'a EmbassyInterfaceStatus,
    retag: DynamicSender<'a, InterfaceLifecycle>,
}

impl<'a, SPI, BUSY, DIO1, RST, DLY> LoRaInterface<'a, SPI, BUSY, DIO1, RST, DLY> {
    /// The id a radio on `profile` will carry — for the caller that stands its
    /// [`EmbassyInterfaceStatus`] up under the same key before building the interface.
    #[must_use]
    pub fn interface_id(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(profile))
    }

    #[must_use]
    pub fn new(
        radio: Sx126x<SPI, BUSY, DIO1, RST, DLY>,
        profile: RadioProfile,
        control: &'a LoRaControl,
        status: &'a EmbassyInterfaceStatus,
        retag: DynamicSender<'a, InterfaceLifecycle>,
    ) -> Self {
        let tag = core::channel_tag(&profile);
        let id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &tag);
        let duty = profile.region.regulatory_duty_cycle();
        Self {
            id,
            radio,
            profile,
            duty,
            tag,
            control,
            status,
            retag,
        }
    }

    #[must_use]
    pub fn with_duty_cycle(mut self, duty: Option<AirtimeDutyCycle>) -> Self {
        self.duty = duty;
        self
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
        core::descriptor(self.id, &self.profile, self.duty)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let LoRaInterface {
            id,
            mut radio,
            mut profile,
            duty,
            tag: _,
            control,
            status,
            retag,
        } = self;
        let mut current_id = id;

        let Some((frequency_hz, modulation, packet, power_dbm)) = subghz_params(&profile) else {
            crate::diagnostic_log::error!(
                "RNS_LORA unsupported modulation profile; interface offline"
            );
            status.set_connection(ConnectionState::Disconnected);
            return;
        };
        if let Err(e) = radio
            .init(frequency_hz, modulation, packet, power_dbm)
            .await
        {
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
        let duty_cycle = duty;
        let mut gate: DutyGate<FixedDutyQueue<DUTY_QUEUE_FRAMES, LORA_MAX_PAYLOAD>> =
            DutyGate::new();
        let started = Instant::now();
        status.set_connection(ConnectionState::Connected);
        // Arm continuous RX ONCE. The select below waits on `read_frame` without re-arming, so a
        // poll tick that cancels the read leaves the radio still receiving (the RxDone IRQ latches)
        // rather than guillotining a long packet mid-air — the difference between hearing a
        // multi-hundred-ms LoRa announce and never completing one. Re-armed only after a TX or a
        // reconfigure, which genuinely leave RX.
        if let Err(e) = radio.arm_rx().await {
            crate::diagnostic_log::debug!("RNS_LORA initial RX arm failed: {e:?}");
        }

        // The CSMA contender: at most one packet in flight. While `pending_len` is set the worker is
        // backing off to send it — and still listening, so a frame heard mid-contention is received
        // and restarts the backoff instead of being transmitted over.
        let mut pending_buf = [0u8; LORA_MAX_PAYLOAD];
        let mut pending_len: Option<usize> = None;
        let mut csma_rng: u32 = 0;
        let mut csma_remaining_ms: u64 = 0;
        let mut csma_restarts: u32 = 0;

        loop {
            if !status.is_enabled() {
                status.set_connection(ConnectionState::Disabled);
                while !status.is_enabled() {
                    Timer::after(ENABLED_POLL).await;
                }
                status.set_connection(ConnectionState::Connected);
                pending_len = None;
                if let Err(e) = radio.arm_rx().await {
                    crate::diagnostic_log::debug!("RNS_LORA RX re-arm after enable failed: {e:?}");
                    if is_radio_fault(&e) {
                        reinit_radio(&mut radio, &profile).await;
                    }
                }
            }

            if let Some(len) = pending_len {
                // CONTENDING: win the channel for one packet. Race the WHOLE backoff window against an
                // incoming frame — `read_frame` runs uninterrupted (never chopped into sense slices,
                // which corrupted long receptions). A frame completing means the channel is busy, so
                // back off again; the window elapsing frame-free, confirmed by a final carrier sense
                // for a preamble still mid-flight, means the channel is ours.
                let now = InstantMillis(started.elapsed().as_millis());
                match select(
                    radio.read_frame(&mut rx_buf),
                    Timer::after(Duration::from_millis(csma_remaining_ms)),
                )
                .await
                {
                    Either::First(Ok(received)) => {
                        deliver_rx(
                            &rx_buf[..received.len],
                            now,
                            status,
                            &mut throughput,
                            &mut reassembler,
                            &mut seam,
                        )
                        .await;
                        csma_remaining_ms = csma_budget_ms(&mut csma_rng, now);
                        csma_restarts = 0;
                    }
                    Either::First(Err(e)) => {
                        crate::diagnostic_log::debug!("RNS_LORA rx error: {e:?}");
                        if is_radio_fault(&e) {
                            reinit_radio(&mut radio, &profile).await;
                        }
                        csma_remaining_ms = csma_budget_ms(&mut csma_rng, now);
                    }
                    Either::Second(()) => {
                        // Backoff elapsed frame-free. A final carrier sense catches a frame whose
                        // preamble is mid-flight (no RxDone yet) so we don't transmit over it; the
                        // restart count is bounded so a wedged-busy channel can't starve us forever.
                        let win = if channel_busy(&mut radio).await {
                            csma_restarts += 1;
                            csma_remaining_ms = csma_budget_ms(&mut csma_rng, now);
                            csma_restarts >= CSMA_MAX_RESTARTS
                        } else {
                            true
                        };
                        if win {
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
                            match tx {
                                Err(e) if is_radio_fault(&e) => {
                                    reinit_radio(&mut radio, &profile).await;
                                }
                                _ => {
                                    if let Err(e) = radio.arm_rx().await {
                                        crate::diagnostic_log::debug!(
                                            "RNS_LORA RX re-arm after tx failed: {e:?}"
                                        );
                                        if is_radio_fault(&e) {
                                            reinit_radio(&mut radio, &profile).await;
                                        }
                                    }
                                }
                            }
                            pending_len = None;
                            csma_restarts = 0;
                        }
                    }
                }
            } else {
                // IDLE: listen, accept an outbound (airtime-gated), reconfigure, drain the duty queue.
                match select4(
                    radio.read_frame(&mut rx_buf),
                    seam.next_outbound(),
                    control.wait(),
                    Timer::after(ENABLED_POLL),
                )
                .await
                {
                    Either4::First(Ok(received)) => {
                        let now = InstantMillis(started.elapsed().as_millis());
                        deliver_rx(
                            &rx_buf[..received.len],
                            now,
                            status,
                            &mut throughput,
                            &mut reassembler,
                            &mut seam,
                        )
                        .await;
                    }
                    Either4::First(Err(e)) => {
                        crate::diagnostic_log::debug!("RNS_LORA rx error: {e:?}");
                        if is_radio_fault(&e) {
                            reinit_radio(&mut radio, &profile).await;
                        }
                    }
                    Either4::Second(outbound) => {
                        // A new outbound: airtime-gate it, then hand it to the contender.
                        let now = InstantMillis(started.elapsed().as_millis());
                        let send_now = match duty_cycle {
                            None => true,
                            Some(duty) => {
                                let air = packet_airtime(outbound, &profile);
                                let util = airtime.utilization(now);
                                matches!(
                                    gate.offer(outbound, air, util, &duty),
                                    DutyVerdict::Transmit
                                )
                            }
                        };
                        if send_now {
                            let len = outbound.len().min(pending_buf.len());
                            pending_buf[..len].copy_from_slice(&outbound[..len]);
                            pending_len = Some(len);
                            csma_remaining_ms = csma_budget_ms(&mut csma_rng, now);
                            csma_restarts = 0;
                        }
                    }
                    Either4::Third(new_profile) => match subghz_params(&new_profile) {
                        Some((frequency_hz, modulation, packet, power_dbm)) => {
                            if let Err(e) = radio
                                .init(frequency_hz, modulation, packet, power_dbm)
                                .await
                            {
                                crate::diagnostic_log::warn!(
                                    "RNS_LORA reconfigure init failed: {e:?}"
                                );
                            } else {
                                profile = new_profile;
                                if let Some(message) =
                                    retag_message(current_id, &profile, duty_cycle)
                                {
                                    if let InterfaceLifecycle::Retag { new_id, .. } = &message {
                                        current_id = *new_id;
                                        status.set_id(*new_id);
                                    }
                                    retag.send(message).await;
                                }
                            }
                            if let Err(e) = radio.arm_rx().await {
                                crate::diagnostic_log::debug!(
                                    "RNS_LORA RX re-arm after reconfigure failed: {e:?}"
                                );
                                if is_radio_fault(&e) {
                                    reinit_radio(&mut radio, &profile).await;
                                }
                            }
                        }
                        None => {
                            crate::diagnostic_log::warn!(
                                "RNS_LORA reconfigure to an unsupported modulation ignored"
                            );
                        }
                    },
                    Either4::Fourth(()) => {
                        // Idle tick: release one airtime-cleared held packet into the contender.
                        if let Some(duty) = duty_cycle {
                            let now = InstantMillis(started.elapsed().as_millis());
                            let util = airtime.utilization(now);
                            let mut released_len = None;
                            gate.release_ready(util, &duty, |bytes| {
                                let len = bytes.len().min(pending_buf.len());
                                pending_buf[..len].copy_from_slice(&bytes[..len]);
                                released_len = Some(len);
                            });
                            if let Some(len) = released_len {
                                pending_len = Some(len);
                                csma_remaining_ms = csma_budget_ms(&mut csma_rng, now);
                                csma_restarts = 0;
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
    use prns_core::interfaces::lora::core::{PreambleSymbols, TxPower, DEFAULT_915_PROFILE};

    fn id_of(profile: &RadioProfile) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::LoRa, &core::channel_tag(profile))
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
        let profile = core::DEFAULT_915_PROFILE;
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
}
