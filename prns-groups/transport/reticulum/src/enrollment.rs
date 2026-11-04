//! Identified bootstrap exchanges before an endpoint is an enrolled group peer.
//!
//! This carrier authenticates the remote Reticulum identity, not an invitation,
//! MLS credential, group authority or payload. The caller owns those checks and
//! must durably commit enrollment before acknowledging it. All operations borrow
//! the existing host; no node, executor, identity or background task is created.

use personal_rns::routing::links::resources::MAX_EFFICIENT_SIZE;
use personal_rns::routing::request_handlers::RequestPathHash as EngineRequestPathHash;
use personal_rns::runtime::PrnsEvent;
use prns_groups_core::sync::{PathCapabilities, SyncPolicy};
use prns_host::{
    DestinationHash, DestinationIdentityConfig, DestinationName, IdentityHash,
    RequestHandlerConfig, RequestPathHash, RequestPolicy, ResourceStrategy,
    SingleDestinationConfig,
};
use prns_host_runtime::owner::HostClient;
use prns_host_runtime::service_scope::HostServiceScope;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::network::{connect_identified, request_ceiling, respond, CarrierLimits};
pub use crate::{IngressOutcome, NetworkError, RequestJob};

pub use crate::{enrollment_destination_for as destination_for, ENROLLMENT_ASPECTS as ASPECTS};
pub const REQUEST_PATH: &str = "/prns/groups/enrollment/v3";
/// Default application-frame budget, including its enrollment header. Explicit
/// Limits can select another budget within Reticulum's efficient request size.
/// The binary wrapper and request header are separately included in the limit.
pub const MAX_FRAME_BYTES: usize = 131_072;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_frame_bytes: usize,
    pub inbound_jobs: usize,
    /// Local command timeout and minimum network budget; `SyncPolicy` supplies
    /// the bounded capability/size-aware network deadlines.
    pub exchange_timeout: Duration,
}
impl Limits {
    /// Complete application enrollment frame supported after validating the
    /// enclosing Reticulum binary/request framing and runtime queue limits.
    pub fn max_application_frame_bytes(self) -> Result<usize, NetworkError> {
        self.validate()?;
        Ok(self.max_frame_bytes)
    }

    fn validate(self) -> Result<(), NetworkError> {
        if self.max_frame_bytes == 0
            || self.inbound_jobs == 0
            || self.inbound_jobs > tokio::sync::Semaphore::MAX_PERMITS
            || self.exchange_timeout.as_millis() == 0
            || self.exchange_timeout.as_millis() > u128::from(u64::MAX)
            || std::time::Instant::now()
                .checked_add(self.exchange_timeout)
                .is_none()
            || request_ceiling(self.max_frame_bytes)? > MAX_EFFICIENT_SIZE
        {
            return Err(NetworkError::InvalidLimits);
        }
        Ok(())
    }
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

/// A separate SINGLE destination keeps bootstrap traffic out of group inventory.
/// The node handler allows requests before enrollment, while `Ingress` requires
/// Link identification before copying them. The application must validate every
/// invitation against `RequestJob::requester()` before releasing private data.
pub fn destination_config(limits: Limits) -> Result<SingleDestinationConfig, NetworkError> {
    limits.validate()?;
    let name = DestinationName::try_new(
        crate::APP_NAME,
        ASPECTS.iter().map(|aspect| (*aspect).into()),
    )
    .map_err(|_| NetworkError::InvalidPeer)?;
    let mut config = SingleDestinationConfig::new(name, DestinationIdentityConfig::HostIdentity);
    config.maximum_request_bytes = Some(request_ceiling(limits.max_frame_bytes)? as u64);
    config.resource_strategy = ResourceStrategy::Refuse;
    config.request_handlers.push(RequestHandlerConfig {
        path: REQUEST_PATH.into(),
        policy: RequestPolicy::AllowAll,
    });
    Ok(config)
}

/// Separate bounded queue; unidentified and oversized input never reaches it.
#[derive(Clone)]
pub struct Ingress(crate::Ingress);
impl Ingress {
    pub fn new(
        destination: DestinationHash,
        limits: Limits,
    ) -> Result<(Self, mpsc::Receiver<RequestJob>), NetworkError> {
        limits.validate()?;
        let (ingress, receiver) = crate::Ingress::for_path(
            destination,
            request_path(),
            limits.max_frame_bytes,
            limits.inbound_jobs,
        );
        Ok((Self(ingress), receiver))
    }
    pub fn on_event(&self, event: PrnsEvent<'_>) -> IngressOutcome {
        self.0.on_event(event)
    }
    pub fn on_event_ref(&self, event: &PrnsEvent<'_>) -> IngressOutcome {
        self.0.on_event_ref(event)
    }
}

/// Identity-authenticated exchanges independent of a trusted MLS peer mapping.
pub struct Network {
    host: HostClient,
    scope: Option<HostServiceScope>,
    destination: DestinationHash,
    local_identity: IdentityHash,
    limits: Limits,
    policy: SyncPolicy,
}
impl Network {
    pub fn new(
        host: HostClient,
        destination: DestinationHash,
        limits: Limits,
    ) -> Result<Self, NetworkError> {
        limits.validate()?;
        let local_identity = host.identity_hash().map_err(NetworkError::Session)?;
        if destination != destination_for(local_identity)?
            || !host
                .destination_hashes()
                .map_err(NetworkError::Session)?
                .contains(&destination)
        {
            return Err(NetworkError::IdentityMismatch);
        }
        Ok(Self {
            host,
            scope: None,
            destination,
            local_identity,
            limits,
            policy: SyncPolicy::default(),
        })
    }

