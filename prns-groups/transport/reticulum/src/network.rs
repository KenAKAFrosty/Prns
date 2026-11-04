//! Borrowed Reticulum host operations for the bounded group synchronization carrier.

use personal_rns::identity::PublicIdentityMaterial;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::routing::links::request::{
    packed_binary_len, write_packed_binary_header, MAX_PACKED_BINARY_HEADER_LEN,
    REQUEST_WIRE_OVERHEAD,
};
use personal_rns::routing::links::resources::MAX_EFFICIENT_SIZE;
use personal_rns::routing::request_handlers::RequestPathHash as EngineRequestPathHash;
use prns_groups_core::sync::{PathCapabilities, SyncPolicy};
use prns_groups_service::Endpoint;
use prns_host::{
    CommandOutcome, DestinationHash, DestinationIdentityConfig, DestinationName, HostCommand,
    IdentityHash, LinkId, RequestHandlerConfig, RequestPathHash, RequestPolicy, ResourceStrategy,
    SingleDestinationConfig,
};
use prns_host_runtime::owner::HostClient;
use prns_host_runtime::service_scope::HostServiceScope;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::ingress::RequestJob;
use crate::{destination_for, Limits, NetworkError, Peer, APP_NAME, ASPECTS, REQUEST_PATH};

/// Commands use the application's existing host; this value never stops it.
#[derive(Clone)]
pub struct Network {
    host: HostClient,
    scope: Option<HostServiceScope>,
    local_destination: DestinationHash,
    local_identity: IdentityHash,
    peers: Vec<Peer>,
    limits: Limits,
    policy: SyncPolicy,
    push_enabled: bool,
}

impl Network {
    pub fn new(
        host: HostClient,
        local_destination: DestinationHash,
        peers: Vec<Peer>,
        limits: Limits,
    ) -> Result<Self, NetworkError> {
        let local_identity = host.identity_hash().map_err(NetworkError::Session)?;
        if !host
            .destination_hashes()
            .map_err(NetworkError::Session)?
            .contains(&local_destination)
        {
            return Err(NetworkError::IdentityMismatch);
        }
        Self::build(host, None, local_identity, local_destination, peers, limits)
    }

    /// Use an explicitly registered identity on a shared host. The embedding
    /// validates live endpoint registration before creating the immutable scope;
    /// the host's startup destination list is not dynamic registration evidence.
    pub fn scoped(
        scope: HostServiceScope,
        local_destination: DestinationHash,
        peers: Vec<Peer>,
        limits: Limits,
    ) -> Result<Self, NetworkError> {
        scope.check_open().map_err(NetworkError::Scope)?;
        if !scope.contains_destination(local_destination) {
            return Err(NetworkError::IdentityMismatch);
        }
        Self::build(
            scope.host().clone(),
            Some(scope.clone()),
            scope.identity_hash(),
            local_destination,
            peers,
            limits,
        )
    }

    fn build(
        host: HostClient,
        scope: Option<HostServiceScope>,
        local_identity: IdentityHash,
        local_destination: DestinationHash,
        peers: Vec<Peer>,
        limits: Limits,
    ) -> Result<Self, NetworkError> {
        validate_limits(limits)?;
        if peers.len() > limits.max_peers {
            return Err(NetworkError::InvalidPeer);
        }
        if destination_for(local_identity)? != local_destination {
            return Err(NetworkError::IdentityMismatch);
        }
        for (index, peer) in peers.iter().enumerate() {
            if destination_for(peer.identity)? != peer.destination
                || peers[..index].iter().any(|prior| {
                    prior.endpoint == peer.endpoint
                        || prior.identity == peer.identity
                        || prior.destination == peer.destination
                })
            {
                return Err(NetworkError::InvalidPeer);
            }
        }
        Ok(Self {
            host,
            scope,
            local_destination,
            local_identity,
            peers,
            limits,
            policy: SyncPolicy::default(),
            push_enabled: true,
        })
    }

    /// Temporarily suppress capability probes for a known legacy peer. This is
    /// a local optimization only; unsupported observations never mean delivery.
    pub fn with_push_enabled(mut self, enabled: bool) -> Self {
        self.push_enabled = enabled;
        self
    }

