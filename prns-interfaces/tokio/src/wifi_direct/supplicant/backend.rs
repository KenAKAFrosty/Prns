use std::collections::HashSet;
use std::path::Path;

use super::super::wpa::group::{plan_for, wait_link_local, WpaGroup};
use super::ctrl::{WpaCommand, WpaCtrlError, WpaMonitor};
use super::parse;

use prns_core::interfaces::wifi_direct::core::{
    host_role, GoIntent, GroupRole, HostRole, Initiative, PeerEvidence, Platform,
    DEVICE_NAME_MARKER, GROUP_PASSPHRASE, GROUP_SSID_PREFIX,
};
use prns_core::interfaces::wifi_direct::seam::{
    Availability, DiscoveryMode, GroupEndReason, WifiDirectBackend, WifiDirectEvent,
};
use prns_core::interfaces::MacAddress;

const CTRL_LOST_REASON: &str = "the wpa_supplicant control connection closed";

pub struct SupplicantBackend {
    command: WpaCommand,
    monitor: WpaMonitor,
    local_address: Option<MacAddress>,
    peers: HashSet<MacAddress>,
    group_iface: Option<String>,
}

impl SupplicantBackend {
    pub async fn attach(ctrl_dir: impl AsRef<Path>, interface: &str) -> Result<Self, WpaCtrlError> {
        let socket = ctrl_dir.as_ref().join(interface);
        let command = WpaCommand::open(&socket)?;
        let monitor = WpaMonitor::open(&socket).await?;
        let local_address = read_local_address(&command).await;
        let _ = command.request(&parse::advertise_service_command()).await?;
        let _ = command.request("P2P_SERV_DISC_EXTERNAL 0").await?;
        let _ = command.request(&parse::discover_service_command()).await?;
        log::info!("wifi-direct supplicant attached on {interface} ({local_address:?})");
        Ok(Self {
            command,
            monitor,
            local_address,
            peers: HashSet::new(),
            group_iface: None,
        })
    }

    async fn peer_platform(&self, peer: MacAddress) -> Platform {
        let Ok(info) = self
            .command
            .request(&format!("P2P_PEER {}", render_mac(peer)))
            .await
        else {
            return Platform::Native;
        };
        let is_supplicant = info
            .lines()
            .find_map(|line| line.strip_prefix("device_name="))
            .is_some_and(|name| name.starts_with(DEVICE_NAME_MARKER));
        if is_supplicant {
            Platform::Supplicant
        } else {
            Platform::Native
        }
    }

    async fn resolve_initiative(&self, peer: MacAddress) -> Initiative {
        match host_role(Platform::Supplicant, self.peer_platform(peer).await) {
            HostRole::WeHost => Initiative::Ours,
            HostRole::PeerHosts => Initiative::Theirs,
            HostRole::Tiebreak => match self.local_address {
                Some(local) if local.octets() < peer.octets() => Initiative::Ours,
                _ => Initiative::Theirs,
            },
        }
    }

    fn host_ssid(&self) -> String {
        match self.local_address {
            Some(address) => {
                let octets = address.octets();
                format!(
                    "{GROUP_SSID_PREFIX}{:02x}{:02x}{:02x}",
                    octets[3], octets[4], octets[5]
                )
            }
            None => format!("{GROUP_SSID_PREFIX}node"),
        }
    }

    async fn formed_group(&mut self, payload: &str) -> Option<WpaGroup> {
        let started = parse::parse_group_started(payload)?;
        self.group_iface = Some(started.interface.clone());
        let role = if started.is_owner {
            GroupRole::Owner
        } else {
            GroupRole::Client
        };
        if role == GroupRole::Owner {
            let _ = self
                .command
                .request(&parse::advertise_offer_command(&started.ssid))
                .await;
            log::info!(
                "wifi-direct hosting {} on {}",
                started.ssid,
                started.interface
            );
        }
        let (link_local, scope) = wait_link_local(&started.interface).await?;
        Some(WpaGroup::new(role, plan_for(role, link_local, scope)))
    }
}

