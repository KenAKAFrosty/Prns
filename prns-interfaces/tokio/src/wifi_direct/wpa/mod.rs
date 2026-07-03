pub mod group;
pub mod proxies;

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use self::group::{plan_for, role_from_group, wait_link_local, WpaGroup};
use self::proxies::{
    GroupProperties, P2PDeviceProxy, PeerProxy, SupplicantInterfaceProxy, SupplicantProxy,
    P2P_DEVICE_INTERFACE, SUPPLICANT_SERVICE,
};
use prns_core::interfaces::wifi_direct::core::{
    GoIntent, Initiative, PeerEvidence, DEVICE_NAME_MARKER,
};
use prns_core::interfaces::wifi_direct::seam::{
    Availability, DiscoveryMode, GroupEndReason, WifiDirectBackend, WifiDirectEvent,
};
use prns_core::interfaces::MacAddress;

const BUS_LOST_REASON: &str = "the wpa_supplicant D-Bus connection closed";
const SUPPLICANT_GONE_REASON: &str = "wpa_supplicant left the bus";
const FIND_RETRY: Duration = Duration::from_secs(2);
const FIND_REASSERT_DELAY: Duration = Duration::from_secs(1);
const RESIGHT_PERIOD: Duration = Duration::from_secs(5);
const LISTEN_LEASE_SECS: i32 = 30;
const LISTEN_REASSERT: Duration = Duration::from_secs(25);
const EXTENDED_LISTEN_PERIOD_MS: i32 = 500;
const EXTENDED_LISTEN_INTERVAL_MS: i32 = 1_500;

#[derive(Debug)]
pub enum WpaP2pError {
    SupplicantUnreachable(zbus::Error),
    NoP2pInterface(zbus::Error),
    P2pUnsupported(zbus::Error),
    LocalAddressUnavailable,
    Dbus(zbus::Error),
}

enum PumpEvent {
    Sighting {
        peer: MacAddress,
        path: OwnedObjectPath,
        name: String,
    },
    PeerGone {
        path: OwnedObjectPath,
    },
    Invitation {
        peer: MacAddress,
        path: OwnedObjectPath,
        name: String,
    },
    GroupFormed {
        group: WpaGroup,
        group_iface: OwnedObjectPath,
    },
    GroupFinished,
    FormationFailed,
    FormationProgress,
    FindStopped,
    FindRetry,
    Resight,
    PumpClosed,
}

struct PeerRecord {
    path: OwnedObjectPath,
    initiative: Initiative,
}

pub struct WpaP2pBackend {
    connection: zbus::Connection,
    p2p: P2PDeviceProxy<'static>,
    local: MacAddress,
    local_name: String,
    peers: HashMap<MacAddress, PeerRecord>,
    peers_by_path: HashMap<OwnedObjectPath, MacAddress>,
    forming_with: Option<MacAddress>,
    group_iface: Option<OwnedObjectPath>,
    queued: VecDeque<WifiDirectEvent<WpaGroup>>,
    desired_discovery: bool,
    formation_active: bool,
    bus_lost: bool,
    events: mpsc::UnboundedReceiver<PumpEvent>,
    events_tx: mpsc::UnboundedSender<PumpEvent>,
}

