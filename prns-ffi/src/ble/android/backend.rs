use prns_core::interfaces::bluetooth_auto::core::{BleAddress, LinkCapabilities, Psm};
use prns_core::interfaces::bluetooth_auto::limits;
use prns_core::interfaces::bluetooth_auto::seam::{BleBackend, BleEvent, Origin};

use super::bridge::{AndroidBleBridge, Event};
use super::link::AndroidBleLink;
use super::AndroidBleError;

pub struct AndroidBleBackend {
    bridge: AndroidBleBridge,
}

impl AndroidBleBackend {
    #[must_use]
    pub fn new(bridge: AndroidBleBridge) -> Self {
        Self { bridge }
    }
}

impl BleBackend for AndroidBleBackend {
    const MAX_PEERS: usize = limits::ANDROID_MAX_PEERS;
    type Error = AndroidBleError;
    type Link = AndroidBleLink;

    async fn set_radio_enabled(&mut self, enabled: bool) -> Result<(), AndroidBleError> {
        self.bridge.set_radio_enabled(enabled);
        Ok(())
    }

    async fn local_capabilities(
        &mut self,
        mut configured: LinkCapabilities,
    ) -> Result<LinkCapabilities, AndroidBleError> {
        let psm = self.bridge.await_psm().await;
        configured.l2cap = Psm::new(psm);
        Ok(configured)
    }

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), AndroidBleError> {
        self.bridge.set_advertising(enabled);
        Ok(())
    }

    async fn set_scanning(&mut self, enabled: bool) -> Result<(), AndroidBleError> {
        self.bridge.set_scanning(enabled);
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<AndroidBleLink> {
        loop {
            let event = self
                .bridge
                .shared
                .events
                .lock()
                .ok()
                .and_then(|mut events| events.pop_front());
            match event {
                Some(Event::Sighting { address, rssi }) => {
                    return BleEvent::Sighting { address, rssi };
                }
                Some(Event::DialFailed { address }) => {
                    return BleEvent::DialFailed { address };
                }
                Some(Event::Link(pending)) => {
                    let dialed = pending.dialed;
                    let peer_rssi = pending.rssi;
                    let link = AndroidBleLink {
                        conn_id: pending.conn_id,
                        address: pending.address,
                        dialect: pending.dialect,
                        control_in: pending.control_in,
                        l2cap_in: Some(pending.l2cap_in),
                        data_in: Some(pending.data_in),
                        control_out: pending.control_out,
                        l2cap_out: pending.l2cap_out,
                        data_out: pending.data_out,
                        l2cap_up: pending.l2cap_up,
                        l2cap_opens: pending.l2cap_opens,
                        work: pending.work,
                    };
                    if dialed {
                        return BleEvent::LinkReady {
                            link,
                            origin: Origin::Dialed,
                            peer_rssi,
                        };
                    }
                    return BleEvent::Inbound(link);
                }
                None => self.bridge.shared.events_ready.notified().await,
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        self.bridge.push_dial(*address.octets());
    }
}
