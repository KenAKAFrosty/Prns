mod group;
mod proxies;

pub use group::WpaGroup;
pub(crate) use group::{
    client_plan, owner_plan, role_from_group, wait_for_go_address, wait_link_local,
};
pub use proxies::{P2PDeviceProxy, SupplicantProxy, P2P_DEVICE_INTERFACE, SUPPLICANT_SERVICE};

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use self::proxies::{GroupProperties, PeerProxy, SupplicantInterfaceProxy};
use prns_core::interfaces::wifi_direct::{
    Availability, DiscoveryMode, GroupEndReason, WifiDirectBackend, WifiDirectEvent,
};
use prns_core::interfaces::wifi_direct::{
    GoIntent, GroupRole, Initiative, PeerEvidence, DEVICE_NAME_MARKER, SERVICE_TYPE,
};
use prns_core::interfaces::MacAddress;

const BUS_LOST_REASON: &str = "the wpa_supplicant D-Bus connection closed";
const SUPPLICANT_GONE_REASON: &str = "wpa_supplicant left the bus";
const NETDEV_ACCESS_REASON: &str =
    "wpa_supplicant D-Bus access denied; add this user to the 'netdev' group and start a fresh login session";
// _prns._tcp DNS-SD records in Wi-Fi P2P service-discovery wire form (RFC 1035 with the fixed P2P
// compression pointers c0 0c -> _tcp.local and c0 27 -> _prns._tcp.local). Captured on the air from
// Android's WifiP2pDnsSdServiceInfo, not derived. BONJOUR_PTR_QUERY/RESPONSE are the AddService
// query/response; the response is RDATA only because wpa prepends the question echo itself (register
// the full record and Android double-parses it and throws). SD_PTR_QUERY_TLV is the whole outgoing
// query TLV (2-byte length, protocol 01 = bonjour, transaction) for ServiceDiscoveryRequest.
const BONJOUR_PTR_QUERY: &[u8] = &[
    0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];
const BONJOUR_PTR_RESPONSE: &[u8] = &[0x04, 0x50, 0x72, 0x6e, 0x73, 0xc0, 0x27];
const SD_PTR_QUERY_TLV: &[u8] = &[
    0x0d, 0x00, 0x01, 0x01, 0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];
const SERVICE_MARKER: &[u8] = b"_prns";
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
    AccessDenied,
    Dbus(zbus::Error),
}

