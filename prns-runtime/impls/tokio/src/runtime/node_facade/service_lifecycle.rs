//! Native service registration and two-phase scoped retirement on a live node.
//! Fence service network producers and settle their admitted operations before
//! preparing a pause. This primitive does not turn an unrestricted node handle
//! into an authorization gate.
//!
//! Register with `initially_accepting: false`, install callbacks, obtain durable
//! lifecycle permission, then activate ingress. Before pause, fence producers and
//! wait for admitted operations. Keep receive callbacks alive until prepare returns;
//! native callbacks run synchronously before that acknowledgement. Then drain any
//! application work they accepted, save the ratchet snapshot durably, wait for the
//! finite interface-buffer fence, and finish. No whole-node shutdown is involved.
//!
//! A pause revokes engine authority for late cryptographic continuations. Worker
//! jobs can retain cloned secrets until they complete; this is not a secure-memory
//! erasure guarantee. Active ratchet rotation still requires normal host persistence
//! independently of this pause snapshot. Shared global engine capacities apply to
//! inbound Links; this API does not provide a per-service inbound Link quota.
use std::sync::Arc;

use crate::crypto::ratchets::RatchetPolicy;
use crate::identity::vault::IdentitySecretKey;
use crate::identity::Zeroizing;
use crate::manifold::driver::FrameDrainFence;
use crate::manifold::driver::HostCommand;
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use crate::wire::DestinationHash;
use tokio::sync::oneshot;

use super::PrnsNodeHandle;

/// These native request buffers are bounded and checked before dispatch. The
/// generic identity state machine remains allocation-free.
#[derive(Clone)]
pub struct ServiceRegistration {
    /// False stages all ingress and announces until activate_ingress succeeds.
    pub initially_accepting: bool,
    pub secret: IdentitySecretKey,
    pub app_name: String,
    pub aspects: Vec<String>,
    pub app_data: Vec<u8>,
    pub proof_strategy: ProofStrategy,
    pub link_request_policy: LinkRequestPolicy,
    pub ratchet_policy: RatchetPolicy,
    pub resource_strategy: ResourceStrategy,
    pub maximum_request_bytes: crate::units::ByteLimit,
    pub request_handlers: Vec<(RequestPathHash, RequestPolicy)>,
    pub restore_ratchets: Option<Zeroizing<Vec<u8>>>,
}

impl ServiceRegistration {
    /// Check the canonical staging limits before retaining resources. Register
    /// checks these same bounds again before admitting the host command.
    pub fn validate_bounds(&self) -> Result<(), ServiceLifecycleError> {
        if self.app_name.len() > 64
            || self.aspects.len() > 8
            || self.aspects.iter().any(|part| part.len() > 64)
            || self.app_data.len() > 500
            || self.request_handlers.len() > 32
            || self
                .restore_ratchets
                .as_ref()
                .is_some_and(|bytes| bytes.len() > 32 * 1024)
        {
            return Err(ServiceLifecycleError::Bounds);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceLifecycleError {
    NodeStopped,
    Bounds,
    Conflict,
    WrongPhase,
    /// No registration landed, or this endpoint already completed retirement.
    NotRegistered,
    InvalidRatchets,
    Capacity,
    RetirementIncomplete,
    NotDrained,
}
impl std::fmt::Display for ServiceLifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "service lifecycle: {self:?}")
    }
}
impl std::error::Error for ServiceLifecycleError {}

/// A retryable retirement result. Ratchet bytes contain secrets and must be
/// durably saved under this destination before `finish_pause`. Fence completion
/// proves release of pre-barrier interface buffers, not physical transmission,
/// delivery, or recall from a radio/controller that already accepted a frame.
#[derive(Clone)]
pub struct ServicePause {
    pub(crate) endpoint: u64,
    pub(crate) token: Arc<()>,
    pub(crate) destination: DestinationHash,
    pub(crate) ratchets: Option<Zeroizing<Vec<u8>>>,
    pub(crate) fences: Vec<FrameDrainFence>,
}
impl ServicePause {
    pub fn destination(&self) -> DestinationHash {
        self.destination
    }
    pub fn ratchets(&self) -> Option<&[u8]> {
        self.ratchets.as_deref().map(|bytes| bytes.as_slice())
    }
    pub fn is_drained(&self) -> bool {
        self.fences.iter().all(FrameDrainFence::is_complete)
    }
}

