mod member;
mod status;

pub use status::{WeaveInterfaceIssue, WeaveInterfaceStatus};

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use prns_core::interfaces::rns_serial_framing::RnsSerialDecoder;
use prns_core::interfaces::weave::{
    self, DeviceEvent, EndpointId, MultipathDeduplicator, SwitchId, WeaveHostIdentity,
};
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceId, InterfaceKind, InterfaceStatus,
};
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

use crate::reconnect::{ReconnectPolicy, ReconnectSchedule};

use self::member::{OutboundPacket, WeavePeer, DEVICE_OUTBOUND_DEPTH, PEER_INBOUND_DEPTH};

struct ManagedPeer {
    attached: AttachedInterface,
    inbound: mpsc::Sender<Vec<u8>>,
    status: prns_runtime::manifold::driver::TokioInterfaceStatus,
    last_heard: tokio::time::Instant,
}

pub struct WeaveInterface<Open> {
    open: Open,
    reconnect_policy: ReconnectPolicy,
    policy: EffectiveInterfacePolicy,
    channel_tag: Vec<u8>,
    identity: WeaveHostIdentity,
    status: WeaveInterfaceStatus,
}

impl<Open> WeaveInterface<Open> {
    pub fn with_generated_identity(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
    ) -> Result<Self, getrandom::Error> {
        let mut secret = [0u8; 32];
        getrandom::getrandom(&mut secret)?;
        Ok(Self::with_identity(
            open,
            reconnect_policy,
            policy,
            channel_tag,
            WeaveHostIdentity::from_signing_secret(secret),
        ))
    }

    pub fn with_identity(
        open: Open,
        reconnect_policy: ReconnectPolicy,
        policy: EffectiveInterfacePolicy,
        channel_tag: &[u8],
        identity: WeaveHostIdentity,
    ) -> Self {
        let channel_tag = channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Weave, &channel_tag);
        Self {
            open,
            reconnect_policy,
            policy,
            channel_tag,
            identity,
            status: WeaveInterfaceStatus::new(id),
        }
    }

    pub fn id(&self) -> InterfaceId {
        self.status.id()
    }

    pub fn status(&self) -> WeaveInterfaceStatus {
        self.status.clone()
    }
}

impl<Open, OpenFuture, Stream> InterfaceSupervisor for WeaveInterface<Open>
where
    Open: FnMut() -> OpenFuture,
    OpenFuture: Future<Output = io::Result<Stream>>,
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    const KIND: InterfaceKind = InterfaceKind::Weave;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(mut self, fleet: Fleet) {
        let mut reconnect = self.reconnect_policy.schedule();
        loop {
            self.status.wait_until_enabled().await;
            self.status.begin_connection_attempt();
            let stream = match (self.open)().await {
                Ok(stream) => stream,
                Err(error) => {
                    self.status.complete_initial_attempt();
                    self.status
                        .set_issue(WeaveInterfaceIssue::SerialUnavailable);
                    crate::diagnostic_log::warn!(
                        "Weave interface {:?} could not open: {error}",
                        self.id().as_bytes()
                    );
                    if !wait_for_retry(&self.status, &fleet, &mut reconnect).await {
                        continue;
                    }
                    continue;
                }
            };
            let mut connected_at = None;
            let result = serve_connection(
                stream,
                &fleet,
                &self.status,
                &self.identity,
                self.policy,
                &self.channel_tag,
                &mut connected_at,
            )
            .await;
            if let Some(connected_at) = connected_at {
                reconnect.record_connection_lifetime(connected_at.elapsed());
            }
            self.status.complete_initial_attempt();
            self.status.mark_disconnected();
            let issue = match result {
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                    crate::diagnostic_log::warn!(
                        "Weave interface {:?} handshake timed out",
                        self.id().as_bytes()
                    );
                    WeaveInterfaceIssue::HandshakeTimedOut
                }
                Err(error) => {
                    crate::diagnostic_log::warn!(
                        "Weave interface {:?} connection closed: {error}",
                        self.id().as_bytes()
                    );
                    WeaveInterfaceIssue::ConnectionLost
                }
                Ok(()) => WeaveInterfaceIssue::None,
            };
            self.status.set_issue(issue);
            let _ = wait_for_retry(&self.status, &fleet, &mut reconnect).await;
        }
    }
}