impl WpaP2pBackend {
    pub async fn open(ifname: &str) -> Result<Self, WpaP2pError> {
        let connection = zbus::Connection::system()
            .await
            .map_err(WpaP2pError::SupplicantUnreachable)?;
        let supplicant = SupplicantProxy::new(&connection)
            .await
            .map_err(WpaP2pError::SupplicantUnreachable)?;
        let path = match supplicant.get_interface(ifname).await {
            Ok(path) => path,
            Err(_) => {
                let mut args = HashMap::new();
                args.insert("Ifname", Value::from(ifname));
                supplicant
                    .create_interface(args)
                    .await
                    .map_err(WpaP2pError::NoP2pInterface)?
            }
        };
        let p2p = P2PDeviceProxy::builder(&connection)
            .path(path)
            .map_err(WpaP2pError::NoP2pInterface)?
            .build()
            .await
            .map_err(WpaP2pError::NoP2pInterface)?;
        p2p.p2p_device_config()
            .await
            .map_err(WpaP2pError::P2pUnsupported)?;
        let local = sysfs_mac(ifname).ok_or(WpaP2pError::LocalAddressUnavailable)?;
        let mut config = HashMap::new();
        let name = marker_device_name(local);
        config.insert("DeviceName", Value::from(name.as_str()));
        p2p.set_p2p_device_config(config)
            .await
            .map_err(WpaP2pError::Dbus)?;
        let mut listen = HashMap::new();
        listen.insert("period", Value::from(EXTENDED_LISTEN_PERIOD_MS));
        listen.insert("interval", Value::from(EXTENDED_LISTEN_INTERVAL_MS));
        match p2p.extended_listen(listen).await {
            Ok(()) => log::info!("wifi-direct extended listen armed on {ifname}"),
            Err(err) => log::warn!("wifi-direct extended listen unavailable on {ifname}: {err}"),
        }
        let (events_tx, events) = mpsc::unbounded_channel();
        spawn_pump(connection.clone(), events_tx.clone());
        let resight = events_tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(RESIGHT_PERIOD);
            loop {
                ticker.tick().await;
                if resight.send(PumpEvent::Resight).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            connection,
            p2p,
            local,
            local_name: name,
            peers: HashMap::new(),
            peers_by_path: HashMap::new(),
            forming_with: None,
            group_iface: None,
            queued: VecDeque::new(),
            desired_discovery: false,
            formation_active: false,
            bus_lost: false,
            events,
            events_tx,
        })
    }

    fn record_peer(&mut self, peer: MacAddress, path: OwnedObjectPath, name: &str) -> Initiative {
        let initiative = if self.local_name.as_str() < name {
            Initiative::Ours
        } else {
            Initiative::Theirs
        };
        self.peers_by_path.insert(path.clone(), peer);
        self.peers.insert(peer, PeerRecord { path, initiative });
        initiative
    }

    fn park_if_supplicant_gone(&mut self, err: &zbus::Error) -> bool {
        if !service_gone(err) {
            return false;
        }
        if !self.bus_lost {
            self.bus_lost = true;
            self.queued.push_back(WifiDirectEvent::AvailabilityChanged(
                Availability::Unavailable(SUPPLICANT_GONE_REASON),
            ));
        }
        true
    }

    fn schedule_find_retry(&self, delay: Duration) {
        let retry = self.events_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = retry.send(PumpEvent::FindRetry);
        });
    }

    fn responder_stance(&self) -> bool {
        !self.peers.is_empty()
            && self
                .peers
                .values()
                .all(|record| matches!(record.initiative, Initiative::Theirs))
    }

    async fn try_find(&mut self) -> Result<(), zbus::Error> {
        if self.bus_lost {
            return Ok(());
        }
        if self.formation_active {
            log::debug!("wifi-direct find deferred while a formation is in flight");
            return Ok(());
        }
        if self.responder_stance() {
            let _ = self.p2p.stop_find().await;
            return match self.p2p.listen(LISTEN_LEASE_SECS).await {
                Ok(()) => {
                    log::info!(
                        "wifi-direct listening as the responder for {:?}",
                        self.local
                    );
                    self.schedule_find_retry(LISTEN_REASSERT);
                    Ok(())
                }
                Err(err) => {
                    log::warn!("wifi-direct listen for {:?} failed: {err}", self.local);
                    if !self.park_if_supplicant_gone(&err) {
                        self.schedule_find_retry(FIND_RETRY);
                    }
                    Err(err)
                }
            };
        }
        match self.p2p.find(HashMap::new()).await {
            Ok(()) => {
                log::info!("wifi-direct find running for {:?}", self.local);
                Ok(())
            }
            Err(err) => {
                log::warn!("wifi-direct find for {:?} failed: {err}", self.local);
                if !self.park_if_supplicant_gone(&err) {
                    self.schedule_find_retry(FIND_RETRY);
                }
                Err(err)
            }
        }
    }

    async fn connect_toward(&mut self, peer: MacAddress, go_intent: Option<i32>) {
        if self.forming_with == Some(peer) {
            log::info!("wifi-direct already negotiating with {peer:?}; letting it ride");
            return;
        }
        let Some(path) = self.peers.get(&peer).map(|record| record.path.clone()) else {
            self.queued
                .push_back(WifiDirectEvent::FormationFailed { peer });
            return;
        };
        let mut args = HashMap::new();
        args.insert("peer", Value::from(path.into_inner()));
        args.insert("wps_method", Value::from("pbc"));
        if let Some(intent) = go_intent {
            args.insert("go_intent", Value::from(intent));
        }
        match self.p2p.connect(args).await {
            Ok(_generated_pin) => {
                log::info!("wifi-direct GO negotiation started toward {peer:?}");
                self.forming_with = Some(peer);
                self.formation_active = true;
            }
            Err(err) => {
                log::warn!("wifi-direct connect toward {peer:?} failed: {err}");
                self.park_if_supplicant_gone(&err);
                self.queued
                    .push_back(WifiDirectEvent::FormationFailed { peer });
            }
        }
    }
}