/// Retain this token before awaiting registration. Replays address the same
/// registration even when the command reply was lost. Dropping it is not cleanup.
#[derive(Clone)]
pub struct ServiceEndpoint {
    pub(crate) handle: PrnsNodeHandle,
    pub(crate) id: u64,
}
impl PrnsNodeHandle {
    pub fn service_endpoint(&self) -> ServiceEndpoint {
        ServiceEndpoint {
            handle: self.clone(),
            id: self.mint().0,
        }
    }
}
impl ServiceEndpoint {
    pub async fn register(
        &self,
        registration: ServiceRegistration,
    ) -> Result<DestinationHash, ServiceLifecycleError> {
        registration.validate_bounds()?;
        let (reply, receive) = oneshot::channel();
        self.handle
            .commands
            .send(HostCommand::ServiceLifecycle(ServiceCommand::Register {
                endpoint: self.id,
                registration: Box::new(registration),
                reply,
            }))
            .map_err(|_| ServiceLifecycleError::NodeStopped)?;
        receive
            .await
            .map_err(|_| ServiceLifecycleError::NodeStopped)?
    }

    /// Open staged ingress after durable lifecycle permission and callback readiness.
    pub async fn activate_ingress(&self) -> Result<(), ServiceLifecycleError> {
        let (reply, receive) = oneshot::channel();
        self.handle
            .commands
            .send(HostCommand::ServiceLifecycle(
                ServiceCommand::ActivateIngress {
                    endpoint: self.id,
                    reply,
                },
            ))
            .map_err(|_| ServiceLifecycleError::NodeStopped)?;
        receive
            .await
            .map_err(|_| ServiceLifecycleError::NodeStopped)?
    }

    /// The caller must first fence new operations and settle every outgoing Link
    /// establishment, then supply the exact owned outgoing Link set. Shared or
    /// foreign Link ids must never be supplied. Responders are derived by the node.
    pub async fn prepare_pause(
        &self,
        outgoing_links: Vec<LinkId>,
    ) -> Result<ServicePause, ServiceLifecycleError> {
        if outgoing_links.len() > 256 {
            return Err(ServiceLifecycleError::Bounds);
        }
        let (reply, receive) = oneshot::channel();
        self.handle
            .commands
            .send(HostCommand::ServiceLifecycle(
                ServiceCommand::PreparePause {
                    endpoint: self.id,
                    outgoing_links,
                    reply,
                },
            ))
            .map_err(|_| ServiceLifecycleError::NodeStopped)?;
        receive
            .await
            .map_err(|_| ServiceLifecycleError::NodeStopped)?
    }

    /// Acknowledge durable storage of `pause.ratchets()` after its buffer fence
    /// completes. This releases only this endpoint's retained key ownership.
    /// On failure, retain the endpoint and receipt and retry; do not resume work.
    pub async fn finish_pause(&self, pause: &ServicePause) -> Result<(), ServiceLifecycleError> {
        if pause.endpoint != self.id {
            return Err(ServiceLifecycleError::Conflict);
        }
        let (reply, receive) = oneshot::channel();
        self.handle
            .commands
            .send(HostCommand::ServiceLifecycle(ServiceCommand::FinishPause {
                endpoint: self.id,
                token: pause.token.clone(),
                reply,
            }))
            .map_err(|_| ServiceLifecycleError::NodeStopped)?;
        receive
            .await
            .map_err(|_| ServiceLifecycleError::NodeStopped)?
    }
}

#[doc(hidden)]
pub enum ServiceCommand {
    ActivateIngress {
        endpoint: u64,
        reply: oneshot::Sender<Result<(), ServiceLifecycleError>>,
    },
    Register {
        endpoint: u64,
        registration: Box<ServiceRegistration>,
        reply: oneshot::Sender<Result<DestinationHash, ServiceLifecycleError>>,
    },
    PreparePause {
        endpoint: u64,
        outgoing_links: Vec<LinkId>,
        reply: oneshot::Sender<Result<ServicePause, ServiceLifecycleError>>,
    },
    FinishPause {
        endpoint: u64,
        token: Arc<()>,
        reply: oneshot::Sender<Result<(), ServiceLifecycleError>>,
    },
}