impl<Open> prns_core::interfaces::ReportsStatus for WeaveInterface<Open> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

async fn wait_for_retry(
    status: &WeaveInterfaceStatus,
    fleet: &Fleet,
    reconnect: &mut ReconnectSchedule,
) -> bool {
    let delay = reconnect.next_delay(|bytes| fleet.fill_entropy(bytes));
    tokio::select! {
        () = tokio::time::sleep(delay) => true,
        () = status.wait_until_disabled() => false,
    }
}

async fn serve_connection<Stream>(
    mut stream: Stream,
    fleet: &Fleet,
    status: &WeaveInterfaceStatus,
    identity: &WeaveHostIdentity,
    policy: EffectiveInterfacePolicy,
    parent_channel_tag: &[u8],
    connected_at: &mut Option<tokio::time::Instant>,
) -> io::Result<()>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    let mut framed = vec![0u8; weave::FRAMED_LEN];
    let discovery_len = weave::encode_discovery(identity, &mut framed).map_err(encoding_error)?;
    stream.write_all(&framed[..discovery_len]).await?;

    let (outbound_tx, mut outbound_rx) = mpsc::channel(DEVICE_OUTBOUND_DEPTH);
    let mut peers = HashMap::<EndpointId, ManagedPeer>::new();
    let mut decoder = Box::new(RnsSerialDecoder::<{ weave::WDCL_MAX_CHUNK }>::new());
    let mut read = vec![0u8; weave::READ_BUF_LEN];
    let mut raw = vec![0u8; SwitchId::LEN + 1 + 2 + EndpointId::LEN + weave::WEAVE_MAX_WIRE_PACKET];
    let mut remote_switch = None;
    let mut deduplicator = MultipathDeduplicator::new();
    let started = tokio::time::Instant::now();
    let handshake_deadline = tokio::time::sleep(std::time::Duration::from_millis(
        weave::HANDSHAKE_TIMEOUT_MILLIS,
    ));
    tokio::pin!(handshake_deadline);
    let mut peer_jobs = tokio::time::interval(std::time::Duration::from_secs(1));

    let result = loop {
        tokio::select! {
            () = status.wait_until_disabled() => break Ok(()),
            () = &mut handshake_deadline, if status.connection() != ConnectionState::Connected => {
                break Err(io::Error::new(io::ErrorKind::TimedOut, "Weave handshake timed out"));
            }
            _ = peer_jobs.tick() => {
                remove_expired_peers(&mut peers, status);
            }
            outbound = outbound_rx.recv() => {
                let Some(OutboundPacket { endpoint, payload }) = outbound else {
                    break Err(io::Error::new(io::ErrorKind::BrokenPipe, "Weave peer output closed"));
                };
                let Some(remote_switch) = remote_switch else {
                    continue;
                };
                let written = weave::encode_endpoint_packet(
                    remote_switch,
                    endpoint,
                    &payload,
                    &mut raw,
                    &mut framed,
                )
                .map_err(encoding_error)?;
                stream.write_all(&framed[..written]).await?;
            }
            read_result = stream.read(&mut read) => {
                let read_len = match read_result {
                    Ok(0) => break Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                    Ok(read_len) => read_len,
                    Err(error) => break Err(error),
                };
                let mut offset = 0;
                while offset < read_len {
                    let frame = match decoder.feed_slice_next(&read[..read_len], &mut offset) {
                        Ok(Some(frame)) => frame,
                        Ok(None) => break,
                        Err(_) => continue,
                    };
                    let event = match weave::decode_device_frame(frame, identity.switch_id()) {
                        Ok(event) => event,
                        Err(weave::DecodeError::InvalidDiscoverySignature) => {
                            crate::diagnostic_log::warn!("Weave device returned an invalid discovery signature");
                            continue;
                        }
                    };
                    match event {
                        DeviceEvent::Discovered { switch_id, .. } => {
                            remote_switch = Some(switch_id);
                            status.set_remote_switch(switch_id);
                            let written = weave::encode_handshake(identity, switch_id, &mut framed)
                                .map_err(encoding_error)?;
                            stream.write_all(&framed[..written]).await?;
                        }
                        DeviceEvent::Connected if remote_switch.is_some() => {
                            *connected_at = Some(tokio::time::Instant::now());
                            status.mark_connected();
                            status.set_issue(WeaveInterfaceIssue::None);
                            status.complete_initial_attempt();
                        }
                        DeviceEvent::HostEndpoint(endpoint) => status.set_host_endpoint(endpoint),
                        DeviceEvent::EndpointAlive(endpoint) => {
                            ensure_peer(
                                endpoint,
                                &mut peers,
                                fleet,
                                status,
                                policy,
                                parent_channel_tag,
                                &outbound_tx,
                            );
                        }
                        DeviceEvent::EndpointPacket { source, payload } => {
                            ensure_peer(
                                source,
                                &mut peers,
                                fleet,
                                status,
                                policy,
                                parent_channel_tag,
                                &outbound_tx,
                            );
                            if deduplicator.accepts(payload, started.elapsed().as_millis() as u64) {
                                if let Some(peer) = peers.get(&source) {
                                    let _ = peer.inbound.try_send(payload.to_vec());
                                }
                            }
                        }
                        DeviceEvent::EndpointTimedOut(_)
                        | DeviceEvent::EndpointVia { .. }
                        | DeviceEvent::Connected
                        | DeviceEvent::Ignored => {}
                    }
                }
            }
        }
    };
    teardown_peers(&mut peers, status);
    result
}