impl WifiDirectBackend for WpaP2pBackend {
    type Error = WpaP2pError;
    type Group = WpaGroup;

    fn local_address(&self) -> MacAddress {
        self.local
    }

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error> {
        match mode {
            DiscoveryMode::On => {
                self.desired_discovery = true;
                self.try_find().await.map_err(WpaP2pError::Dbus)
            }
            DiscoveryMode::Off => {
                self.desired_discovery = false;
                self.p2p.stop_find().await.map_err(WpaP2pError::Dbus)
            }
        }
    }

    async fn form_group(&mut self, peer: MacAddress, intent: GoIntent) {
        self.connect_toward(peer, Some(i32::from(intent.wire())))
            .await;
    }

    async fn accept_invitation(&mut self, peer: MacAddress, intent: GoIntent) {
        self.connect_toward(peer, Some(i32::from(intent.wire())))
            .await;
    }

    async fn remove_group(&mut self) {
        log::info!("wifi-direct removing the group or canceling the formation in flight");
        self.forming_with = None;
        self.formation_active = false;
        if let Some(path) = self.group_iface.take() {
            let group_device = P2PDeviceProxy::builder(&self.connection)
                .path(path)
                .ok()
                .map(|builder| builder.build());
            if let Some(build) = group_device {
                if let Ok(proxy) = build.await {
                    let _ = proxy.disconnect().await;
                }
            }
        }
        let _ = self.p2p.cancel().await;
    }

    async fn next_event(&mut self) -> WifiDirectEvent<WpaGroup> {
        loop {
            if let Some(event) = self.queued.pop_front() {
                return event;
            }
            if self.bus_lost {
                match self.events.recv().await {
                    Some(_) => continue,
                    None => std::future::pending::<()>().await,
                }
            }
            match self.events.recv().await {
                Some(PumpEvent::Sighting { peer, path, name }) => {
                    let initiative = self.record_peer(peer, path, &name);
                    if matches!(initiative, Initiative::Theirs) && self.responder_stance() {
                        self.schedule_find_retry(FIND_REASSERT_DELAY);
                    }
                    return WifiDirectEvent::Sighting {
                        peer,
                        evidence: PeerEvidence::NameMarker,
                        initiative,
                    };
                }
                Some(PumpEvent::PeerGone { path }) => {
                    if let Some(peer) = self.peers_by_path.remove(&path) {
                        self.peers.remove(&peer);
                        return WifiDirectEvent::PeerGone { peer };
                    }
                }
                Some(PumpEvent::Invitation { peer, path, name }) => {
                    self.record_peer(peer, path, &name);
                    return WifiDirectEvent::Invitation { peer };
                }
                Some(PumpEvent::GroupFormed { group, group_iface }) => {
                    self.group_iface = Some(group_iface);
                    self.forming_with = None;
                    self.formation_active = false;
                    return WifiDirectEvent::GroupFormed { group };
                }
                Some(PumpEvent::GroupFinished) => {
                    self.group_iface = None;
                    self.formation_active = false;
                    self.schedule_find_retry(FIND_REASSERT_DELAY);
                    return WifiDirectEvent::GroupLost {
                        reason: GroupEndReason::LinkLost,
                    };
                }
                Some(PumpEvent::FormationFailed) => {
                    self.formation_active = false;
                    self.schedule_find_retry(FIND_REASSERT_DELAY);
                    if let Some(peer) = self.forming_with.take() {
                        return WifiDirectEvent::FormationFailed { peer };
                    }
                    return WifiDirectEvent::GroupLost {
                        reason: GroupEndReason::LinkLost,
                    };
                }
                Some(PumpEvent::FormationProgress) => {
                    self.formation_active = true;
                    return WifiDirectEvent::FormationProgress;
                }
                Some(PumpEvent::FindStopped) => {
                    if self.desired_discovery {
                        self.schedule_find_retry(FIND_REASSERT_DELAY);
                    }
                }
                Some(PumpEvent::FindRetry) => {
                    if self.desired_discovery && !self.formation_active {
                        let _ = self.try_find().await;
                    }
                }
                Some(PumpEvent::Resight) => {
                    for (peer, record) in &self.peers {
                        self.queued.push_back(WifiDirectEvent::Sighting {
                            peer: *peer,
                            evidence: PeerEvidence::NameMarker,
                            initiative: record.initiative,
                        });
                    }
                }
                Some(PumpEvent::PumpClosed) | None => {
                    self.bus_lost = true;
                    return WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(
                        BUS_LOST_REASON,
                    ));
                }
            }
        }
    }
}