    /// Identity-explicit carrier on a dynamically registered shared host.
    pub fn scoped(
        scope: HostServiceScope,
        destination: DestinationHash,
        limits: Limits,
    ) -> Result<Self, NetworkError> {
        limits.validate()?;
        scope.check_open().map_err(NetworkError::Scope)?;
        let local_identity = scope.identity_hash();
        if destination != destination_for(local_identity)?
            || !scope.contains_destination(destination)
        {
            return Err(NetworkError::IdentityMismatch);
        }
        Ok(Self {
            host: scope.host().clone(),
            scope: Some(scope),
            destination,
            local_identity,
            limits,
            policy: SyncPolicy::default(),
        })
    }

    pub fn with_sync_policy(mut self, policy: SyncPolicy) -> Result<Self, NetworkError> {
        if !policy.is_valid() {
            return Err(NetworkError::InvalidLimits);
        }
        self.policy = policy;
        Ok(self)
    }

    pub const fn sync_policy(&self) -> SyncPolicy {
        self.policy
    }

    fn carrier_limits(&self) -> CarrierLimits {
        CarrierLimits {
            policy: self.policy,
            ..self.limits.into()
        }
    }

    pub async fn path_capabilities(
        &self,
        identity: IdentityHash,
    ) -> Result<PathCapabilities, NetworkError> {
        crate::network::path_capabilities(
            &self.host,
            destination_for(identity)?,
            self.carrier_limits(),
        )
        .await
    }

    pub const fn local_identity(&self) -> IdentityHash {
        self.local_identity
    }

    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// The expected identity determines the destination and must match its
    /// verified announce key. Dropping/timing out this exchange closes its Link;
    /// a late establishment remains host-owned until it can also be closed.
    pub async fn exchange(
        &self,
        identity: IdentityHash,
        frame: &[u8],
    ) -> Result<Vec<u8>, NetworkError> {
        self.exchange_with_capabilities(identity, frame)
            .await
            .map(|(bytes, _)| bytes)
    }

    /// Preserve the measured path sample so a caller can schedule subsequent
    /// retries and announcements without assuming that the first hop is the
    /// complete path.
    pub async fn exchange_with_capabilities(
        &self,
        identity: IdentityHash,
        frame: &[u8],
    ) -> Result<(Vec<u8>, PathCapabilities), NetworkError> {
        if frame.len() > self.limits.max_frame_bytes {
            return Err(NetworkError::FrameTooLarge);
        }
        if identity == self.local_identity {
            return Err(NetworkError::InvalidPeer);
        }
        let destination = destination_for(identity)?;
        let connection = connect_identified(
            &self.host,
            self.scope.as_ref(),
            self.local_identity,
            identity,
            destination,
            self.carrier_limits(),
            REQUEST_PATH,
        )
        .await?;
        let result = connection.request(frame).await;
        let path = connection.capabilities();
        let _closed = connection.close().await;
        result.map(|bytes| (bytes, path))
    }

    /// Reply only after the caller has authorized this identified requester and
    /// durably completed its operation. Queueing a reply does not prove receipt.
    pub async fn respond(&self, job: &RequestJob, frame: &[u8]) -> Result<(), NetworkError> {
        if job.destination != self.destination || job.path != request_path() {
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
}

fn request_path() -> RequestPathHash {
    RequestPathHash::new(*EngineRequestPathHash::of(REQUEST_PATH).as_bytes())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use personal_rns::engine::InstantMillis;
    use personal_rns::identity::IdentityHash as EngineIdentityHash;
    use personal_rns::routing::links::request::{
        write_packed_binary_header, RequestId, MAX_PACKED_BINARY_HEADER_LEN,
    };
    use personal_rns::routing::links::LinkId;
    use personal_rns::runtime::Message;
    use personal_rns::units::RttMillis;

    fn limits() -> Limits {
        Limits {
            max_frame_bytes: MAX_FRAME_BYTES,
            inbound_jobs: 1,
            exchange_timeout: Duration::from_secs(2),
        }
    }

    fn request<'a>(data: &'a [u8], identified: bool, path: &str) -> PrnsEvent<'a> {
        PrnsEvent::Message(Message::Request {
            destination: personal_rns::wire::DestinationHash::new([1; 16]),
            link_id: LinkId::new([2; 16]),
            request_id: RequestId([3; 16]),
            requester: identified.then_some(EngineIdentityHash::new([4; 16])),
            path_hash: EngineRequestPathHash::of(path),
            requested_at: InstantMillis(0),
            rtt: RttMillis::new(7),
            data,
        })
    }

