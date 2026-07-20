use std::collections::{HashMap, HashSet};
use std::fmt;

use tokio::sync::mpsc;

use prns_core::interfaces::{InterfaceId, InterfaceKind, InterfaceStatus};
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};

use super::super::destination::{
    load_destination, persist_destination, I2pDestinationKeyPath, I2pDestinationStorageError,
};
use super::super::sam::{I2pBase32Address, SamSessionDestination, SamValueError};
use super::super::{
    generate_session_id, I2pSessionIdError, SamBridgeTransport, SamSessionTransport,
};
use super::member::{I2pAcceptedPeer, I2pConfiguredPeer, I2pMemberEvent};
use super::status::{I2pInterfaceStatus, I2pPeerStatus, I2pRuntimeIssue};
use super::types::{I2pReachability, I2pRuntimeConfig};

pub struct I2pInterface<B> {
    bridge: B,
    config: I2pRuntimeConfig,
    status: I2pInterfaceStatus,
}

impl<B> I2pInterface<B> {
    pub fn new(bridge: B, config: I2pRuntimeConfig) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::I2p, config.name.as_str().as_bytes());
        let expects_activity = !config.peers.is_empty() || config.reachability.is_connectable();
        Self {
            bridge,
            config,
            status: I2pInterfaceStatus::new(id, expects_activity),
        }
    }

    pub fn id(&self) -> InterfaceId {
        self.status.id()
    }

    pub fn status(&self) -> I2pInterfaceStatus {
        self.status.clone()
    }
}

struct ManagedMember {
    attached: AttachedInterface,
    status: I2pPeerStatus,
}

struct RuntimeCycle<'a, B> {
    bridge: &'a B,
    config: &'a I2pRuntimeConfig,
    fleet: &'a Fleet,
    status: &'a I2pInterfaceStatus,
    events_tx: mpsc::UnboundedSender<I2pMemberEvent>,
    events_rx: mpsc::UnboundedReceiver<I2pMemberEvent>,
    members: HashMap<InterfaceId, ManagedMember>,
    pending_initial: HashSet<InterfaceId>,
    endpoint_initial_pending: bool,
    connection_number: u64,
}

impl<'a, B> RuntimeCycle<'a, B>
where
    B: SamBridgeTransport,
{
    fn new(
        bridge: &'a B,
        config: &'a I2pRuntimeConfig,
        fleet: &'a Fleet,
        status: &'a I2pInterfaceStatus,
    ) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let mut cycle = Self {
            bridge,
            config,
            fleet,
            status,
            events_tx,
            events_rx,
            members: HashMap::new(),
            pending_initial: HashSet::new(),
            endpoint_initial_pending: config.reachability.is_connectable(),
            connection_number: 0,
        };
        cycle.attach_configured_peers();
        cycle.refresh_status();
        cycle.settle_initial_attempts();
        cycle
    }

    fn attach_configured_peers(&mut self) {
        for peer in self.config.peers.iter().cloned() {
            let member = I2pConfiguredPeer::new(
                self.bridge.clone(),
                peer,
                self.config.policy,
                self.config.retry,
                self.events_tx.clone(),
            );
            let id = member.id();
            let status = member.status();
            let attached = self.fleet.add(member);
            self.pending_initial.insert(id);
            self.members.insert(id, ManagedMember { attached, status });
        }
    }

    fn attach_accepted_peer(&mut self, accepted: super::super::sam::I2pAcceptedStream<B::Stream>) {
        self.connection_number = self.connection_number.wrapping_add(1);
        let member = I2pAcceptedPeer::new(
            accepted.peer,
            self.connection_number,
            accepted.stream,
            self.config.policy,
            self.events_tx.clone(),
        );
        let id = member.id();
        let status = member.status();
        let attached = self.fleet.add(member);
        self.members.insert(id, ManagedMember { attached, status });
        self.refresh_status();
    }

    async fn run_outbound_only(&mut self) {
        loop {
            tokio::select! {
                () = self.status.wait_until_disabled() => return,
                event = self.events_rx.recv() => {
                    let Some(event) = event else {
                        return;
                    };
                    self.apply_member_event(event);
                }
            }
        }
    }

    async fn run_connectable(&mut self, key_path: &I2pDestinationKeyPath) {
        loop {
            if !self.status.is_enabled() {
                return;
            }
            let opened = open_endpoint(self.bridge, key_path).await;
            self.endpoint_initial_pending = false;
            self.settle_initial_attempts();
            let (session, published) = match opened {
                Ok(opened) => opened,
                Err(error) => {
                    self.status.mark_listener_offline();
                    self.status.set_issue(error.issue());
                    crate::diagnostic_log::error!(
                        "i2p interface [{}]: endpoint setup failed: {error}",
                        self.config.name.as_str()
                    );
                    if !self
                        .wait_retry(self.config.retry.endpoint_retry_interval())
                        .await
                    {
                        return;
                    }
                    continue;
                }
            };
            self.status.set_issue(I2pRuntimeIssue::None);
            self.status.set_published_destination(published);
            self.status.mark_listener_online();
            loop {
                tokio::select! {
                    () = self.status.wait_until_disabled() => return,
                    event = self.events_rx.recv() => {
                        let Some(event) = event else {
                            return;
                        };
                        self.apply_member_event(event);
                    }
                    accepted = session.accept() => match accepted {
                        Ok(accepted) => self.attach_accepted_peer(accepted),
                        Err(error) => {
                            self.status.mark_listener_offline();
                            self.status.set_issue(I2pRuntimeIssue::SamUnavailable);
                            crate::diagnostic_log::error!(
                                "i2p interface [{}]: accept failed: {error}",
                                self.config.name.as_str()
                            );
                            break;
                        }
                    }
                }
            }
            if !self
                .wait_retry(self.config.retry.endpoint_retry_interval())
                .await
            {
                return;
            }
        }
    }

    async fn wait_retry(&mut self, interval: std::time::Duration) -> bool {
        let sleep = tokio::time::sleep(interval);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = self.status.wait_until_disabled() => return false,
                () = &mut sleep => return true,
                event = self.events_rx.recv() => {
                    let Some(event) = event else {
                        return false;
                    };
                    self.apply_member_event(event);
                }
            }
        }
    }

    fn apply_member_event(&mut self, event: I2pMemberEvent) {
        match event {
            I2pMemberEvent::InitialAttempt(id) => {
                self.pending_initial.remove(&id);
            }
            I2pMemberEvent::Closed(id) => {
                self.members.remove(&id);
            }
        }
        self.refresh_status();
        self.settle_initial_attempts();
    }

    fn refresh_status(&self) {
        self.status.set_members(
            self.members
                .values()
                .map(|member| member.status.clone())
                .collect(),
        );
    }

    fn settle_initial_attempts(&self) {
        if self.pending_initial.is_empty() && !self.endpoint_initial_pending {
            self.status.complete_initial_attempts();
        }
    }

    fn teardown(&mut self) {
        self.status.mark_listener_offline();
        for (_, member) in self.members.drain() {
            member.attached.teardown();
        }
        self.status.set_members(Vec::new());
    }
}