enum PumpEvent {
    Sighting {
        peer: MacAddress,
        path: OwnedObjectPath,
        name: String,
        evidence: PeerEvidence,
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
    evidence: PeerEvidence,
}

pub enum WpaP2pBackend {
    Live(Box<WpaSession>),
    Blocked(&'static str),
}

impl WpaP2pBackend {
    pub async fn open(ifname: &str) -> Result<Self, WpaP2pError> {
        match WpaSession::connect(ifname).await {
            Ok(session) => Ok(Self::Live(Box::new(session))),
            Err(WpaP2pError::AccessDenied) => {
                let user = std::env::var("USER").unwrap_or_else(|_| String::from("$USER"));
                crate::diagnostic_log::error!(
                    "wifi-direct: NOT starting on {ifname} — wpa_supplicant's D-Bus interface \
                     (fi.w1.wpa_supplicant1) refused access. It is restricted to root and the \
                     'netdev' group, and this will not resolve on its own. Add this user to netdev \
                     and start a FRESH login session — an existing session keeps its old group \
                     list, so a new terminal is not enough. Run `sudo usermod -aG netdev {user}`, \
                     then log fully out and back in. NetworkManager can keep managing your Wi-Fi \
                     the whole time; this only needs permission to speak to the supplicant it \
                     already runs."
                );
                Ok(Self::Blocked(NETDEV_ACCESS_REASON))
            }
            Err(other) => Err(other),
        }
    }
}

pub struct WpaSession {
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

impl WpaSession {
    async fn connect(ifname: &str) -> Result<Self, WpaP2pError> {
        let connection = zbus::Connection::system()
            .await
            .map_err(WpaP2pError::SupplicantUnreachable)?;
        let supplicant = SupplicantProxy::new(&connection)
            .await
            .map_err(WpaP2pError::SupplicantUnreachable)?;
        let path = match supplicant.get_interface(ifname).await {
            Ok(path) => path,
            Err(err) if access_denied(&err) => return Err(WpaP2pError::AccessDenied),
            Err(_) => {
                let mut args = HashMap::new();
                args.insert("Ifname", Value::from(ifname));
                match supplicant.create_interface(args).await {
                    Ok(path) => path,
                    Err(err) if access_denied(&err) => return Err(WpaP2pError::AccessDenied),
                    Err(err) => return Err(WpaP2pError::NoP2pInterface(err)),
                }
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
            Ok(()) => {
                crate::diagnostic_log::debug!("wifi-direct extended listen armed on {ifname}")
            }
            Err(err) => crate::diagnostic_log::warn!(
                "wifi-direct extended listen unavailable on {ifname}: {err}"
            ),
        }
        let mut service = HashMap::new();
        service.insert("service_type", Value::from("bonjour"));
        service.insert("query", Value::from(BONJOUR_PTR_QUERY.to_vec()));
        service.insert("response", Value::from(BONJOUR_PTR_RESPONSE.to_vec()));
        match p2p.add_service(service).await {
            Ok(()) => {
                crate::diagnostic_log::debug!("wifi-direct advertising {SERVICE_TYPE} on {ifname}")
            }
            Err(err) => crate::diagnostic_log::warn!(
                "wifi-direct AddService for {SERVICE_TYPE} failed: {err}"
            ),
        }
        let _ = p2p.service_discovery_external(0).await;
        let mut query = HashMap::new();
        query.insert("tlv", Value::from(SD_PTR_QUERY_TLV.to_vec()));
        match p2p.service_discovery_request(query).await {
            Ok(reference) => {
                crate::diagnostic_log::debug!(
                    "wifi-direct service discovery for {SERVICE_TYPE} registered ref={reference}"
                );
            }
            Err(err) => {
                crate::diagnostic_log::warn!("wifi-direct service discovery request failed: {err}")
            }
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

    fn record_peer(
        &mut self,
        peer: MacAddress,
        path: OwnedObjectPath,
        name: &str,
        evidence: PeerEvidence,
    ) -> Initiative {
        let initiative = if self.local_name.as_str() < name {
            Initiative::Ours
        } else {
            Initiative::Theirs
        };
        self.peers_by_path.insert(path.clone(), peer);
        self.peers.insert(
            peer,
            PeerRecord {
                path,
                initiative,
                evidence,
            },
        );
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
            crate::diagnostic_log::debug!(
                "wifi-direct find deferred while a formation is in flight"
            );
            return Ok(());
        }
        if self.responder_stance() {
            let _ = self.p2p.stop_find().await;
            return match self.p2p.listen(LISTEN_LEASE_SECS).await {
                Ok(()) => {
                    crate::diagnostic_log::debug!(
                        "wifi-direct listening as the responder for {:?}",
                        self.local
                    );
                    self.schedule_find_retry(LISTEN_REASSERT);
                    Ok(())
                }
                Err(err) => {
                    crate::diagnostic_log::warn!(
                        "wifi-direct listen for {:?} failed: {err}",
                        self.local
                    );
                    if !self.park_if_supplicant_gone(&err) {
                        self.schedule_find_retry(FIND_RETRY);
                    }
                    Err(err)
                }
            };
        }
        match self.p2p.find(HashMap::new()).await {
            Ok(()) => {
                crate::diagnostic_log::debug!("wifi-direct find running for {:?}", self.local);
                Ok(())
            }
            Err(err) => {
                crate::diagnostic_log::warn!("wifi-direct find for {:?} failed: {err}", self.local);
                if !self.park_if_supplicant_gone(&err) {
                    self.schedule_find_retry(FIND_RETRY);
                }
                Err(err)
            }
        }
    }

    async fn connect_toward(&mut self, peer: MacAddress, go_intent: Option<i32>) {
        if self.forming_with == Some(peer) {
            crate::diagnostic_log::debug!(
                "wifi-direct already negotiating with {peer:?}; letting it ride"
            );
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
                crate::diagnostic_log::debug!("wifi-direct GO negotiation started toward {peer:?}");
                self.forming_with = Some(peer);
                self.formation_active = true;
            }
            Err(err) => {
                crate::diagnostic_log::warn!("wifi-direct connect toward {peer:?} failed: {err}");
                self.park_if_supplicant_gone(&err);
                self.queued
                    .push_back(WifiDirectEvent::FormationFailed { peer });
            }
        }
    }
}

impl WpaSession {
    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), WpaP2pError> {
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
        crate::diagnostic_log::debug!(
            "wifi-direct removing the group or canceling the formation in flight"
        );
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
                Some(PumpEvent::Sighting {
                    peer,
                    path,
                    name,
                    evidence,
                }) => {
                    let initiative = self.record_peer(peer, path, &name, evidence);
                    if matches!(initiative, Initiative::Theirs) && self.responder_stance() {
                        self.schedule_find_retry(FIND_REASSERT_DELAY);
                    }
                    return WifiDirectEvent::Sighting {
                        peer,
                        evidence,
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
                    self.record_peer(peer, path, &name, PeerEvidence::ServiceRecord);
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
                            evidence: record.evidence,
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

impl WifiDirectBackend for WpaP2pBackend {
    type Error = WpaP2pError;
    type Group = WpaGroup;

    fn blocked(&self) -> Option<&'static str> {
        match self {
            Self::Live(_) => None,
            Self::Blocked(reason) => Some(reason),
        }
    }

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error> {
        match self {
            Self::Live(session) => session.set_discovery(mode).await,
            Self::Blocked(_) => Ok(()),
        }
    }

    async fn form_group(&mut self, peer: MacAddress, intent: GoIntent) {
        if let Self::Live(session) = self {
            session.form_group(peer, intent).await;
        }
    }

    async fn accept_invitation(&mut self, peer: MacAddress, intent: GoIntent) {
        if let Self::Live(session) = self {
            session.accept_invitation(peer, intent).await;
        }
    }

    async fn remove_group(&mut self) {
        if let Self::Live(session) = self {
            session.remove_group().await;
        }
    }

    async fn next_event(&mut self) -> WifiDirectEvent<WpaGroup> {
        match self {
            Self::Live(session) => session.next_event().await,
            Self::Blocked(_) => std::future::pending().await,
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
                    crate::diagnostic_log::debug!("wifi-direct DeviceFound at {path}");
                    let Some((peer, name)) = peer_identity(&connection, &path).await else {
                        crate::diagnostic_log::warn!(
                            "wifi-direct peer properties unreadable at {path}"
                        );
                        continue;
                    };
                    let marked = name.starts_with(DEVICE_NAME_MARKER);
                    crate::diagnostic_log::debug!(
                        "wifi-direct sighted {name:?} ({peer:?}) marked={marked}"
                    );
                    if !marked {
                        continue;
                    }
                    let _ = events.send(PumpEvent::Sighting {
                        peer,
                        path,
                        name,
                        evidence: PeerEvidence::NameMarker,
                    });
                }
                "DeviceLost" => {
                    let Ok((path,)) = message.body().deserialize::<(OwnedObjectPath,)>() else {
                        continue;
                    };
                    let _ = events.send(PumpEvent::PeerGone { path });
                }
                "ServiceDiscoveryResponse" => {
                    let Ok((response,)) = message
                        .body()
                        .deserialize::<(HashMap<String, OwnedValue>,)>()
                    else {
                        continue;
                    };
                    let carries_service = response
                        .get("tlvs")
                        .and_then(|value| value.try_clone().ok())
                        .and_then(|value| Vec::<u8>::try_from(value).ok())
                        .is_some_and(|tlvs| {
                            tlvs.windows(SERVICE_MARKER.len())
                                .any(|window| window == SERVICE_MARKER)
                        });
                    if !carries_service {
                        continue;
                    }
                    let Some(peer_path) = response
                        .get("peer_object")
                        .and_then(|value| value.try_clone().ok())
                        .and_then(|value| OwnedObjectPath::try_from(value).ok())
                    else {
                        continue;
                    };
                    let Some((peer, name)) = peer_identity(&connection, &peer_path).await else {
                        continue;
                    };
                    crate::diagnostic_log::debug!(
                        "wifi-direct service-recognized {name:?} ({peer:?}) via {SERVICE_TYPE}"
                    );
                    let _ = events.send(PumpEvent::Sighting {
                        peer,
                        path: peer_path,
                        name,
                        evidence: PeerEvidence::ServiceRecord,
                    });
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
                    crate::diagnostic_log::debug!(
                        "wifi-direct invitation from {name:?} ({peer:?})"
                    );
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
                    crate::diagnostic_log::debug!(
                        "wifi-direct GO negotiation succeeded; provisioning underway"
                    );
                    let _ = events.send(PumpEvent::FormationProgress);
                }
                "GONegotiationFailure" => {
                    crate::diagnostic_log::warn!("wifi-direct GO negotiation failed");
                    let _ = events.send(PumpEvent::FormationFailed);
                }
                "GroupFormationFailure" => {
                    if let Ok((reason,)) = message.body().deserialize::<(String,)>() {
                        crate::diagnostic_log::warn!(
                            "wifi-direct group formation failed: {reason}"
                        );
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
        crate::diagnostic_log::warn!("wifi-direct GroupStarted carried no role");
        return None;
    };
    let Some(role_string) = role_value
        .try_clone()
        .ok()
        .and_then(|value| String::try_from(value).ok())
    else {
        crate::diagnostic_log::warn!("wifi-direct GroupStarted role was not a string");
        return None;
    };
    let Some(role) = role_from_group(&role_string) else {
        crate::diagnostic_log::warn!("wifi-direct GroupStarted role {role_string:?} is unknown");
        return None;
    };
    let Some(iface_value) = properties.get("interface_object") else {
        crate::diagnostic_log::warn!("wifi-direct GroupStarted carried no interface_object");
        return None;
    };
    let Some(group_iface) = iface_value
        .try_clone()
        .ok()
        .and_then(|value| OwnedObjectPath::try_from(value).ok())
    else {
        crate::diagnostic_log::warn!("wifi-direct GroupStarted interface_object was not a path");
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
                crate::diagnostic_log::warn!(
                    "wifi-direct group interface Ifname read failed: {err}"
                );
                return None;
            }
        },
        Err(err) => {
            crate::diagnostic_log::warn!("wifi-direct group interface proxy build failed: {err}");
            return None;
        }
    };
    crate::diagnostic_log::debug!("wifi-direct group started as {role_string} on {ifname}");
    if role == GroupRole::Owner {
        wait_for_go_address(&ifname).await;
        return Some((WpaGroup::new(GroupRole::Owner, owner_plan()), group_iface));
    }
    let (link_local, scope) = wait_link_local(&ifname).await?;
    crate::diagnostic_log::debug!(
        "wifi-direct group segment address {link_local}%{scope} on {ifname}"
    );
    Some((
        WpaGroup::new(GroupRole::Client, client_plan(link_local, scope)),
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

fn access_denied(err: &zbus::Error) -> bool {
    matches!(
        err,
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied"
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
