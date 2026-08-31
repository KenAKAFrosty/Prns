use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, DialOutcome, Origin, RadioMode, ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{BleAddress, LinkCapabilities, Psm};

use super::bridge::{AndroidBleBridge, Event, PEER_CAPACITY};
use super::link::AndroidBleLink;
use super::AndroidBleError;

pub struct AndroidBleBackend {
    bridge: AndroidBleBridge,
}

impl AndroidBleBackend {
    pub const MAX_PEERS: usize = PEER_CAPACITY;

    #[must_use]
    pub fn new(bridge: AndroidBleBridge) -> Self {
        Self { bridge }
    }
}

impl BleBackend<{ AndroidBleBackend::MAX_PEERS }> for AndroidBleBackend {
    type Error = AndroidBleError;
    type Link = AndroidBleLink;

    fn local_group_tag(&self) -> Option<[u8; 4]> {
        let mut out = [0u8; 4];
        if self.bridge.local_group_tag(&mut out) >= 4 {
            Some(out)
        } else {
            None
        }
    }

    fn drop_all_links(&mut self) {
        self.bridge.close_all_peer_links();
    }

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), AndroidBleError> {
        self.bridge.set_radio_mode(mode);
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

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), AndroidBleError> {
        self.bridge.set_advertising(mode);
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), AndroidBleError> {
        self.bridge.set_scanning(mode);
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
                        peer_protocol: pending.peer_protocol,
                        peer_identity: pending.peer_identity,
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

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        if self.bridge.push_dial(*address.octets()) {
            DialOutcome::Started
        } else {
            DialOutcome::Busy
        }
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        if !self.bridge.close_by_address(*address.octets()) {
            crate::diagnostic_log::error!(
                "bluetooth: could not queue Android physical close for {:02x?}",
                address.octets()
            );
        }
    }
}
