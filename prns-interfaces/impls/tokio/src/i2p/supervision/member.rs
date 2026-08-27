use tokio::io::BufReader;
use tokio::sync::mpsc;

use prns_core::interfaces::i2p;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

use crate::byte_stream::framing;

use super::super::sam::{I2pPublicDestination, SamSessionDestination};
use super::super::{
    generate_session_id, SamBridgeTransport, SamFailureClass, SamSessionTransport,
    SamTransportError,
};
use super::config::{I2pPeerAddress, I2pRetryPolicy};
use super::status::{I2pInterfaceIssue, I2pPeerStatus};

pub(crate) enum I2pMemberEvent {
    InitialAttempt(InterfaceId),
    Closed(InterfaceId),
}

pub(crate) struct I2pConfiguredPeer<B> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    bridge: B,
    peer: I2pPeerAddress,
    policy: EffectiveInterfacePolicy,
    retry: I2pRetryPolicy,
    status: I2pPeerStatus,
    events: mpsc::UnboundedSender<I2pMemberEvent>,
}

impl<B> I2pConfiguredPeer<B> {
    pub(crate) fn new(
        bridge: B,
        peer: I2pPeerAddress,
        policy: EffectiveInterfacePolicy,
        retry: I2pRetryPolicy,
        events: mpsc::UnboundedSender<I2pMemberEvent>,
    ) -> Self {
        let channel_tag = configured_channel_tag(&peer);
        let id = InterfaceId::from_channel_tag(InterfaceKind::I2pPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            bridge,
            peer,
            policy,
            retry,
            status: I2pPeerStatus::new(id, ConnectionState::Initializing),
            events,
        }
    }

    pub(crate) fn id(&self) -> InterfaceId {
        self.id
    }

    pub(crate) fn status(&self) -> I2pPeerStatus {
        self.status.clone()
    }
}

impl<B> Interface for I2pConfiguredPeer<B>
where
    B: SamBridgeTransport,
{
    const HW_MTU: usize = i2p::I2P_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::I2pPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        i2p::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers = framing::FramedBuffers::<
            framing::HdlcFraming,
            { i2p::READ_BUF_LEN },
            { i2p::FRAMED_LEN },
        >::new();
        let mut session: Option<B::Session> = None;
        let mut initial_attempt_reported = false;
        loop {
            if session.is_none() {
                self.status.set_connection(if initial_attempt_reported {
                    ConnectionState::Reconnecting
                } else {
                    ConnectionState::Initializing
                });
                let id = match generate_session_id() {
                    Ok(id) => id,
                    Err(error) => {
                        self.status.set_issue(I2pInterfaceIssue::EntropyUnavailable);
                        crate::diagnostic_log::error!("i2p peer [{}]: {error}", self.peer.as_str());
                        report_initial_attempt(
                            &self.events,
                            self.id,
                            &mut initial_attempt_reported,
                        );
                        tokio::time::sleep(self.retry.tunnel_setup_interval()).await;
                        continue;
                    }
                };
                match self
                    .bridge
                    .create_session(id, SamSessionDestination::Transient)
                    .await
                {
                    Ok(created) => {
                        session = Some(created);
                        self.status.clear_issue();
                    }
                    Err(error) => {
                        self.status.set_issue(I2pInterfaceIssue::SamUnavailable);
                        crate::diagnostic_log::error!(
                            "i2p peer [{}]: could not create SAM session: {error}",
                            self.peer.as_str()
                        );
                    }
                }
                report_initial_attempt(&self.events, self.id, &mut initial_attempt_reported);
                if session.is_none() {
                    tokio::time::sleep(self.retry.tunnel_setup_interval()).await;
                    continue;
                }
            }

            let destination = match resolve_peer(&self.bridge, &self.peer).await {
                Ok(destination) => destination,
                Err(error) => {
                    self.status.set_issue(issue_for(&error));
                    self.status.set_connection(ConnectionState::Reconnecting);
                    crate::diagnostic_log::error!(
                        "i2p peer [{}]: destination lookup failed: {error}",
                        self.peer.as_str()
                    );
                    tokio::time::sleep(self.retry.peer_reconnect_interval()).await;
                    continue;
                }
            };

            let Some(active_session) = session.as_ref() else {
                continue;
            };
            let stream = match active_session.connect(destination).await {
                Ok(stream) => stream,
                Err(error) => {
                    self.status.set_issue(issue_for(&error));
                    self.status.set_connection(ConnectionState::Reconnecting);
                    crate::diagnostic_log::error!(
                        "i2p peer [{}]: stream connection failed: {error}",
                        self.peer.as_str()
                    );
                    session = None;
                    tokio::time::sleep(self.retry.peer_reconnect_interval()).await;
                    continue;
                }
            };

            self.status.clear_issue();
            self.status.set_connection(ConnectionState::Connected);
            seam.request_tunnel_synthesis().await;
            framing::serve_with_hdlc_idle_watchdog(
                stream,
                &mut buffers,
                &mut seam,
                &mut framing::WireMeters {
                    status: self.status.wire(),
                    airtime: &mut airtime,
                    throughput: &mut throughput,
                    bitrate: self.policy.bitrate,
                    started,
                },
            )
            .await;
            self.status.set_connection(ConnectionState::Reconnecting);
            tokio::time::sleep(self.retry.peer_reconnect_interval()).await;
        }
    }
}

