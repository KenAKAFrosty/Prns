mod catalog;
mod http;
mod lan_discovery;
mod network;
mod transport;

use std::io;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::task::JoinSet;

use prns_core::interfaces::browser_rendezvous as contract;
use prns_core::interfaces::browser_rendezvous::BrowserRendezvousId;
use prns_core::interfaces::websocket::{WebSocketFramingSelection, WebSocketWireFraming};
use prns_core::interfaces::{EffectiveInterfacePolicy, InterfaceKind, ReportsStatus};
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

pub use crate::network_device::AutoWifiDevicePolicy;
use crate::websocket::WebSocketServerConnection;

pub use catalog::{
    decode_browser_gateway_catalog, BrowserGatewayCatalogDecodeError, BrowserGatewayEndpoint,
    BrowserGatewayEndpointError,
};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
const ACCEPT_QUEUE_DEPTH: usize = 64;
const MAX_PENDING_HANDSHAKES: usize = 64;

pub struct BrowserRendezvous {
    id: BrowserRendezvousId,
    channel_tag: [u8; contract::ID_LEN],
    policy: EffectiveInterfacePolicy,
    devices: AutoWifiDevicePolicy,
    discovered: Option<UnboundedReceiver<Vec<BrowserGatewayEndpoint>>>,
    status: BrowserRendezvousStatus,
}

impl BrowserRendezvous {
    pub fn new(
        id: BrowserRendezvousId,
        devices: AutoWifiDevicePolicy,
        policy: EffectiveInterfacePolicy,
    ) -> Self {
        Self {
            id,
            channel_tag: *id.as_bytes(),
            policy,
            devices,
            discovered: None,
            status: BrowserRendezvousStatus::new(),
        }
    }

    pub fn with_discovered_gateways(
        mut self,
        discovered: UnboundedReceiver<Vec<BrowserGatewayEndpoint>>,
    ) -> Self {
        self.discovered = Some(discovered);
        self
    }

    pub fn status(&self) -> BrowserRendezvousStatus {
        self.status.clone()
    }
}

impl InterfaceSupervisor for BrowserRendezvous {
    const KIND: InterfaceKind = InterfaceKind::WebSocketServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        let catalog = catalog::Catalog::new(self.id);
        let mut handshakes = JoinSet::new();
        let (accepted_tx, mut accepted_rx) = mpsc::channel(ACCEPT_QUEUE_DEPTH);
        let mut listeners = network::ListenerSet::new();
        let mut loopback_client: Option<AttachedInterface> = None;
        let mut discovered = self.discovered;
        let mut lan_discovery = lan_discovery::LanDiscovery::spawn(self.id, self.devices.clone());
        let mut lan_discovery_open = true;
        let mut reconcile = tokio::time::interval(RECONCILE_INTERVAL);
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = reconcile.tick() => {
                    if listeners.contains(network::loopback(contract::PORT)) {
                        lan_discovery.set_core(true);
                        reconcile_lan_listeners(
                            &mut listeners,
                            &accepted_tx,
                            &self.devices,
                            &self.status,
                        ).await;
                        continue;
                    }
                    match TcpListener::bind(network::loopback(contract::PORT)).await {
                        Ok(listener) => {
                            if let Some(client) = loopback_client.take() {
                                client.teardown();
                            }
                            self.status.clear_client();
                            if listeners.insert(listener, accepted_tx.clone()).is_ok() {
                                self.status.set_role(BrowserRendezvousRole::Core);
                                lan_discovery.set_core(true);
                                reconcile_lan_listeners(
                                    &mut listeners,
                                    &accepted_tx,
                                    &self.devices,
                                    &self.status,
                                ).await;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                            listeners.clear();
                            self.status.set_listener_count(0);
                            lan_discovery.set_core(false);
                            if loopback_client.is_none() {
                                let client = transport::BrowserRendezvousClient::new(self.policy);
                                let client_status = client.status();
                                let core_id = client.core_id();
                                loopback_client = Some(fleet.add(client));
                                self.status.set_client(client_status, core_id);
                            }
                            self.status.set_role(BrowserRendezvousRole::Client);
                        }
                        Err(_) => {
                            listeners.clear();
                            self.status.set_listener_count(0);
                            lan_discovery.set_core(false);
                            self.status.set_role(BrowserRendezvousRole::Contending);
                        }
                    }
                }
                accepted = accepted_rx.recv(), if handshakes.len() < MAX_PENDING_HANDSHAKES => {
                    let Some(accepted) = accepted else {
                        return;
                    };
                    if !network::peer_is_eligible(accepted.local, accepted.peer, &self.devices) {
                        continue;
                    }
                    let catalog = catalog.clone();
                    let id = self.id;
                    handshakes.spawn(async move {
                        tokio::time::timeout(
                            transport::HANDSHAKE_TIMEOUT,
                            prepare_connection(accepted, catalog, id),
                        )
                        .await
                        .ok()
                        .flatten()
                    });
                }
                completed = handshakes.join_next(), if !handshakes.is_empty() => {
                    let Some(Ok(Some(prepared))) = completed else {
                        continue;
                    };
                    let mut channel_tag = self.id.as_bytes().to_vec();
                    channel_tag.extend_from_slice(prepared.peer.to_string().as_bytes());
                    let connection = WebSocketServerConnection::with_policy(
                        channel_tag,
                        prepared.socket,
                        self.policy,
                        WebSocketFramingSelection::Fixed(WebSocketWireFraming::RawPacket),
                    );
                    let _ = fleet.add(connection);
                }
                snapshot = next_discovery(discovered.as_mut()) => {
                    match snapshot {
                        Some(endpoints) => catalog.replace_discovered(
                            catalog::CatalogSource::Injected,
                            endpoints,
                        ),
                        None => discovered = None,
                    }
                }
                snapshot = lan_discovery.next_snapshot(), if lan_discovery_open => {
                    match snapshot {
                        Some(endpoints) => catalog.replace_discovered(
                            catalog::CatalogSource::LanDiscovery,
                            endpoints,
                        ),
                        None => lan_discovery_open = false,
                    }
                }
            }
        }
    }
}