fn ensure_peer(
    endpoint: EndpointId,
    peers: &mut HashMap<EndpointId, ManagedPeer>,
    fleet: &Fleet,
    status: &WeaveInterfaceStatus,
    policy: EffectiveInterfacePolicy,
    parent_channel_tag: &[u8],
    outbound: &mpsc::Sender<OutboundPacket>,
) {
    if let Some(peer) = peers.get_mut(&endpoint) {
        peer.last_heard = tokio::time::Instant::now();
        return;
    }
    let (inbound_tx, inbound_rx) = mpsc::channel(PEER_INBOUND_DEPTH);
    let peer = WeavePeer::new(
        parent_channel_tag,
        endpoint,
        policy,
        inbound_rx,
        outbound.clone(),
    );
    let peer_status = peer.status();
    let endpoint_name = format_weave_peer_name(endpoint);
    let attached = fleet.add_named(peer, endpoint_name, None);
    peers.insert(
        endpoint,
        ManagedPeer {
            attached,
            inbound: inbound_tx,
            status: peer_status,
            last_heard: tokio::time::Instant::now(),
        },
    );
    publish_members(peers, status);
}

fn remove_expired_peers(
    peers: &mut HashMap<EndpointId, ManagedPeer>,
    status: &WeaveInterfaceStatus,
) {
    let now = tokio::time::Instant::now();
    let expired = peers
        .iter()
        .filter_map(|(endpoint, peer)| {
            (now.duration_since(peer.last_heard).as_millis()
                > u128::from(weave::PEERING_TIMEOUT_MILLIS))
            .then_some(*endpoint)
        })
        .collect::<Vec<_>>();
    for endpoint in expired {
        if let Some(peer) = peers.remove(&endpoint) {
            peer.attached.teardown();
        }
    }
    publish_members(peers, status);
}

fn teardown_peers(peers: &mut HashMap<EndpointId, ManagedPeer>, status: &WeaveInterfaceStatus) {
    for (_, peer) in peers.drain() {
        peer.attached.teardown();
    }
    status.set_members(Vec::new());
}

fn publish_members(peers: &HashMap<EndpointId, ManagedPeer>, status: &WeaveInterfaceStatus) {
    status.set_members(peers.values().map(|peer| peer.status.clone()).collect());
}

fn format_weave_peer_name(endpoint: EndpointId) -> String {
    let bytes = endpoint.as_bytes();
    format!(
        "endpoint {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
    )
}

fn encoding_error(error: weave::EncodeError) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("could not encode Weave WDCL frame: {error:?}"),
    )
}
