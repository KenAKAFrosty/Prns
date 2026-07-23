use std::collections::HashMap;
use std::path::Path;

use super::super::wpa::{
    client_plan, owner_plan, owner_plan_v6, wait_for_go_address, wait_link_local, WpaGroup,
};
use super::ctrl::{WpaCommand, WpaCtrlError, WpaMonitor};
use super::parse;
use super::process::{SupplicantLaunchError, SupplicantProcess};

use prns_core::interfaces::channel_rendezvous::{
    decide, ChannelCommitment, RendezvousOutcome, SocialChannel,
};
use prns_core::interfaces::wifi_direct::{
    host_role, service_instance_platform, GoIntent, GroupRole, HostRole, Initiative, PeerEvidence,
    Platform, DEVICE_NAME_MARKER, GROUP_PASSPHRASE, GROUP_SSID_PREFIX,
};
use prns_core::interfaces::wifi_direct::{
    Availability, DiscoveryMode, GroupEndReason, WifiDirectBackend, WifiDirectEvent,
};
use prns_core::interfaces::MacAddress;

const CTRL_LOST_REASON: &str = "the wpa_supplicant control connection closed";

const STA_CHANNEL_UNAVAILABLE: &str =
    "the Wi-Fi station channel cannot host a co-located Wi-Fi Direct group";

pub struct SupplicantBackend {
    command: WpaCommand,
    monitor: WpaMonitor,
    p2p_monitor: Option<WpaMonitor>,
    local_address: Option<MacAddress>,
    peers: HashMap<MacAddress, Platform>,
    group_iface: Option<String>,
    pending_unavailable: Option<&'static str>,
    _process: Option<SupplicantProcess>,
}

impl SupplicantBackend {
    pub async fn launch(interface: &str) -> Result<Self, SupplicantLaunchError> {
        let (process, ctrl_dir) = SupplicantProcess::spawn(interface).await?;
        let mut backend = Self::attach(&ctrl_dir, interface)
            .await
            .map_err(SupplicantLaunchError::Attach)?;
        backend._process = Some(process);
        Ok(backend)
    }