    pub(crate) const fn push_enabled(&self) -> bool {
        self.push_enabled
    }

    /// Override bounded transfer and scheduling policy without changing the wire.
    pub fn with_sync_policy(mut self, policy: SyncPolicy) -> Result<Self, NetworkError> {
        if !policy.is_valid() {
            return Err(NetworkError::InvalidLimits);
        }
        self.policy = policy;
        Ok(self)
    }

    #[must_use]
    pub const fn sync_policy(&self) -> SyncPolicy {
        self.policy
    }

    /// Read the route Reticulum currently selects, without creating a Link or
    /// emitting a path request. Missing routes remain conservative/unknown.
    pub async fn path_capabilities(&self, peer: &Peer) -> Result<PathCapabilities, NetworkError> {
        if self.peer(peer.endpoint) != Some(peer) {
            return Err(NetworkError::UnknownPeer);
        }
        path_capabilities(&self.host, peer.destination, self.carrier_limits()).await
    }

    fn carrier_limits(&self) -> CarrierLimits {
        CarrierLimits {
            policy: self.policy,
            ..self.limits.into()
        }
    }

    /// Populate the destination's initially empty identified-requester allow list.
    /// Caller-supplied peer bindings are trusted provisioning data for this slice.
    pub async fn authorize_peers(&self) -> Result<(), NetworkError> {
        for peer in &self.peers {
            match execute(
                &self.host,
                self.scope.as_ref(),
                self.limits,
                HostCommand::AllowRequester {
                    destination: self.local_destination,
                    path_hash: request_path(),
                    identity: peer.identity,
                },
            )
            .await?
            {
                CommandOutcome::RequesterAllowed => {}
                _ => return Err(NetworkError::UnexpectedOutcome),
            }
        }
        Ok(())
    }

    /// Authenticate the responder destination, then identify this host to its
    /// allow-listed request path. All established links have scoped ownership.
    pub async fn connect(&self, peer: &Peer) -> Result<Connection, NetworkError> {
        let Some(known) = self.peer(peer.endpoint) else {
            return Err(NetworkError::UnknownPeer);
        };
        if known.identity != peer.identity || known.destination != peer.destination {
            return Err(NetworkError::UnknownPeer);
        }
        connect_identified(
            &self.host,
            self.scope.as_ref(),
            self.local_identity,
            peer.identity,
            peer.destination,
            self.carrier_limits(),
            REQUEST_PATH,
        )
        .await
    }

    pub async fn respond(&self, job: &RequestJob, frame: &[u8]) -> Result<(), NetworkError> {
        if self.requester(job).is_none() {
            return Err(NetworkError::UnknownPeer);
        }
        if job.destination != self.local_destination || job.path != request_path() {
            return Err(NetworkError::UnknownPeer);
        }
        respond(
            &self.host,
            self.scope.as_ref(),
            job,
            frame,
            self.limits.into(),
        )
    }

    #[must_use]
    pub fn peer(&self, endpoint: Endpoint) -> Option<&Peer> {
        self.peers.iter().find(|peer| peer.endpoint == endpoint)
    }

    #[must_use]
    pub fn requester(&self, job: &RequestJob) -> Option<Endpoint> {
        // Validate the dispatch binding before any mutable service operation,
        // not only when queuing its eventual response.
        if job.destination != self.local_destination || job.path != request_path() {
            return None;
        }
        self.peers
            .iter()
            .find(|peer| peer.identity == job.requester)
            .map(|peer| peer.endpoint)
    }

    #[must_use]
    pub const fn local_identity(&self) -> IdentityHash {
        self.local_identity
    }

    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }
}

/// Shared carrier bounds; enrollment has no inventory page or MLS peer limits.
#[derive(Clone, Copy)]
pub(crate) struct CarrierLimits {
    pub(crate) max_frame_bytes: usize,
    pub(crate) exchange_timeout: std::time::Duration,
    pub(crate) policy: SyncPolicy,
}
impl From<Limits> for CarrierLimits {
    fn from(limits: Limits) -> Self {
        Self {
            max_frame_bytes: limits.max_frame_bytes,
            exchange_timeout: limits.exchange_timeout,
            policy: SyncPolicy::default(),
        }
    }
}