fn spawn_pump(connection: zbus::Connection, events: mpsc::UnboundedSender<PumpEvent>) {
    tokio::spawn(async move {
        let Some(mut stream) = p2p_signal_stream(&connection).await else {
            let _ = events.send(PumpEvent::PumpClosed);
            return;
        };
        while let Some(message) = stream.next().await {
            let Ok(message) = message else { continue };
            let header = message.header();
            let Some(member) = header.member() else {
                continue;
            };
            match member.as_str() {
                "DeviceFound" => {
                    let Ok((path,)) = message.body().deserialize::<(OwnedObjectPath,)>() else {
                        continue;
                    };
                    log::info!("wifi-direct DeviceFound at {path}");
                    let Some((peer, name)) = peer_identity(&connection, &path).await else {
                        log::warn!("wifi-direct peer properties unreadable at {path}");
                        continue;
                    };
                    let marked = name.starts_with(DEVICE_NAME_MARKER);
                    log::info!("wifi-direct sighted {name:?} ({peer:?}) marked={marked}");
                    if !marked {
                        continue;
                    }
                    let _ = events.send(PumpEvent::Sighting { peer, path, name });
                }
                "DeviceLost" => {
                    let Ok((path,)) = message.body().deserialize::<(OwnedObjectPath,)>() else {
                        continue;
                    };
                    let _ = events.send(PumpEvent::PeerGone { path });
                }
                "FindStopped" => {
                    let _ = events.send(PumpEvent::FindStopped);
                }
                "GONegotiationRequest" => {
                    let Ok((path, _passwd_id, _go_intent)) =
                        message.body().deserialize::<(OwnedObjectPath, u16, u8)>()
                    else {
                        continue;
                    };
                    let Some((peer, name)) = peer_identity(&connection, &path).await else {
                        continue;
                    };
                    log::info!("wifi-direct invitation from {name:?} ({peer:?})");
                    if !name.starts_with(DEVICE_NAME_MARKER) {
                        continue;
                    }
                    let _ = events.send(PumpEvent::Invitation { peer, path, name });
                }
                "GroupStarted" => {
                    let Ok((properties,)) = message.body().deserialize::<(GroupProperties,)>()
                    else {
                        continue;
                    };
                    match formed_group(&connection, &properties).await {
                        Some((group, group_iface)) => {
                            let _ = events.send(PumpEvent::GroupFormed { group, group_iface });
                        }
                        None => {
                            let _ = events.send(PumpEvent::FormationFailed);
                        }
                    }
                }
                "GroupFinished" => {
                    let _ = events.send(PumpEvent::GroupFinished);
                }
                "GONegotiationSuccess" => {
                    log::info!("wifi-direct GO negotiation succeeded; provisioning underway");
                    let _ = events.send(PumpEvent::FormationProgress);
                }
                "GONegotiationFailure" => {
                    log::warn!("wifi-direct GO negotiation failed");
                    let _ = events.send(PumpEvent::FormationFailed);
                }
                "GroupFormationFailure" => {
                    if let Ok((reason,)) = message.body().deserialize::<(String,)>() {
                        log::warn!("wifi-direct group formation failed: {reason}");
                    }
                    let _ = events.send(PumpEvent::FormationFailed);
                }
                _ => {}
            }
        }
        let _ = events.send(PumpEvent::PumpClosed);
    });
}

