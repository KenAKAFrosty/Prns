//! The embassy ESP-NOW worker: bridges a board's [`EspNowRadio`] to the reactor seam as a
//! broadcast-only interface. Generic over the radio trait, so it compile-checks off-target and the
//! concrete esp-radio handle is supplied by the board crate. Far simpler than the LoRa worker — no
//! CSMA, no duty gate, no split/reassembly: the silicon owns channel access and fragmentation, so a
//! frame is broadcast and received whole.

use alloc::boxed::Box;

use embassy_futures::select::{select3, Either3};
use embassy_time::{Duration, Instant, Timer};
use heapless::Vec as HeaplessVec;

use crate::engine::InstantMillis;
use crate::interfaces::esp_now::core::{
    self, ChannelPolicy, EspNowRadio, CHANNEL_TAG_CAP, ESP_NOW_HW_MTU, ESP_NOW_V2_AIR_MTU,
};
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::impls::embassy_reactor::EmbassyInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// How often the worker re-checks its enable gate, so a "Power" toggle takes effect within a beat
/// rather than waiting on traffic.
const ENABLED_POLL: Duration = Duration::from_millis(250);

/// One ESP-NOW radio spoken as a broadcast Reticulum interface. Owns the radio for its whole life;
/// `policy` decides whether it parks on a fixed channel or follows the WiFi station; `status` carries
/// its enable gate and counters.
pub struct EspNowInterface<'a, R> {
    id: InterfaceId,
    radio: R,
    policy: ChannelPolicy,
    tag: HeaplessVec<u8, CHANNEL_TAG_CAP>,
    status: &'a EmbassyInterfaceStatus,
}

impl<'a, R> EspNowInterface<'a, R> {
    #[must_use]
    pub fn new(radio: R, policy: ChannelPolicy, status: &'a EmbassyInterfaceStatus) -> Self {
        Self {
            id: core::interface_id(),
            radio,
            policy,
            tag: core::channel_tag(),
            status,
        }
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// The id this interface will carry — for the caller that stands its
    /// [`EmbassyInterfaceStatus`] up under the same key before building the interface.
    #[must_use]
    pub fn interface_id() -> InterfaceId {
        core::interface_id()
    }
}

impl<R: EspNowRadio> Interface for EspNowInterface<'_, R> {
    const HW_MTU: usize = ESP_NOW_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::EspNow;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let EspNowInterface {
            mut radio,
            policy,
            status,
            ..
        } = self;
        if let ChannelPolicy::Fixed(channel) = policy {
            radio.set_channel(channel);
        }

        let mut rx_buf = Box::new([0u8; ESP_NOW_V2_AIR_MTU]);
        let mut throughput = ThroughputLedger::new();
        let started = Instant::now();
        status.set_connection(ConnectionState::Connected);
        log::info!("RNS_ESPNOW interface up, policy {policy:?}");

        loop {
            if !status.is_enabled() {
                status.set_connection(ConnectionState::Disabled);
                while !status.is_enabled() {
                    Timer::after(ENABLED_POLL).await;
                }
                status.set_connection(ConnectionState::Connected);
            }

            match select3(
                radio.receive(&mut rx_buf[..]),
                seam.next_outbound(),
                Timer::after(ENABLED_POLL),
            )
            .await
            {
                Either3::First(len) => {
                    if len > 0 {
                        let now = InstantMillis(started.elapsed().as_millis());
                        status.add_rx(len as u64);
                        throughput.record_rx(now, len as u64);
                        status.set_transfer_rates(throughput.rates());
                        seam.next_inbound(&rx_buf[..len]).await;
                    }
                }
                Either3::Second(outbound) => {
                    let len = outbound.len().min(ESP_NOW_V2_AIR_MTU);
                    if radio.broadcast(&outbound[..len]).await {
                        let now = InstantMillis(started.elapsed().as_millis());
                        status.add_tx(len as u64);
                        throughput.record_tx(now, len as u64);
                        status.set_transfer_rates(throughput.rates());
                    }
                }
                Either3::Third(()) => {}
            }
        }
    }
}
