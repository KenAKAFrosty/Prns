//! Optional per-phase discovery evidence. Types are always available so runtimes can
//! exhaustively project the journal independently of dependency feature unification.
//! `path-diagnostics` gates emission; the core stores no event history and does no IO.
use crate::engine::{CommandId, InstantMillis};
use crate::interfaces::{InterfaceId, InterfaceMode};
use crate::routing::ingress::IgnoreReason;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathDiscoveryEvent {
    /// Core monotonic observation time, when the entry point has one. Signing
    /// completion has no clock; its sink must timestamp observation instead.
    pub at: Option<InstantMillis>,
    pub destination: Option<DestinationHash>,
    pub interface: Option<InterfaceId>,
    /// Public wire request nonce, present only while handling that request.
    pub request_tag: Option<[u8; 16]>,
    pub phase: PathDiscoveryPhase,
}

#[cfg(feature = "path-diagnostics")]
impl PathDiscoveryEvent {
    pub(crate) const fn new(
        at: Option<InstantMillis>,
        destination: Option<DestinationHash>,
        interface: Option<InterfaceId>,
        phase: PathDiscoveryPhase,
    ) -> Self {
        Self {
            at,
            destination,
            interface,
            request_tag: None,
            phase,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDiscoveryPhase {
    LocalRequest {
        command: CommandId,
    },
    RequestIngress {
        disposition: PathRequestDisposition,
        mode: Option<InterfaceMode>,
        recursive: bool,
        cached_interface: Option<InterfaceId>,
    },
    RequestEgress(PathRequestEgress),
    ResponseIngress(PathResponseDisposition),
    /// Core issued a response directive; carrier enqueue and physical transmission
    /// remain runtime evidence, not a successful delivery claim.
    ResponseDispatched,
    LocalTimeout {
        command: CommandId,
    },
    RecursiveTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRequestDisposition {
    LocalResponse,
    CachedResponse,
    ResponseQueueFull,
    RecursiveForward,
    BoundaryForward,
    LocalClientForward,
    OfferedToLocalClients,
    Ignored(IgnoreReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRequestEgress {
    /// The runtime invoked SendIfOnline's callback, or received an unconditional
    /// Send directive. This does not establish actual medium transmission.
    Dispatched,
    Offline,
    RateLimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathResponseDisposition {
    /// Cryptographic verification was delegated; this is not yet trusted evidence.
    VerificationPending,
    Accepted,
    /// Verified response matched the retained route without renewing its age.
    RetainedRouteMatched,
    Ignored,
    Blackholed,
}

impl PathDiscoveryPhase {
    pub const fn name(self) -> &'static str {
        match self {
            Self::LocalRequest { .. } => "local_request",
            Self::RequestIngress { .. } => "request_ingress",
            Self::RequestEgress(_) => "request_egress",
            Self::ResponseIngress(_) => "response_ingress",
            Self::ResponseDispatched => "response_dispatched",
            Self::LocalTimeout { .. } => "local_timeout",
            Self::RecursiveTimeout => "recursive_timeout",
        }
    }
}