async fn p2p_signal_stream(connection: &zbus::Connection) -> Option<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(P2P_DEVICE_INTERFACE)
        .ok()?
        .sender(SUPPLICANT_SERVICE)
        .ok()?
        .build();
    zbus::MessageStream::for_match_rule(rule, connection, None)
        .await
        .ok()
}

async fn peer_identity(
    connection: &zbus::Connection,
    path: &OwnedObjectPath,
) -> Option<(MacAddress, String)> {
    let proxy = PeerProxy::builder(connection)
        .path(path.clone())
        .ok()?
        .build()
        .await
        .ok()?;
    let name = proxy.device_name().await.ok()?;
    let address = proxy.device_address().await.ok()?;
    let octets: [u8; 6] = address.as_slice().try_into().ok()?;
    Some((MacAddress::new(octets), name))
}

async fn formed_group(
    connection: &zbus::Connection,
    properties: &HashMap<String, OwnedValue>,
) -> Option<(WpaGroup, OwnedObjectPath)> {
    let Some(role_value) = properties.get("role") else {
        log::warn!("wifi-direct GroupStarted carried no role");
        return None;
    };
    let Some(role_string) = role_value
        .try_clone()
        .ok()
        .and_then(|value| String::try_from(value).ok())
    else {
        log::warn!("wifi-direct GroupStarted role was not a string");
        return None;
    };
    let Some(role) = role_from_group(&role_string) else {
        log::warn!("wifi-direct GroupStarted role {role_string:?} is unknown");
        return None;
    };
    let Some(iface_value) = properties.get("interface_object") else {
        log::warn!("wifi-direct GroupStarted carried no interface_object");
        return None;
    };
    let Some(group_iface) = iface_value
        .try_clone()
        .ok()
        .and_then(|value| OwnedObjectPath::try_from(value).ok())
    else {
        log::warn!("wifi-direct GroupStarted interface_object was not a path");
        return None;
    };
    let ifname = match SupplicantInterfaceProxy::builder(connection)
        .path(group_iface.clone())
        .ok()?
        .build()
        .await
    {
        Ok(proxy) => match proxy.ifname().await {
            Ok(ifname) => ifname,
            Err(err) => {
                log::warn!("wifi-direct group interface Ifname read failed: {err}");
                return None;
            }
        },
        Err(err) => {
            log::warn!("wifi-direct group interface proxy build failed: {err}");
            return None;
        }
    };
    log::info!("wifi-direct group started as {role_string} on {ifname}");
    let (link_local, scope) = wait_link_local(&ifname).await?;
    log::info!("wifi-direct group segment address {link_local}%{scope} on {ifname}");
    Some((
        WpaGroup::new(role, plan_for(role, link_local, scope)),
        group_iface,
    ))
}

fn service_gone(err: &zbus::Error) -> bool {
    matches!(
        err,
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown"
    )
}

fn marker_device_name(local: MacAddress) -> String {
    let octets = local.octets();
    format!(
        "{DEVICE_NAME_MARKER}-{:02x}{:02x}{:02x}",
        octets[3], octets[4], octets[5]
    )
}

fn sysfs_mac(ifname: &str) -> Option<MacAddress> {
    let raw = std::fs::read_to_string(format!("/sys/class/net/{ifname}/address")).ok()?;
    parse_mac(raw.trim())
}

fn parse_mac(rendered: &str) -> Option<MacAddress> {
    let mut octets = [0u8; 6];
    let mut parts = rendered.split(':');
    for slot in &mut octets {
        *slot = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(MacAddress::new(octets))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sysfs_rendered_mac_parses_to_its_octets() {
        assert_eq!(
            parse_mac("02:00:00:00:01:00"),
            Some(MacAddress::new([0x02, 0, 0, 0, 1, 0]))
        );
        assert_eq!(parse_mac("02:00:00:00:01"), None);
        assert_eq!(parse_mac("02:00:00:00:01:00:33"), None);
        assert_eq!(parse_mac("zz:00:00:00:01:00"), None);
    }

    #[test]
    fn the_marker_device_name_carries_the_marker_and_a_suffix() {
        let name = marker_device_name(MacAddress::new([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]));
        assert_eq!(name, "Prns-ddeeff");
        assert!(name.starts_with(DEVICE_NAME_MARKER));
    }
}
