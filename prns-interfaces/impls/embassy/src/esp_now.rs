use alloc::boxed::Box;

use embassy_futures::select::{select3, Either3};
use embassy_time::Instant;
use heapless::Vec as HeaplessVec;

use prns_core::engine::InstantMillis;
use prns_core::interfaces::esp_now::{
    self, ChannelPolicy, EspNowRadio, CHANNEL_TAG_CAP, ESP_NOW_HW_MTU, ESP_NOW_V2_AIR_MTU,
};
use prns_core::interfaces::{
    BitrateBps, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::driver::EmbassyInterfaceStatus;
use prns_runtime::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use prns_runtime::manifold::throughput::ThroughputLedger;

pub struct EspNowInterface<'a, R> {
    id: InterfaceId,
    radio: R,
    policy: ChannelPolicy,
    bitrate: BitrateBps,
    tag: HeaplessVec<u8, CHANNEL_TAG_CAP>,
    status: &'a EmbassyInterfaceStatus,
}

impl<'a, R> EspNowInterface<'a, R> {
    #[must_use]
    pub fn new(
        radio: R,
        policy: ChannelPolicy,
        bitrate: BitrateBps,
        status: &'a EmbassyInterfaceStatus,
    ) -> Self {
        Self {
            id: esp_now::interface_id(),
            radio,
            policy,
            bitrate,
            tag: esp_now::channel_tag(),
            status,
        }
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// The id this interface will carry — for the caller that stands its [`EmbassyInterfaceStatus`] up under the same key before building the interface.
    #[must_use]
    pub fn interface_id() -> InterfaceId {
        esp_now::interface_id()
    }
}

impl<R: EspNowRadio> Interface for EspNowInterface<'_, R> {
    const HW_MTU: usize = ESP_NOW_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::EspNow;

    fn descriptor(&self) -> InterfaceDescriptor {
        esp_now::descriptor(self.id, self.bitrate)
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
        crate::diagnostic_log::info!("RNS_ESPNOW interface up, policy {policy:?}");

        loop {
            if !status.is_enabled() {
                status.set_connection(ConnectionState::Disabled);
                status.wait_until_enabled().await;
                status.set_connection(ConnectionState::Connected);
            }

            match select3(
                radio.receive(&mut rx_buf[..]),
                seam.next_outbound(),
                status.wait_until_disabled(),
            )
            .await
            {
                Either3::First(len) => {
                    if len > 0 {
                        status.count_frame_in();
                        let now = InstantMillis(started.elapsed().as_millis());
                        status.add_rx(len as u64);
                        throughput.record_rx(now, len as u64);
                        status.set_transfer_rates(throughput.rates());
                        seam.next_inbound(&rx_buf[..len]).await;
                        status.count_frame_delivered();
                    }
                }
                Either3::Second(outbound) => {
                    let len = outbound.len().min(ESP_NOW_V2_AIR_MTU);
                    let disposition = if radio.broadcast(&outbound[..len]).await {
                        let now = InstantMillis(started.elapsed().as_millis());
                        status.add_tx(len as u64);
                        throughput.record_tx(now, len as u64);
                        status.set_transfer_rates(throughput.rates());
                        OutboundDisposition::Sent
                    } else {
                        OutboundDisposition::Dropped(OutboundDropReason::TransportFailure)
                    };
                    seam.complete_outbound(disposition);
                }
                Either3::Third(()) => {}
            }
        }
    }
}