    pub async fn attach(ctrl_dir: impl AsRef<Path>, interface: &str) -> Result<Self, WpaCtrlError> {
        let dir = ctrl_dir.as_ref();
        let socket = dir.join(interface);
        let command = WpaCommand::open(&socket)?;
        let monitor = WpaMonitor::open(&socket).await?;
        let p2p_socket = dir.join(format!("p2p-dev-{interface}"));
        let p2p_monitor = WpaMonitor::open(&p2p_socket).await.ok();
        if p2p_monitor.is_none() {
            crate::diagnostic_log::warn!(
                "wifi-direct: no monitor on {p2p_socket:?}; P2P discovery events may be missed"
            );
        }
        let local_address = read_local_address(&command).await;
        if let Some(address) = local_address {
            let _ = command
                .request(&format!("SET device_name {}", marker_device_name(address)))
                .await;
        }
        let _ = command.request(&parse::advertise_service_command()).await?;
        let _ = command.request("P2P_SERV_DISC_EXTERNAL 0").await?;
        let _ = command.request(&parse::discover_service_command()).await?;
        crate::diagnostic_log::debug!(
            "wifi-direct supplicant attached on {interface} ({local_address:?})"
        );
        Ok(Self {
            command,
            monitor,
            p2p_monitor,
            local_address,
            peers: HashMap::new(),
            group_iface: None,
            pending_unavailable: None,
            _process: None,
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

    fn resolve_initiative(&self, peer: MacAddress, platform: Platform) -> Initiative {
        match host_role(Platform::Supplicant, platform) {
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

    async fn sta_commitment(&self) -> ChannelCommitment {
        match self.command.request("STATUS").await {
            Ok(status) => parse::parse_status_commitment(&status),
            Err(_) => ChannelCommitment::Free,
        }
    }

    async fn host_autonomous_group(&self, freq: u16) {
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
            .request(&format!("P2P_GROUP_ADD persistent={network} freq={freq}"))
            .await;
    }

    async fn formed_group(&mut self, payload: &str) -> Option<WpaGroup> {
        let started = parse::parse_group_started(payload)?;
        self.group_iface = Some(started.interface.clone());
        if started.is_owner {
            let _ = self
                .command
                .request(&parse::advertise_offer_command(&started.ssid))
                .await;
            crate::diagnostic_log::debug!(
                "wifi-direct hosting {} on {}",
                started.ssid,
                started.interface
            );
            let plan = if wait_for_go_address(&started.interface).await {
                owner_plan()
            } else {
                let (link_local, scope) = wait_link_local(&started.interface).await?;
                owner_plan_v6(link_local, scope)
            };
            return Some(WpaGroup::new(GroupRole::Owner, plan));
        }
        let (link_local, scope) = wait_link_local(&started.interface).await?;
        Some(WpaGroup::new(
            GroupRole::Client,
            client_plan(link_local, scope),
        ))
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

    async fn form_group(&mut self, peer: MacAddress, intent: GoIntent) {
        let outcome = decide(
            self.sta_commitment().await,
            Some(ChannelCommitment::Free),
            SocialChannel::DEFAULT,
        );
        let Some(freq) = group_freq(outcome) else {
            self.pending_unavailable = Some(STA_CHANNEL_UNAVAILABLE);
            return;
        };
        let platform = match self.peers.get(&peer) {
            Some(platform) => *platform,
            None => self.peer_platform(peer).await,
        };
        match platform {
            Platform::Supplicant => {
                let _ = self
                    .command
                    .request(&go_neg_command(peer, intent, freq))
                    .await;
            }
            Platform::Native => self.host_autonomous_group(freq).await,
        }
    }

    async fn accept_invitation(&mut self, peer: MacAddress, intent: GoIntent) {
        let outcome = decide(
            self.sta_commitment().await,
            Some(ChannelCommitment::Free),
            SocialChannel::DEFAULT,
        );
        let Some(freq) = group_freq(outcome) else {
            self.pending_unavailable = Some(STA_CHANNEL_UNAVAILABLE);
            return;
        };
        let _ = self
            .command
            .request(&authorize_command(peer, intent, freq))
            .await;
    }

    async fn join_group(&mut self, peer: MacAddress) {
        let _ = self
            .command
            .request(&format!("P2P_CONNECT {} pbc join", render_mac(peer)))
            .await;
    }

    async fn remove_group(&mut self) {
        if self.group_iface.take().is_some() {
            let _ = self.command.request("P2P_GROUP_REMOVE *").await;
        }
    }

    async fn next_event(&mut self) -> WifiDirectEvent<WpaGroup> {
        if let Some(reason) = self.pending_unavailable.take() {
            return WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(reason));
        }
        loop {
            let received = if let Some(p2p) = self.p2p_monitor.as_ref() {
                tokio::select! {
                    base = self.monitor.next_event() => base,
                    discovery = p2p.next_event() => discovery,
                }
            } else {
                self.monitor.next_event().await
            };
            let event = match received {
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
                    let Some(instance) = parse::service_instance(&event.payload) else {
                        continue;
                    };
                    match instance {
                        ssid if ssid.starts_with(GROUP_SSID_PREFIX) => {
                            return WifiDirectEvent::GroupOffer { peer };
                        }
                        instance => {
                            let platform = match service_instance_platform(&instance) {
                                Some(platform) => platform,
                                None => self.peer_platform(peer).await,
                            };
                            self.peers.insert(peer, platform);
                            let initiative = self.resolve_initiative(peer, platform);
                            return WifiDirectEvent::Sighting {
                                peer,
                                evidence: PeerEvidence::ServiceRecord,
                                initiative,
                            };
                        }
                    }
                }
                "P2P-DEVICE-FOUND" => {
                    let Some(peer) = parse::parse_peer_address(&event.payload) else {
                        continue;
                    };
                    let is_marker = parse::field(&event.payload, "name")
                        .is_some_and(|name| name.starts_with(DEVICE_NAME_MARKER));
                    if is_marker && self.peers.insert(peer, Platform::Supplicant).is_none() {
                        let initiative = self.resolve_initiative(peer, Platform::Supplicant);
                        return WifiDirectEvent::Sighting {
                            peer,
                            evidence: PeerEvidence::NameMarker,
                            initiative,
                        };
                    }
                }
                "P2P-DEVICE-LOST" => {
                    if let Some(peer) = parse::parse_peer_address(&event.payload) {
                        if self.peers.remove(&peer).is_some() {
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

fn go_neg_command(peer: MacAddress, intent: GoIntent, freq: u16) -> String {
    format!(
        "P2P_CONNECT {} pbc go_intent={} freq={freq}",
        render_mac(peer),
        intent.wire()
    )
}

fn authorize_command(peer: MacAddress, intent: GoIntent, freq: u16) -> String {
    format!(
        "P2P_CONNECT {} pbc auth go_intent={} freq={freq}",
        render_mac(peer),
        intent.wire()
    )
}

fn group_freq(outcome: RendezvousOutcome) -> Option<u16> {
    match outcome {
        RendezvousOutcome::StayOn(channel) | RendezvousOutcome::RetuneTo(channel) => {
            Some(channel.as_mhz())
        }
        RendezvousOutcome::SeekPeer => Some(SocialChannel::DEFAULT.channel().as_mhz()),
        RendezvousOutcome::Incompatible => None,
    }
}

fn render_mac(address: MacAddress) -> String {
    let octets = address.octets();
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        octets[0], octets[1], octets[2], octets[3], octets[4], octets[5]
    )
}

fn marker_device_name(address: MacAddress) -> String {
    let octets = address.octets();
    format!(
        "{DEVICE_NAME_MARKER}-{:02x}{:02x}{:02x}",
        octets[3], octets[4], octets[5]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::channel_rendezvous::WifiChannel;
    use std::sync::{Arc, Mutex};
    use tokio::net::UnixDatagram;

    #[test]
    fn group_freq_maps_a_channel_or_declines() {
        let channel = WifiChannel::new(5180).unwrap();
        assert_eq!(group_freq(RendezvousOutcome::StayOn(channel)), Some(5180));
        assert_eq!(group_freq(RendezvousOutcome::RetuneTo(channel)), Some(5180));
        assert_eq!(group_freq(RendezvousOutcome::Incompatible), None);
        assert_eq!(
            group_freq(RendezvousOutcome::SeekPeer),
            Some(SocialChannel::DEFAULT.channel().as_mhz())
        );
    }

    #[test]
    fn go_negotiation_toward_a_supplicant_peer_carries_the_decided_channel() {
        let peer = MacAddress::new([0x42, 0, 0, 0, 0, 1]);
        assert_eq!(
            go_neg_command(peer, GoIntent::PREFER_OWNER, 5180),
            "P2P_CONNECT 42:00:00:00:00:01 pbc go_intent=13 freq=5180"
        );
    }

    #[test]
    fn an_invitation_is_authorized_without_starting_a_second_negotiation() {
        let peer = MacAddress::new([0x42, 0, 0, 0, 0, 1]);
        assert_eq!(
            authorize_command(peer, GoIntent::PREFER_CLIENT, 2412),
            "P2P_CONNECT 42:00:00:00:00:01 pbc auth go_intent=2 freq=2412"
        );
    }

    async fn fake_supplicant(
        server: UnixDatagram,
        status: &'static str,
        seen: Arc<Mutex<Vec<String>>>,
    ) {
        let mut buffer = [0u8; 4096];
        loop {
            let (read, peer) = server.recv_from(&mut buffer).await.unwrap();
            let command = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let reply: &[u8] = if command.starts_with("P2P_") || command.starts_with("SET") {
                b"OK"
            } else if command == "STATUS" {
                status.as_bytes()
            } else if command == "ADD_NETWORK" {
                b"0"
            } else {
                b"OK"
            };
            seen.lock().unwrap().push(command);
            if let Some(path) = peer.as_pathname() {
                let _ = server.send_to(reply, path).await;
            }
        }
    }

    async fn backend_over_fake(
        label: &str,
        status: &'static str,
    ) -> (
        SupplicantBackend,
        Arc<Mutex<Vec<String>>>,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!("prns-scc-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("wlan0");
        let _ = std::fs::remove_file(&socket);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = UnixDatagram::bind(&socket).unwrap();
        tokio::spawn(fake_supplicant(server, status, seen.clone()));
        let backend = SupplicantBackend::attach(&dir, "wlan0").await.unwrap();
        (backend, seen, dir)
    }

    #[tokio::test]
    async fn a_dfs_station_channel_declines_to_host_and_reports_unavailable() {
        let status =
            "p2p_device_address=02:aa:bb:cc:dd:ee\nwpa_state=COMPLETED\nfreq=5300\nssid=Home\n";
        let (mut backend, seen, dir) = backend_over_fake("dfs", status).await;

        backend
            .form_group(
                MacAddress::new([0x42, 0, 0, 0, 0, 1]),
                GoIntent::PREFER_OWNER,
            )
            .await;
        let reason = match backend.next_event().await {
            WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(reason)) => reason,
            _ => panic!("a DFS station channel must report unavailable-with-reason"),
        };

        assert_eq!(reason, STA_CHANNEL_UNAVAILABLE);
        assert!(
            !seen
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("P2P_GROUP_ADD")),
            "no group is formed on a channel that cannot host one",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_station_channel_forms_the_group_co_channel() {
        let status =
            "p2p_device_address=02:aa:bb:cc:dd:ee\nwpa_state=COMPLETED\nfreq=2412\nssid=Home\n";
        let (mut backend, seen, dir) = backend_over_fake("cochannel", status).await;

        backend
            .form_group(
                MacAddress::new([0x42, 0, 0, 0, 0, 1]),
                GoIntent::PREFER_OWNER,
            )
            .await;

        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("P2P_GROUP_ADD") && c.contains("freq=2412")),
            "the group owner forms on the station channel",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