impl CarrierLimits {
    fn bounded_timeout(self, adaptive_millis: u64) -> Duration {
        let floor = u64::try_from(self.exchange_timeout.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(
            adaptive_millis
                .max(floor)
                .min(self.policy.max_exchange_millis),
        )
    }

    fn connect_timeout(self, path: PathCapabilities) -> Duration {
        self.bounded_timeout(self.policy.connect_deadline_millis(path))
    }

    fn request_timeout(self, path: PathCapabilities, request: usize, response: usize) -> Duration {
        // Reticulum allows application processing grace before a response or
        // Resource advertisement. A fast nominal link must not make our outer
        // deadline shorter than that native allowance. Explicit application
        // maximums can still impose a deliberately tighter bound.
        let native = personal_rns::routing::links::data::link_traffic_timeout_ms(
            personal_rns::units::RttMillis::new(
                path.rtt_millis.unwrap_or(self.policy.unknown_rtt_millis),
            ),
        )
        .saturating_add(personal_rns::routing::links::request::REQUEST_RESPONSE_GRACE_MS);
        self.bounded_timeout(
            self.policy
                .exchange_deadline_millis(path, request, response)
                .max(native),
        )
    }
}

pub(crate) async fn path_capabilities(
    host: &HostClient,
    destination: DestinationHash,
    limits: CarrierLimits,
) -> Result<PathCapabilities, NetworkError> {
    let services = host.service_client().map_err(NetworkError::Session)?;
    let protocol = services.protocols();
    let route = tokio::time::timeout(
        limits.exchange_timeout,
        protocol.route(personal_rns::wire::DestinationHash::new(
            destination.into_bytes(),
        )),
    )
    .await
    .map_err(|_| NetworkError::Timeout)?;
    let timing = protocol.interface_timing_inventory();
    Ok(capabilities_for_route(route.as_ref(), &timing))
}

fn capabilities_for_route(
    route: Option<&personal_rns::node_introspection::RouteSnapshot>,
    timing: &[personal_rns::node_introspection::InterfaceTimingSnapshot],
) -> PathCapabilities {
    match route {
        Some(route) => PathCapabilities {
            local_bitrate_bps: timing
                .iter()
                .find(|entry| {
                    entry.id == route.interface
                        && entry.connection.is_online()
                        && entry.capabilities.allows_transmit()
                })
                .map(|entry| entry.bitrate.get()),
            hops: Some(route.hops),
            rtt_millis: None,
        },
        None => PathCapabilities::default(),
    }
}

pub(crate) async fn connect_identified(
    owner: &HostClient,
    scope: Option<&HostServiceScope>,
    local_identity: IdentityHash,
    identity: IdentityHash,
    destination: DestinationHash,
    limits: CarrierLimits,
    path: &'static str,
) -> Result<Connection, NetworkError> {
    let initial_path = path_capabilities(owner, destination, limits).await?;
    let connect_timeout = limits.connect_timeout(initial_path);
    let host = owner.clone();
    let retained_scope = scope.cloned();
    // The host owns this admitted operation through completion. If our
    // waiter is cancelled or times out during establishment, its late
    // Connection result is dropped and closes the newly established link.
    // This uses the host's existing bounded operation lane, not a service
    // task or a second runtime.
    let operation = protocol_operation(owner, scope, move |protocol| async move {
        tokio::time::timeout(connect_timeout, async {
            if initial_path.hops.is_none() {
                protocol
                    .request_path(personal_rns::wire::DestinationHash::new(
                        destination.into_bytes(),
                    ))
                    .await
                    .map_err(NetworkError::Path)?;
            }
            // Announce-derived keys must match the provisioned endpoint binding.
            let key = host
                .destination_public_key(destination)
                .await
                .map_err(NetworkError::Session)?
                .ok_or(NetworkError::IdentityMismatch)?;
            if PublicIdentityMaterial::from_bytes(key)
                .identity_hash()
                .as_bytes()
                != identity.as_bytes()
            {
                return Err(NetworkError::IdentityMismatch);
            }
            Ok::<(), NetworkError>(())
        })
        .await
        .map_err(|_| NetworkError::Timeout)??;
        // Discovery cannot create a Link. Open the bounded early-close
        // publication window only immediately before dispatching establishment,
        // so unrelated closures during an offline path wait cannot exhaust it.
        // Admission is checked again here, and the reservation still precedes
        // every establishment effect and travels with the host-owned future.
        let reservation = retained_scope
            .as_ref()
            .map(HostServiceScope::reserve_link)
            .transpose()
            .map_err(NetworkError::Scope)?;
        // The node does not cancel an admitted establishment when its
        // waiter disappears. Retain this bounded host operation until the
        // node settles it so even a late successful Link gets a close guard.
        let established = protocol
            .establish_link_with_rtt(personal_rns::wire::DestinationHash::new(
                destination.into_bytes(),
            ))
            .await
            .map_err(NetworkError::Establish)?;
        let link = LinkId::new(*established.link_id.as_bytes());
        if let Some(reservation) = reservation {
            reservation.track(link);
        }
        let mut connection = Connection {
            host: host.clone(),
            scope: retained_scope,
            link: Some(link),
            limits,
            path,
            capabilities: initial_path,
            link_rtt_millis: established.rtt_millis.max(1),
            measured_rtt: AtomicU64::new(established.rtt_millis.max(1)),
        };
        // Re-read after discovery/establishment: a previously unknown route may
        // now resolve to a low-bitrate physical fleet member. The close guard
        // already exists if inspection fails or the caller was cancelled.
        connection.capabilities = path_capabilities(&host, destination, limits).await?;
        tokio::time::timeout(
            limits.request_timeout(connection.capabilities(), 512, 0),
            async {
                protocol
                    .identify(
                        established.link_id,
                        personal_rns::identity::IdentityHash::new(local_identity.into_bytes()),
                    )
                    .await
                    .map_err(NetworkError::Identify)?;
                Ok::<(), NetworkError>(())
            },
        )
        .await
        .map_err(|_| NetworkError::Timeout)??;
        Ok(connection)
    });
    tokio::time::timeout(connect_timeout, operation)
        .await
        .map_err(|_| NetworkError::Timeout)??
}

pub(crate) fn respond(
    host: &HostClient,
    scope: Option<&HostServiceScope>,
    job: &RequestJob,
    frame: &[u8],
    limits: CarrierLimits,
) -> Result<(), NetworkError> {
    let payload = copy_frame(frame, limits)?;
    let send = || {
        host.service_client()
            .map_err(NetworkError::Session)?
            .protocols()
            .respond_owned_bytes(
                personal_rns::runtime::request_endpoints::RespondToken {
                    link_id: personal_rns::routing::links::LinkId::new(job.link.into_bytes()),
                    request_id: personal_rns::routing::links::request::RequestId(
                        job.request.into_bytes(),
                    ),
                    rtt: personal_rns::units::RttMillis::new(job.rtt_millis),
                },
                payload,
            )
            .ok_or(NetworkError::Closed)?;
        Ok(())
    };
    if let Some(scope) = scope {
        scope.with_admission(send).map_err(NetworkError::Scope)?
    } else {
        send()
    }
}

/// One carrier-owned link. Dropping it queues a close without spawning a task;
/// dropping or closing it cannot stop the shared node.
pub struct Connection {
    host: HostClient,
    scope: Option<HostServiceScope>,
    link: Option<LinkId>,
    limits: CarrierLimits,
    path: &'static str,
    capabilities: PathCapabilities,
    // Reticulum's LinkDefault continues using the established Link RTT even
    // when later application exchanges measure a shorter round trip.
    link_rtt_millis: u64,
    measured_rtt: AtomicU64,
}

impl Connection {
    /// Local route hints plus the latest measured end-to-end response RTT.
    #[must_use]
    pub fn capabilities(&self) -> PathCapabilities {
        PathCapabilities {
            rtt_millis: Some(self.measured_rtt.load(Ordering::Relaxed)),
            ..self.capabilities
        }
    }

