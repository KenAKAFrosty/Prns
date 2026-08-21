use super::super::*;

/// Adapts esp-radio's `EspNow` handle to the engine's [`EspNowRadio`] seam — the unsafe-free board
/// side of the boundary, the way the SX1262 driver sits behind `SpiDevice`. Broadcast-only; a
/// transient `NO_MEM` while the radio is off serving a BLE connection event is retried a few times
/// before the frame is dropped for the engine to resend.
pub(in crate::s3) struct EspNowAdapter {
    manager: EspNowManager<'static>,
    sender: EspNowSender<'static>,
    receiver: EspNowReceiver<'static>,
    rate_applied: bool,
}

const ESPNOW_SEND_RETRIES: u8 = 8;
const ESPNOW_SEND_RETRY_DELAY: Duration = Duration::from_millis(5);
pub(in crate::s3) struct EspNowPhySettings {
    pub(in crate::s3) driver_rate: WifiPhyRate,
    pub(in crate::s3) bitrate: BitrateBps,
}
/// The pinned ESP-NOW PHY rate: 802.11g 12 Mbps, QPSK rate-1/2 OFDM. HT/HE *broadcast* RX is
/// hard-pinned to 1 Mbps DSSS by the closed Wi-Fi blob (no public override) so MCS rates transmit but
/// never receive; the legacy OFDM-g family is the broadcast-compatible way to keep OFDM's good
/// multipath, and 12M is the QPSK-1/2 sweet spot (good range at ~the USB-feed budget).
///
/// Off-by-one shim: esp-radio 0.18's `set_rate` casts the sequential `WifiPhyRate` discriminant
/// straight into the C `wifi_phy_rate_t`, which reserves a gap at value 4 — so every variant past the
/// gap programs the rate one slot below its name (`Rate12m` -> C 24M). The discriminant of `Rate6m`
/// (10) equals C `WIFI_PHY_RATE_12M`, so `Rate6m` is what actually selects g-12M. This one spot
/// localizes the workaround; TODO: patch esp-radio's enum upstream and return `Rate12m`.
pub(in crate::s3) const ESPNOW_PHY: EspNowPhySettings = EspNowPhySettings {
    driver_rate: WifiPhyRate::Rate6m,
    bitrate: BitrateBps::guess(12_000_000),
};

impl EspNowAdapter {
    pub(in crate::s3) fn new(esp_now: EspNow<'static>) -> Self {
        let (manager, sender, receiver) = esp_now.split();
        Self {
            manager,
            sender,
            receiver,
            rate_applied: false,
        }
    }

    /// Pin the PHY rate once, lazily on first transmit — by then the radio is started (set_config runs
    /// before the interface loop in both the associated and off-grid paths), which
    /// `esp_wifi_config_espnow_rate` requires.
    fn ensure_rate(&mut self) {
        if !self.rate_applied {
            let _ = self.manager.set_rate(ESPNOW_PHY.driver_rate);
            self.rate_applied = true;
        }
    }
}

impl espnow_core::EspNowRadio for EspNowAdapter {
    fn set_channel(&mut self, channel: EspNowChannel) {
        let _ = self.manager.set_channel(channel.as_u8());
    }

    async fn broadcast(&mut self, frame: &[u8]) -> bool {
        self.ensure_rate();
        for _ in 0..ESPNOW_SEND_RETRIES {
            if self
                .sender
                .send_async(&BROADCAST_ADDRESS, frame)
                .await
                .is_ok()
            {
                return true;
            }
            Timer::after(ESPNOW_SEND_RETRY_DELAY).await;
        }
        false
    }

    async fn receive(&mut self, buf: &mut [u8]) -> usize {
        let frame = self.receiver.receive_async().await;
        let data = frame.data();
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        len
    }
}

/// A node pinned to a Wi-Fi access point is channel-locked to it (ESP-NOW must follow the station's
/// channel, never retune and break the association); a node with no Wi-Fi configured is free to sit on
/// the default rendezvous channel. The locked/free seam a future scan-and-follow layer extends.
pub(in crate::s3) fn espnow_channel_policy(station_configured: bool) -> ChannelPolicy {
    if station_configured {
        ChannelPolicy::FollowStation
    } else {
        ChannelPolicy::Fixed(EspNowChannel::DEFAULT)
    }
}