impl ReportsStatus for BrowserRendezvous {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        None
    }
}

async fn reconcile_lan_listeners(
    listeners: &mut network::ListenerSet,
    accepted: &mpsc::Sender<network::AcceptedStream>,
    devices: &AutoWifiDevicePolicy,
    status: &BrowserRendezvousStatus,
) {
    let desired = network::eligible_addresses(devices, contract::PORT).unwrap_or_default();
    listeners.reconcile(&desired);
    for address in desired {
        if listeners.contains(address.socket) {
            continue;
        }
        let Ok(listener) = TcpListener::bind(address.socket).await else {
            continue;
        };
        let _ = listeners.insert(listener, accepted.clone());
    }
    status.set_listener_count(listeners.len());
}

struct PreparedConnection {
    peer: std::net::SocketAddr,
    socket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
}

async fn prepare_connection(
    accepted: network::AcceptedStream,
    catalog: catalog::Catalog,
    id: BrowserRendezvousId,
) -> Option<PreparedConnection> {
    let _ = accepted.stream.set_nodelay(true);
    let Ok(route) = http::route(accepted.stream, accepted.local, &catalog).await else {
        return None;
    };
    let http::RequestEndpoint::Upgrade(stream) = route else {
        return None;
    };
    let Ok(socket) = transport::accept(stream, id).await else {
        return None;
    };
    Some(PreparedConnection {
        peer: accepted.peer,
        socket,
    })
}