    pub async fn request(&self, frame: &[u8]) -> Result<Vec<u8>, NetworkError> {
        self.request_bounded(frame, self.limits.max_frame_bytes)
            .await
    }

    /// Bound both response allocation and transfer deadline using a known
    /// protocol response ceiling (for example an inventory-advertised envelope).
    pub async fn request_bounded(
        &self,
        frame: &[u8],
        response_bytes: usize,
    ) -> Result<Vec<u8>, NetworkError> {
        let link_id = self.link.ok_or(NetworkError::Closed)?;
        if response_bytes == 0 || response_bytes > self.limits.max_frame_bytes {
            return Err(NetworkError::InvalidLimits);
        }
        let payload = pack_frame(frame, self.limits)?;
        // The host bounds encoded response data before removing the bin wrapper.
        let maximum_response_bytes = packed_binary_len(response_bytes)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(NetworkError::InvalidLimits)?;
        let timeout = self.limits.request_timeout(
            deadline_path(self.capabilities(), self.link_rtt_millis),
            payload.len(),
            response_bytes,
        );
        let path = self.path;
        let request = protocol_operation(
            &self.host,
            self.scope.as_ref(),
            move |protocol| async move {
                let (bytes, rtt) = protocol
                    .request_with_options(
                        personal_rns::routing::links::LinkId::new(link_id.into_bytes()),
                        EngineRequestPathHash::of(path),
                        &payload,
                        personal_rns::runtime::RequestOptions {
                            // Keep Reticulum's RTT/progress-aware response and
                            // Resource state machines. The outer application bound
                            // limits the complete exchange, including transfer time.
                            response_timeout:
                                personal_rns::engine::RequestResponseTimeout::LinkDefault,
                            maximum_response_bytes: personal_rns::units::ByteLimit::Maximum(
                                maximum_response_bytes,
                            ),
                        },
                    )
                    .await
                    .map_err(NetworkError::Request)?;
                let data = personal_rns::routing::links::request::parse_packed_binary(&bytes)
                    .map_err(|_| NetworkError::UnexpectedOutcome)?;
                if data.len() > response_bytes {
                    return Err(NetworkError::FrameTooLarge);
                }
                Ok((data.to_vec(), rtt.millis()))
            },
        );
        let (bytes, rtt) = tokio::time::timeout(timeout, request)
            .await
            .map_err(|_| NetworkError::Timeout)???;
        self.measured_rtt.store(rtt.max(1), Ordering::Relaxed);
        Ok(bytes)
    }

