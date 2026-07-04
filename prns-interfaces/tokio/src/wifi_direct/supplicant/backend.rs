use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::super::wpa::group::{plan_for, wait_link_local, WpaGroup};
use super::ctrl::{WpaCommand, WpaCtrlError, WpaMonitor};
use super::parse;

use prns_core::interfaces::wifi_direct::core::{GoIntent, GroupRole, Initiative, PeerEvidence};
use prns_core::interfaces::wifi_direct::seam::{
    Availability, DiscoveryMode, GroupEndReason, WifiDirectBackend, WifiDirectEvent,
};
use prns_core::interfaces::MacAddress;

const HOST_FREQUENCY: u16 = 2412;
const CTRL_LOST_REASON: &str = "the wpa_supplicant control connection closed";

pub struct SupplicantBackend {
    ctrl_dir: PathBuf,
    command: WpaCommand,
    monitor: WpaMonitor,
    local_address: Option<MacAddress>,
    peers: HashSet<MacAddress>,
    group_iface: Option<String>,
}

impl SupplicantBackend {
    pub async fn attach(ctrl_dir: impl AsRef<Path>, interface: &str) -> Result<Self, WpaCtrlError> {
        let ctrl_dir = ctrl_dir.as_ref().to_owned();
        let socket = ctrl_dir.join(interface);
        let command = WpaCommand::open(&socket)?;
        let monitor = WpaMonitor::open(&socket).await?;
        let local_address = read_local_address(&command).await;
        let _ = command.request(&parse::advertise_service_command()).await?;
        let _ = command.request("P2P_SERV_DISC_EXTERNAL 0").await?;
        let _ = command.request(&parse::discover_service_command()).await?;
        log::info!("wifi-direct supplicant attached on {interface} ({local_address:?})");
        Ok(Self {
            ctrl_dir,
            command,
            monitor,
            local_address,
            peers: HashSet::new(),
            group_iface: None,
        })
    }

    fn initiative_for(&self, peer: MacAddress) -> Initiative {
        match self.local_address {
            Some(local) if local.octets() < peer.octets() => Initiative::Ours,
            Some(_) => Initiative::Theirs,
            None => Initiative::Ours,
        }
    }

    async fn group_passphrase(&self, interface: &str) -> Option<String> {
        let command = WpaCommand::open(&self.ctrl_dir.join(interface)).ok()?;
        match command.request("P2P_GET_PASSPHRASE").await {
            Ok(passphrase) if passphrase != "FAIL" => Some(passphrase),
            _ => None,
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
        if let (GroupRole::Owner, Some(passphrase)) =
            (role, self.group_passphrase(&started.interface).await)
        {
            log::info!(
                "wifi-direct hosting {} (passphrase {passphrase}) on {}",
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
        let _ = self
            .command
            .request(&format!("P2P_GROUP_ADD freq={HOST_FREQUENCY}"))
            .await;
    }

    async fn accept_invitation(&mut self, peer: MacAddress, intent: GoIntent) {
        self.form_group(peer, intent).await;
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
                    self.peers.insert(peer);
                    return WifiDirectEvent::Sighting {
                        peer,
                        evidence: PeerEvidence::ServiceRecord,
                        initiative: self.initiative_for(peer),
                    };
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