async fn next_discovery(
    discovered: Option<&mut UnboundedReceiver<Vec<BrowserGatewayEndpoint>>>,
) -> Option<Vec<BrowserGatewayEndpoint>> {
    match discovered {
        Some(discovered) => discovered.recv().await,
        None => std::future::pending().await,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserRendezvousRole {
    Contending,
    Core,
    Client,
}

#[derive(Clone)]
pub struct BrowserRendezvousStatus {
    shared: Arc<BrowserRendezvousStatusShared>,
}

struct BrowserRendezvousStatusShared {
    role: AtomicU8,
    listeners: AtomicUsize,
    client: Mutex<Option<prns_runtime::manifold::driver::TokioInterfaceStatus>>,
    core_id: Mutex<Option<Arc<Mutex<Option<BrowserRendezvousId>>>>>,
}

impl BrowserRendezvousStatus {
    fn new() -> Self {
        Self {
            shared: Arc::new(BrowserRendezvousStatusShared {
                role: AtomicU8::new(role_byte(BrowserRendezvousRole::Contending)),
                listeners: AtomicUsize::new(0),
                client: Mutex::new(None),
                core_id: Mutex::new(None),
            }),
        }
    }

    pub fn role(&self) -> BrowserRendezvousRole {
        match self.shared.role.load(Ordering::Relaxed) {
            1 => BrowserRendezvousRole::Core,
            2 => BrowserRendezvousRole::Client,
            0 | 3..=u8::MAX => BrowserRendezvousRole::Contending,
        }
    }

    pub fn listener_count(&self) -> usize {
        self.shared.listeners.load(Ordering::Relaxed)
    }

    pub fn core_id(&self) -> Option<BrowserRendezvousId> {
        self.shared
            .core_id
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .and_then(|id| id.lock().ok().and_then(|id| *id))
    }

    fn set_role(&self, role: BrowserRendezvousRole) {
        self.shared.role.store(role_byte(role), Ordering::Relaxed);
    }

    fn set_listener_count(&self, listeners: usize) {
        self.shared.listeners.store(listeners, Ordering::Relaxed);
    }

    fn set_client(
        &self,
        status: prns_runtime::manifold::driver::TokioInterfaceStatus,
        core_id: Arc<Mutex<Option<BrowserRendezvousId>>>,
    ) {
        if let Ok(mut slot) = self.shared.client.lock() {
            *slot = Some(status);
        }
        if let Ok(mut slot) = self.shared.core_id.lock() {
            *slot = Some(core_id);
        }
    }

    fn clear_client(&self) {
        if let Ok(mut slot) = self.shared.client.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.shared.core_id.lock() {
            *slot = None;
        }
    }
}

const fn role_byte(role: BrowserRendezvousRole) -> u8 {
    match role {
        BrowserRendezvousRole::Contending => 0,
        BrowserRendezvousRole::Core => 1,
        BrowserRendezvousRole::Client => 2,
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use prns_core::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use prns_runtime::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    };
    use prns_runtime::storage::GrowableHeap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::{header, HeaderValue};
    use tokio_tungstenite::tungstenite::protocol::Message;

    use super::*;

    #[tokio::test]
    async fn exact_subprotocol_and_hello_admit_byte_exact_transport() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let id = BrowserRendezvousId::new([0x41; contract::ID_LEN]);
        let catalog = catalog::Catalog::new(id);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let route = http::route(stream, network::loopback(contract::PORT), &catalog)
                .await
                .unwrap();
            let http::RequestEndpoint::Upgrade(stream) = route else {
                panic!("WebSocket route expected")
            };
            transport::accept(stream, id).await.unwrap()
        });

        let target = format!("ws://{address}{}", contract::PATH);
        let mut request = target.into_client_request().unwrap();
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(contract::SUBPROTOCOL),
        );
        let (mut client, response) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(
            response
                .headers()
                .get(header::SEC_WEBSOCKET_PROTOCOL)
                .and_then(|value| value.to_str().ok()),
            Some(contract::SUBPROTOCOL)
        );
        client
            .send(Message::binary(
                prns_core::interfaces::browser_rendezvous::ClientHello::encode().to_vec(),
            ))
            .await
            .unwrap();
        let hello = client.next().await.unwrap().unwrap().into_data();
        assert_eq!(
            prns_core::interfaces::browser_rendezvous::ServerHello::decode(&hello)
                .unwrap()
                .id(),
            id
        );

        let mut server_socket = server.await.unwrap();
        let frame = [0x01, 0x7e, 0x7d, 0xff];
        client.send(Message::binary(frame.to_vec())).await.unwrap();
        assert_eq!(
            server_socket
                .next()
                .await
                .unwrap()
                .unwrap()
                .into_data()
                .as_ref(),
            frame
        );
    }

    #[tokio::test]
    async fn malformed_hello_never_becomes_a_transport_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let id = BrowserRendezvousId::new([0x42; contract::ID_LEN]);
        let catalog = catalog::Catalog::new(id);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let route = http::route(stream, network::loopback(contract::PORT), &catalog)
                .await
                .unwrap();
            let http::RequestEndpoint::Upgrade(stream) = route else {
                return false;
            };
            transport::accept(stream, id).await.is_ok()
        });

        let target = format!("ws://{address}{}", contract::PATH);
        let mut request = target.into_client_request().unwrap();
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(contract::SUBPROTOCOL),
        );
        let (mut client, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        client
            .send(Message::binary(b"unrelated service".to_vec()))
            .await
            .unwrap();

        assert!(!server.await.unwrap());
    }

    #[tokio::test]
    async fn catalog_http_is_cors_enabled_typed_json_and_no_store() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let id = BrowserRendezvousId::new([0x43; contract::ID_LEN]);
        let catalog = catalog::Catalog::new(id);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            assert!(matches!(
                http::route(stream, network::loopback(contract::PORT), &catalog,)
                    .await
                    .unwrap(),
                http::RequestEndpoint::Handled
            ));
        });

        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!(
                    "GET {} HTTP/1.1\r\nHost: attacker.example\r\nConnection: close\r\n\r\n",
                    contract::CATALOG_PATH
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        server.await.unwrap();

        let separator = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&response[..separator]).unwrap();
        let body = &response[separator + 4..];
        assert!(headers.contains("200 OK"));
        assert!(headers.contains("Access-Control-Allow-Origin: *"));
        assert!(headers.contains("Cache-Control: no-store"));
        assert!(!headers.contains("Access-Control-Allow-Credentials"));
        assert_eq!(
            decode_browser_gateway_catalog(body).unwrap(),
            vec![BrowserGatewayEndpoint::new(id, network::loopback(contract::PORT)).unwrap()]
        );
        assert!(!std::str::from_utf8(body)
            .unwrap()
            .contains("attacker.example"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn three_local_nodes_form_a_star_and_take_over_after_core_failure() {
        let node_a = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(Zeroizing::new([0xa1; IDENTITY_SECRET_KEY_LEN])),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: prns_runtime::request_endpoints![],
            remote_control: prns_runtime::remote_control::RemoteControlService::Unavailable,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_event, _state: &()| {},
        });
        let node_b = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(Zeroizing::new([0xb2; IDENTITY_SECRET_KEY_LEN])),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: prns_runtime::request_endpoints![],
            remote_control: prns_runtime::remote_control::RemoteControlService::Unavailable,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_event, _state: &()| {},
        });
        let node_c = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: Some(Zeroizing::new([0xc3; IDENTITY_SECRET_KEY_LEN])),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: prns_runtime::request_endpoints![],
            remote_control: prns_runtime::remote_control::RemoteControlService::Unavailable,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_event, _state: &()| {},
        });
        let ids = [
            BrowserRendezvousId::new([0x11; contract::ID_LEN]),
            BrowserRendezvousId::new([0x22; contract::ID_LEN]),
            BrowserRendezvousId::new([0x33; contract::ID_LEN]),
        ];
        let policy = prns_core::interfaces::websocket::configured_policy(Default::default());
        let devices = AutoWifiDevicePolicy::new(vec!["prns-test-no-lan".to_owned()], Vec::new());
        let rendezvous_a = BrowserRendezvous::new(ids[0], devices.clone(), policy);
        let rendezvous_b = BrowserRendezvous::new(ids[1], devices.clone(), policy);
        let rendezvous_c = BrowserRendezvous::new(ids[2], devices, policy);
        let statuses = [
            rendezvous_a.status(),
            rendezvous_b.status(),
            rendezvous_c.status(),
        ];
        let mut supervisors = [
            Some(node_a.handle().supervise(rendezvous_a)),
            Some(node_b.handle().supervise(rendezvous_b)),
            Some(node_c.handle().supervise(rendezvous_c)),
        ];

        let exercise = async {
            wait_until(Duration::from_secs(10), || {
                statuses
                    .iter()
                    .filter(|status| status.role() == BrowserRendezvousRole::Core)
                    .count()
                    == 1
                    && statuses
                        .iter()
                        .filter(|status| status.role() == BrowserRendezvousRole::Client)
                        .count()
                        == 2
            })
            .await;
            let first_core = statuses
                .iter()
                .position(|status| status.role() == BrowserRendezvousRole::Core)
                .unwrap();
            wait_until(Duration::from_secs(10), || {
                statuses.iter().enumerate().all(|(index, status)| {
                    index == first_core || status.core_id() == Some(ids[first_core])
                })
            })
            .await;

            supervisors[first_core].take().unwrap().teardown();
            wait_until(Duration::from_secs(10), || {
                statuses.iter().enumerate().any(|(index, status)| {
                    index != first_core && status.role() == BrowserRendezvousRole::Core
                })
            })
            .await;
            let next_core = statuses
                .iter()
                .enumerate()
                .find_map(|(index, status)| {
                    (index != first_core && status.role() == BrowserRendezvousRole::Core)
                        .then_some(index)
                })
                .unwrap();
            wait_until(Duration::from_secs(10), || {
                statuses.iter().enumerate().any(|(index, status)| {
                    index != first_core
                        && index != next_core
                        && status.role() == BrowserRendezvousRole::Client
                        && status.core_id() == Some(ids[next_core])
                })
            })
            .await;
        };

        tokio::select! {
            result = node_a.run() => panic!("first rendezvous node stopped: {result:?}"),
            result = node_b.run() => panic!("second rendezvous node stopped: {result:?}"),
            result = node_c.run() => panic!("third rendezvous node stopped: {result:?}"),
            _ = exercise => {}
        }
    }

    async fn wait_until(timeout: Duration, ready: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + timeout;
        while !ready() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for rendezvous state"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