impl<B> prns_core::interfaces::ReportsStatus for I2pConfiguredPeer<B> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }

    fn frame_accounting_recorder(&self) -> Option<prns_core::interfaces::FrameAccountingRecorder> {
        prns_core::interfaces::FrameAccountingRecorder::of(self.status())
    }
}

pub(crate) struct I2pAcceptedPeer<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<BufReader<S>>,
    policy: EffectiveInterfacePolicy,
    status: I2pPeerStatus,
    events: mpsc::UnboundedSender<I2pMemberEvent>,
}

impl<S> I2pAcceptedPeer<S> {
    pub(crate) fn new(
        peer: I2pPublicDestination,
        connection_number: u64,
        stream: BufReader<S>,
        policy: EffectiveInterfacePolicy,
        events: mpsc::UnboundedSender<I2pMemberEvent>,
    ) -> Self {
        let channel_tag = accepted_channel_tag(&peer, connection_number);
        let id = InterfaceId::from_channel_tag(InterfaceKind::I2pPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            policy,
            status: I2pPeerStatus::new(id, ConnectionState::Connected),
            events,
        }
    }

    pub(crate) fn id(&self) -> InterfaceId {
        self.id
    }

    pub(crate) fn status(&self) -> I2pPeerStatus {
        self.status.clone()
    }
}

impl<S> Interface for I2pAcceptedPeer<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    const HW_MTU: usize = i2p::I2P_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::I2pPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        i2p::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers = framing::FramedBuffers::<
            framing::HdlcFraming,
            { i2p::READ_BUF_LEN },
            { i2p::FRAMED_LEN },
        >::new();
        framing::serve_with_hdlc_idle_watchdog(
            stream,
            &mut buffers,
            &mut seam,
            &mut framing::WireMeters {
                status: self.status.wire(),
                airtime: &mut airtime,
                throughput: &mut throughput,
                bitrate: self.policy.bitrate,
                started,
            },
        )
        .await;
        self.status.set_connection(ConnectionState::Disconnected);
        let _ = self.events.send(I2pMemberEvent::Closed(self.id));
    }
}

impl<S> prns_core::interfaces::ReportsStatus for I2pAcceptedPeer<S> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }

    fn frame_accounting_recorder(&self) -> Option<prns_core::interfaces::FrameAccountingRecorder> {
        prns_core::interfaces::FrameAccountingRecorder::of(self.status())
    }
}

fn configured_channel_tag(peer: &I2pPeerAddress) -> Vec<u8> {
    let mut tag = b"configured\0".to_vec();
    tag.extend_from_slice(peer.as_str().as_bytes());
    tag
}

fn accepted_channel_tag(peer: &I2pPublicDestination, connection_number: u64) -> Vec<u8> {
    let mut tag = b"accepted\0".to_vec();
    tag.extend_from_slice(peer.as_str().as_bytes());
    tag.extend_from_slice(&connection_number.to_be_bytes());
    tag
}

fn report_initial_attempt(
    events: &mpsc::UnboundedSender<I2pMemberEvent>,
    id: InterfaceId,
    reported: &mut bool,
) {
    if *reported {
        return;
    }
    *reported = true;
    let _ = events.send(I2pMemberEvent::InitialAttempt(id));
}

async fn resolve_peer<B>(
    bridge: &B,
    peer: &I2pPeerAddress,
) -> Result<I2pPublicDestination, B::Error>
where
    B: SamBridgeTransport,
{
    match peer {
        I2pPeerAddress::Named(name) => bridge.resolve_destination(name.clone()).await,
        I2pPeerAddress::Destination(destination) => Ok(destination.clone()),
    }
}

fn issue_for(error: &impl SamTransportError) -> I2pInterfaceIssue {
    match error.failure_class() {
        SamFailureClass::SamUnavailable => I2pInterfaceIssue::SamUnavailable,
        SamFailureClass::PeerUnreachable => I2pInterfaceIssue::PeerUnreachable,
    }
}