enum EndpointError<E> {
    Storage(I2pDestinationStorageError),
    SessionId(I2pSessionIdError),
    Destination(SamValueError),
    Sam(E),
}

impl<E: fmt::Display> fmt::Display for EndpointError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "{error}"),
            Self::SessionId(error) => write!(formatter, "{error}"),
            Self::Destination(error) => write!(formatter, "{error}"),
            Self::Sam(error) => write!(formatter, "{error}"),
        }
    }
}

impl<E> EndpointError<E> {
    fn issue(&self) -> I2pRuntimeIssue {
        match self {
            Self::Storage(_) | Self::Destination(_) => I2pRuntimeIssue::DestinationStorage,
            Self::SessionId(_) => I2pRuntimeIssue::EntropyUnavailable,
            Self::Sam(_) => I2pRuntimeIssue::SamUnavailable,
        }
    }
}

impl<B> InterfaceSupervisor for I2pInterface<B>
where
    B: SamBridgeTransport,
{
    const KIND: InterfaceKind = InterfaceKind::I2p;

    fn channel_tag(&self) -> &[u8] {
        self.config.name.as_str().as_bytes()
    }

    async fn run(self, fleet: Fleet) {
        let Self {
            bridge,
            config,
            status,
        } = self;
        loop {
            status.wait_until_enabled().await;
            status.begin_cycle();
            let mut cycle = RuntimeCycle::new(&bridge, &config, &fleet, &status);
            match &config.reachability {
                I2pReachability::OutboundOnly => cycle.run_outbound_only().await,
                I2pReachability::Connectable { key_path } => {
                    cycle.run_connectable(key_path).await;
                }
            }
            cycle.teardown();
        }
    }
}

impl<B> prns_core::interfaces::ReportsStatus for I2pInterface<B> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

async fn open_endpoint<B>(
    bridge: &B,
    key_path: &I2pDestinationKeyPath,
) -> Result<(B::Session, I2pBase32Address), EndpointError<B::Error>>
where
    B: SamBridgeTransport,
{
    let private = match load_destination(key_path).map_err(EndpointError::Storage)? {
        Some(private) => private,
        None => {
            let generated = bridge
                .generate_destination()
                .await
                .map_err(EndpointError::Sam)?;
            persist_destination(key_path, generated.private).map_err(EndpointError::Storage)?
        }
    };
    let published = private
        .public_destination()
        .and_then(|destination| destination.base32_address())
        .map_err(EndpointError::Destination)?;
    let id = generate_session_id().map_err(EndpointError::SessionId)?;
    let session = bridge
        .create_session(id, SamSessionDestination::Persistent(private))
        .await
        .map_err(EndpointError::Sam)?;
    Ok((session, published))
}