impl WifiDirectBackend for SupplicantBackend {
    type Error = WpaCtrlError;
    type Group = WpaGroup;

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error> {
        let command = match mode {
            DiscoveryMode::On => "P2P_FIND",
            DiscoveryMode::Off => "P2P_STOP_FIND",
        };
        self.command.request(command).await.map(|_| ())
    }

    async fn form_group(&mut self, _peer: MacAddress, _intent: GoIntent) {
        let ssid = self.host_ssid();
        let network = match self.command.request("ADD_NETWORK").await {
            Ok(id) if id != "FAIL" => id,
            _ => return,
        };
        for setting in [
            format!("ssid \"{ssid}\""),
            format!("psk \"{GROUP_PASSPHRASE}\""),
            String::from("mode 3"),
            String::from("disabled 2"),
        ] {
            let _ = self
                .command
                .request(&format!("SET_NETWORK {network} {setting}"))
                .await;
        }
        let _ = self
            .command
            .request(&format!("P2P_GROUP_ADD persistent={network}"))
            .await;
    }

    async fn accept_invitation(&mut self, peer: MacAddress, intent: GoIntent) {
        self.form_group(peer, intent).await;
    }

    async fn join_group(&mut self, peer: MacAddress) {
        let _ = self
            .command
            .request(&format!("P2P_CONNECT {} pbc join", render_mac(peer)))
            .await;
    }

    async fn remove_group(&mut self) {
        if let Some(interface) = self.group_iface.take() {
            let _ = self
                .command
                .request(&format!("P2P_GROUP_REMOVE {interface}"))
                .await;
        }
    }

    async fn next_event(&mut self) -> WifiDirectEvent<WpaGroup> {
        loop {
            let event = match self.monitor.next_event().await {
                Ok(event) => event,
                Err(_) => {
                    return WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(
                        CTRL_LOST_REASON,
                    ));
                }
            };
            match event.name.as_str() {
                "P2P-SERV-DISC-RESP" => {
                    if !parse::service_response_is_prns(&event.payload) {
                        continue;
                    }
                    let Some(peer) = parse::parse_peer_address(&event.payload) else {
                        continue;
                    };
                    let tlvs = event.payload.split_whitespace().last().unwrap_or_default();
                    match parse::parse_offer_ssid(tlvs) {
                        Some(ssid) if ssid.starts_with(GROUP_SSID_PREFIX) => {
                            return WifiDirectEvent::GroupOffer { peer };
                        }
                        _ => {
                            self.peers.insert(peer);
                            let initiative = self.resolve_initiative(peer).await;
                            return WifiDirectEvent::Sighting {
                                peer,
                                evidence: PeerEvidence::ServiceRecord,
                                initiative,
                            };
                        }
                    }
                }
                "P2P-DEVICE-LOST" => {
                    if let Some(peer) = parse::parse_peer_address(&event.payload) {
                        if self.peers.remove(&peer) {
                            return WifiDirectEvent::PeerGone { peer };
                        }
                    }
                }
                "P2P-GO-NEG-REQUEST" => {
                    if let Some(peer) = parse::parse_peer_address(&event.payload) {
                        return WifiDirectEvent::Invitation { peer };
                    }
                }
                "P2P-GROUP-STARTED" => {
                    if let Some(group) = self.formed_group(&event.payload).await {
                        return WifiDirectEvent::GroupFormed { group };
                    }
                    return WifiDirectEvent::GroupLost {
                        reason: GroupEndReason::LinkLost,
                    };
                }
                "P2P-GROUP-REMOVED" => {
                    self.group_iface = None;
                    return WifiDirectEvent::GroupLost {
                        reason: GroupEndReason::LinkLost,
                    };
                }
                _ => {}
            }
        }
    }
}

async fn read_local_address(command: &WpaCommand) -> Option<MacAddress> {
    let status = command.request("STATUS").await.ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("p2p_device_address="))
        .and_then(parse::parse_mac)
}

fn render_mac(address: MacAddress) -> String {
    let octets = address.octets();
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        octets[0], octets[1], octets[2], octets[3], octets[4], octets[5]
    )
}