    fn packed(frame: &[u8]) -> Vec<u8> {
        let mut header = [0; MAX_PACKED_BINARY_HEADER_LEN];
        let len = write_packed_binary_header(frame.len(), &mut header).unwrap();
        let mut result = header[..len].to_vec();
        result.extend_from_slice(frame);
        result
    }

    #[test]
    fn bootstrap_destination_is_separate_and_request_overhead_is_bounded() {
        let identity = IdentityHash::new([1; 16]);
        assert_ne!(
            destination_for(identity).unwrap(),
            crate::destination_for(identity).unwrap()
        );
        let config = destination_config(limits()).unwrap();
        assert_eq!(config.name.app_name(), crate::APP_NAME);
        assert_eq!(config.name.aspects(), ASPECTS);
        assert_eq!(
            config.maximum_request_bytes,
            Some((MAX_FRAME_BYTES + 5 + 28) as u64)
        );
        assert_eq!(config.resource_strategy, ResourceStrategy::Refuse);
        assert_eq!(config.request_handlers.len(), 1);
        assert_eq!(config.request_handlers[0].path, REQUEST_PATH);
        assert_eq!(config.request_handlers[0].policy, RequestPolicy::AllowAll);
    }

    #[test]
    fn invalid_limits_fail_before_queue_allocation() {
        for max_frame_bytes in [0, MAX_EFFICIENT_SIZE, usize::MAX] {
            assert_eq!(
                Limits {
                    max_frame_bytes,
                    ..limits()
                }
                .max_application_frame_bytes(),
                Err(NetworkError::InvalidLimits)
            );
            assert!(matches!(
                Ingress::new(
                    DestinationHash::new([1; 16]),
                    Limits {
                        max_frame_bytes,
                        ..limits()
                    }
                ),
                Err(NetworkError::InvalidLimits)
            ));
        }
        assert!(Limits {
            max_frame_bytes: MAX_FRAME_BYTES * 4,
            ..limits()
        }
        .validate()
        .is_ok());
        assert_eq!(limits().max_application_frame_bytes(), Ok(MAX_FRAME_BYTES));
        for inbound_jobs in [0, usize::MAX] {
            assert_eq!(
                Limits {
                    inbound_jobs,
                    ..limits()
                }
                .validate(),
                Err(NetworkError::InvalidLimits)
            );
        }
        assert_eq!(
            Limits {
                exchange_timeout: Duration::ZERO,
                ..limits()
            }
            .validate(),
            Err(NetworkError::InvalidLimits)
        );
    }

    #[test]
    fn identity_comes_only_from_link_and_payload_is_not_debugged() {
        let (ingress, mut jobs) = Ingress::new(DestinationHash::new([1; 16]), limits()).unwrap();
        let frame = packed(b"identity=mallory;invitation=private-token");
        assert_eq!(
            ingress.on_event(request(&frame, false, REQUEST_PATH)),
            IngressOutcome::Rejected
        );
        assert_eq!(
            ingress.on_event(request(&frame, true, crate::REQUEST_PATH)),
            IngressOutcome::Ignored
        );
        assert_eq!(
            ingress.on_event(request(&frame, true, REQUEST_PATH)),
            IngressOutcome::Queued
        );
        assert_eq!(
            ingress.on_event(request(&frame, true, REQUEST_PATH)),
            IngressOutcome::Full
        );
        let job = jobs.try_recv().unwrap();
        assert_eq!(job.requester(), IdentityHash::new([4; 16]));
        assert_eq!(job.data(), b"identity=mallory;invitation=private-token");
        assert!(!format!("{job:?}").contains("private-token"));
        drop(jobs);
        assert_eq!(
            ingress.on_event(request(&frame, true, REQUEST_PATH)),
            IngressOutcome::Rejected
        );
    }

    #[test]
    fn exact_frame_cap_includes_all_application_bytes() {
        let (ingress, mut jobs) = Ingress::new(DestinationHash::new([1; 16]), limits()).unwrap();
        let frame = packed(&vec![9; MAX_FRAME_BYTES]);
        assert_eq!(
            ingress.on_event(request(&frame, true, REQUEST_PATH)),
            IngressOutcome::Queued
        );
        assert_eq!(jobs.try_recv().unwrap().data().len(), MAX_FRAME_BYTES);
        let oversized = packed(&vec![9; MAX_FRAME_BYTES + 1]);
        assert_eq!(
            ingress.on_event(request(&oversized, true, REQUEST_PATH)),
            IngressOutcome::Rejected
        );
        let mut trailing = frame;
        trailing.push(0);
        assert_eq!(
            ingress.on_event(request(&trailing, true, REQUEST_PATH)),
            IngressOutcome::Rejected
        );
        assert!(jobs.try_recv().is_err());
    }
}