    /// Explicitly queue link closure; Reticulum owns completion of link teardown.
    pub async fn close(mut self) -> Result<(), NetworkError> {
        let Some(link_id) = self.link else {
            return Ok(());
        };
        if self
            .host
            .service_client()
            .map_err(NetworkError::Session)?
            .protocols()
            .close_link(personal_rns::routing::links::LinkId::new(
                link_id.into_bytes(),
            ))
        {
            self.link = None;
            Ok(())
        } else {
            Err(NetworkError::Closed)
        }
    }
}

fn deadline_path(path: PathCapabilities, link_rtt_millis: u64) -> PathCapabilities {
    PathCapabilities {
        rtt_millis: Some(path.rtt_millis.unwrap_or(0).max(link_rtt_millis)),
        ..path
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(link_id) = self.link.take() {
            if let Ok(services) = self.host.service_client() {
                let _queued =
                    services
                        .protocols()
                        .close_link(personal_rns::routing::links::LinkId::new(
                            link_id.into_bytes(),
                        ));
            }
        }
    }
}

/// A SINGLE destination using the existing host identity and fixed request path.
/// The allow list starts empty; `Network::authorize_peers` fills it explicitly.
pub fn destination_config(limits: Limits) -> Result<SingleDestinationConfig, NetworkError> {
    validate_limits(limits)?;
    let name = DestinationName::try_new(APP_NAME, ASPECTS.iter().map(|value| (*value).into()))
        .map_err(|_| NetworkError::InvalidPeer)?;
    let mut config = SingleDestinationConfig::new(name, DestinationIdentityConfig::HostIdentity);
    config.maximum_request_bytes = Some(request_ceiling(limits.max_frame_bytes)? as u64);
    config.resource_strategy = ResourceStrategy::Refuse;
    config.request_handlers.push(RequestHandlerConfig {
        path: REQUEST_PATH.into(),
        policy: RequestPolicy::AllowList,
    });
    Ok(config)
}

pub(crate) fn validate_limits(limits: Limits) -> Result<(), NetworkError> {
    if limits.max_frame_bytes < 134
        || limits.max_page_items == 0
        || limits.max_page_items > limits.max_frame_bytes.saturating_sub(57) / 77
        || limits.max_peers == 0
        || limits.inbound_jobs == 0
        || limits.inbound_jobs > tokio::sync::Semaphore::MAX_PERMITS
        || limits.exchange_timeout.as_millis() == 0
        || limits.exchange_timeout.as_millis() > u128::from(u64::MAX)
        || std::time::Instant::now()
            .checked_add(limits.exchange_timeout)
            .is_none()
        || request_ceiling(limits.max_frame_bytes)? > MAX_EFFICIENT_SIZE
    {
        return Err(NetworkError::InvalidLimits);
    }
    Ok(())
}

pub(crate) fn request_ceiling(max_frame_bytes: usize) -> Result<usize, NetworkError> {
    packed_binary_len(max_frame_bytes)
        .and_then(|value| value.checked_add(REQUEST_WIRE_OVERHEAD))
        .ok_or(NetworkError::InvalidLimits)
}

pub(crate) fn request_path() -> RequestPathHash {
    RequestPathHash::new(*EngineRequestPathHash::of(REQUEST_PATH).as_bytes())
}

fn pack_frame(frame: &[u8], limits: CarrierLimits) -> Result<Vec<u8>, NetworkError> {
    if frame.len() > limits.max_frame_bytes {
        return Err(NetworkError::FrameTooLarge);
    }
    let length = packed_binary_len(frame.len()).ok_or(NetworkError::FrameTooLarge)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(length)
        .map_err(|_| NetworkError::Allocation)?;
    let mut header = [0_u8; MAX_PACKED_BINARY_HEADER_LEN];
    let header_len = write_packed_binary_header(frame.len(), &mut header)
        .map_err(|_| NetworkError::FrameTooLarge)?;
    payload.extend_from_slice(&header[..header_len]);
    payload.extend_from_slice(frame);
    Ok(payload)
}

fn copy_frame(frame: &[u8], limits: CarrierLimits) -> Result<Vec<u8>, NetworkError> {
    if frame.len() > limits.max_frame_bytes {
        return Err(NetworkError::FrameTooLarge);
    }
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(frame.len())
        .map_err(|_| NetworkError::Allocation)?;
    payload.extend_from_slice(frame);
    Ok(payload)
}

async fn execute(
    host: &HostClient,
    scope: Option<&HostServiceScope>,
    limits: Limits,
    command: HostCommand,
) -> Result<CommandOutcome, NetworkError> {
    let operation = async {
        match scope {
            Some(scope) => scope.execute(command).await.map_err(NetworkError::Scope),
            None => host.execute(command).await.map_err(NetworkError::Session),
        }
    };
    tokio::time::timeout(limits.exchange_timeout, operation)
        .await
        .map_err(|_| NetworkError::Timeout)??
        .map_err(NetworkError::Command)
}

async fn protocol_operation<T, F, Fut>(
    host: &HostClient,
    scope: Option<&HostServiceScope>,
    operation: F,
) -> Result<T, NetworkError>
where
    T: Send + 'static,
    F: FnOnce(personal_rns::PrnsNodeHandle) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
{
    match scope {
        Some(scope) => scope
            .protocol_operation(operation)
            .await
            .map_err(NetworkError::Scope),
        None => host
            .protocol_operation(operation)
            .await
            .map_err(NetworkError::Session),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use personal_rns::routing::links::request::parse_packed_binary;
    use std::time::Duration;

    fn limits() -> Limits {
        Limits {
            max_frame_bytes: 4096,
            max_page_items: 8,
            max_peers: 8,
            inbound_jobs: 2,
            exchange_timeout: Duration::from_secs(3),
        }
    }

    #[test]
    fn binary_request_framing_preserves_large_opaque_mls_bytes() {
        let frame: Vec<u8> = (0_u8..=255).cycle().take(2048).collect();
        let packed = pack_frame(&frame, limits().into()).unwrap();
        assert_eq!(packed.len(), frame.len() + 3);
        assert_eq!(parse_packed_binary(&packed).unwrap(), frame);
        assert_eq!(
            pack_frame(&vec![0; 4097], limits().into()),
            Err(NetworkError::FrameTooLarge)
        );
    }

    #[test]
    fn destination_admits_only_bounded_identified_group_requests() {
        let config = destination_config(limits()).unwrap();
        assert_eq!(config.name.app_name(), APP_NAME);
        assert_eq!(config.name.aspects(), ASPECTS);
        assert_eq!(config.maximum_request_bytes, Some(4096 + 3 + 28));
        assert_eq!(config.resource_strategy, ResourceStrategy::Refuse);
        assert_eq!(config.request_handlers.len(), 1);
        assert_eq!(config.request_handlers[0].path, REQUEST_PATH);
        assert_eq!(config.request_handlers[0].policy, RequestPolicy::AllowList);
    }

    #[test]
    fn invalid_limits_are_rejected_before_host_or_queue_allocation() {
        let mut invalid = limits();
        invalid.max_frame_bytes = 133;
        assert_eq!(validate_limits(invalid), Err(NetworkError::InvalidLimits));
        invalid = limits();
        invalid.max_page_items = 53;
        assert_eq!(validate_limits(invalid), Err(NetworkError::InvalidLimits));
        invalid = limits();
        invalid.max_frame_bytes = MAX_EFFICIENT_SIZE;
        assert_eq!(validate_limits(invalid), Err(NetworkError::InvalidLimits));
        invalid = limits();
        invalid.inbound_jobs = usize::MAX;
        assert_eq!(validate_limits(invalid), Err(NetworkError::InvalidLimits));
        invalid = limits();
        invalid.exchange_timeout = Duration::ZERO;
        assert_eq!(validate_limits(invalid), Err(NetworkError::InvalidLimits));
    }

    #[test]
    fn route_capabilities_use_only_the_selected_physical_interface() {
        use personal_rns::interfaces::{
            BitrateBps, ConnectionState, EgressCapability, IngressCapability,
            InterfaceCapabilities, InterfaceId, TransportCapability,
        };
        use personal_rns::node_introspection::{InterfaceTimingSnapshot, RouteSnapshot};
        use personal_rns::routing::routes::{NextHop, RouteRetention};
        use personal_rns::units::InstantMillis;

        let radio = InterfaceId::new([1; 8]);
        let fast = InterfaceId::new([2; 8]);
        let route = RouteSnapshot {
            destination: personal_rns::wire::DestinationHash::new([3; 16]),
            hops: 3,
            via: NextHop::Direct,
            learned_at: InstantMillis(1),
            last_route_activity_at: InstantMillis(1),
            expires_at: InstantMillis(10_000),
            interface: radio,
            retention: RouteRetention::Network,
        };
        let entry = |id, bps| InterfaceTimingSnapshot {
            id,
            bitrate: BitrateBps::guess(bps),
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            connection: ConnectionState::Connected,
        };
        let timing = [entry(fast, 65_000_000), entry(radio, 300)];
        let path = capabilities_for_route(Some(&route), &timing);
        assert_eq!(path.local_bitrate_bps, Some(300));
        assert_eq!(path.hops, Some(3));
        assert_eq!(path.rtt_millis, None);
        // Another attached fast interface is never substituted for missing
        // selected-interface evidence; no route also stays unknown.
        assert_eq!(
            capabilities_for_route(Some(&route), &timing[..1]).local_bitrate_bps,
            None
        );
        assert_eq!(
            capabilities_for_route(None, &timing),
            PathCapabilities::default()
        );
        let mut disabled = timing;
        disabled[1].connection = ConnectionState::Disabled;
        assert_eq!(
            capabilities_for_route(Some(&route), &disabled).local_bitrate_bps,
            None
        );
        disabled[1].connection = ConnectionState::Connected;
        disabled[1].capabilities.egress = EgressCapability::Disabled;
        assert_eq!(
            capabilities_for_route(Some(&route), &disabled).local_bitrate_bps,
            None
        );
    }

    #[test]
    fn transfer_size_and_slow_path_extend_wait_but_never_the_configured_cap() {
        let carrier: CarrierLimits = limits().into();
        let path = PathCapabilities {
            local_bitrate_bps: Some(300),
            hops: Some(1),
            rtt_millis: Some(1_200),
        };
        let inventory = carrier.request_timeout(path, 90, 365);
        let envelope = carrier.request_timeout(path, 69, 16_384);
        assert!(inventory > Duration::from_secs(10));
        assert!(envelope > inventory);
        let capped = CarrierLimits {
            policy: SyncPolicy {
                max_exchange_millis: 60_000,
                ..carrier.policy
            },
            ..carrier
        };
        assert_eq!(
            capped.request_timeout(path, usize::MAX, usize::MAX),
            Duration::from_secs(60)
        );
        let fast = PathCapabilities {
            local_bitrate_bps: Some(65_000_000),
            hops: Some(3),
            rtt_millis: Some(1),
        };
        assert!(carrier.request_timeout(fast, 69, 16_384) > Duration::from_secs(60));
    }

    #[test]
    fn fast_path_keeps_reticulum_response_grace_unless_the_caller_sets_a_tighter_cap() {
        let carrier: CarrierLimits = limits().into();
        let path = PathCapabilities {
            local_bitrate_bps: Some(65_000_000),
            hops: Some(1),
            rtt_millis: Some(1),
        };
        let native = personal_rns::routing::links::data::link_traffic_timeout_ms(
            personal_rns::units::RttMillis::new(1),
        ) + personal_rns::routing::links::request::REQUEST_RESPONSE_GRACE_MS;
        assert!(carrier.policy.exchange_deadline_millis(path, 1, 1) < native);
        assert_eq!(
            carrier.request_timeout(path, 1, 1),
            Duration::from_millis(native)
        );
        let capped = CarrierLimits {
            policy: SyncPolicy {
                min_exchange_millis: 100,
                max_exchange_millis: 100,
                unknown_rtt_millis: 10,
                ..carrier.policy
            },
            ..carrier
        };
        assert_eq!(
            capped.request_timeout(path, 1, 1),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn faster_responses_do_not_reduce_the_established_links_native_deadline_floor() {
        let carrier: CarrierLimits = limits().into();
        let measured = PathCapabilities {
            local_bitrate_bps: Some(1_000_000),
            hops: Some(1),
            rtt_millis: Some(10),
        };
        let established = 8_000;
        let native = personal_rns::routing::links::data::link_traffic_timeout_ms(
            personal_rns::units::RttMillis::new(established),
        ) + personal_rns::routing::links::request::REQUEST_RESPONSE_GRACE_MS;
        assert!(carrier.request_timeout(measured, 1, 1) < Duration::from_millis(native));
        let budget_path = deadline_path(measured, established);
        assert_eq!(budget_path.rtt_millis, Some(established));
        assert!(carrier.request_timeout(budget_path, 1, 1) >= Duration::from_millis(native));
        // Scheduling still gets the latest application observation. A later
        // slower response increases the transfer budget above the Link floor.
        assert_eq!(measured.rtt_millis, Some(10));
        assert_eq!(
            deadline_path(
                PathCapabilities {
                    rtt_millis: Some(20_000),
                    ..measured
                },
                established
            )
            .rtt_millis,
            Some(20_000)
        );
    }
}
