//! Explicit, bounded metadata diagnostics. Never format an input or an error
//! value: those may contain application data, paths or cryptographic material.
use prns_groups_reticulum::NetworkError;
use prns_runtime::engine::{
    EstablishLinkFailure, EstablishLinkRejection, SendRequestFailure, SendResourceFailure,
};
use prns_runtime::routing::links::establish::WriteEstablishLinkRejection;
use prns_runtime::routing::links::resources::ResourceFailureCause;
use prns_runtime::runtime::SendError;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    OnceLock,
};

mod attempt;
mod delivery;
mod schedule;
pub(crate) use attempt::{local_send_result, AttemptTrace};
pub(crate) use delivery::{receipt_transition, DeliveryTrace};
pub use schedule::metadata_schedule_snapshot;
pub(crate) use schedule::{recovery, schedule, Decision};

const TARGET: &str = "prns.groups.diagnostics";
const PER_EVENT_LIMIT: u32 = 64;
static ENABLED: OnceLock<bool> = OnceLock::new();
static EXPLICIT_OPT_IN: AtomicBool = AtomicBool::new(false);
static COUNTS: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static EXHAUSTED: [AtomicBool; 8] = [const { AtomicBool::new(false) }; 8];

fn enabled() -> bool {
    EXPLICIT_OPT_IN.load(Ordering::Relaxed)
        || *ENABLED.get_or_init(|| std::env::var("PRNS_GROUP_DIAGNOSTICS").as_deref() == Ok("1"))
}

/// Enable bounded, metadata-only diagnostic producers for this process.
///
/// Native embeddings without process environment configuration can opt in before
/// starting their runner. The embedding must also install a logger accepting the
/// exact `prns.groups.diagnostics` and `prns.groups.delivery` targets. Existing
/// event budgets are never reset, and no payloads or raw identities are emitted.
/// Restart the process without opting in to return to the default disabled state.
pub fn enable_metadata_diagnostics() {
    EXPLICIT_OPT_IN.store(true, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Event {
    EnrollmentAttempt,
    EnrollmentReceived,
    EnrollmentHandled,
    EnrollmentResponse,
    EnrollmentOutcome,
    SyncReceived,
    SyncServed,
    SyncOutcome,
}
impl Event {
    fn name(self) -> &'static str {
        match self {
            Self::EnrollmentAttempt => "enrollment_attempt",
            Self::EnrollmentReceived => "enrollment_received",
            Self::EnrollmentHandled => "enrollment_handled",
            Self::EnrollmentResponse => "enrollment_response",
            Self::EnrollmentOutcome => "enrollment_outcome",
            Self::SyncReceived => "sync_received",
            Self::SyncServed => "sync_served",
            Self::SyncOutcome => "sync_outcome",
        }
    }
}

pub(crate) fn emit(event: Event, category: &'static str, frame_bytes: usize) {
    if !enabled() || !log::log_enabled!(target: TARGET, log::Level::Info) {
        return;
    }
    if !take_budget(&COUNTS[event as usize]) {
        if !EXHAUSTED[event as usize].swap(true, Ordering::Relaxed) {
            log::info!(target: TARGET, "stage=diagnostics category=event_limit event={} limit={} dropped_at_least=1", event.name(), PER_EVENT_LIMIT);
        }
        return;
    }
    log::info!(target: TARGET, "stage={} category={} frame_bytes={}", event.name(), category, frame_bytes);
}

fn take_budget(counter: &AtomicU32) -> bool {
    {
        let mut previous = counter.load(Ordering::Relaxed);
        loop {
            let Some(next) = (previous < PER_EVENT_LIMIT).then(|| previous + 1) else {
                break Err(previous);
            };
            match counter.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(previous) => break Ok(previous),
                Err(current) => previous = current,
            }
        }
    }
    .is_ok()
}

pub(crate) fn error_category(error: &crate::Error) -> &'static str {
    use crate::Error;
    match error {
        Error::Api(error) => match error {
            crate::ApiError::Busy => "store_busy",
            crate::ApiError::Unavailable => "unavailable",
            crate::ApiError::StorageFull => "storage",
            crate::ApiError::Clock => "clock",
            crate::ApiError::Corrupt => "corrupt",
            crate::ApiError::Retry { .. } => "retry",
            crate::ApiError::Rejected { .. } => "rejected",
            crate::ApiError::InvalidInput
            | crate::ApiError::NotProvisioned
            | crate::ApiError::Configuration => "configuration",
        },
        Error::Transport(error) => match error {
            prns_groups_reticulum::Error::Network(error) => network_category(error),
            prns_groups_reticulum::Error::Service(_) => "transport_service",
            prns_groups_reticulum::Error::Protocol(_) => "transport_protocol",
            prns_groups_reticulum::Error::Remote(_) => "remote_rejection",
            _ => "transport_configuration",
        },
        Error::Enrollment(error) => {
            use crate::enrollment::EnrollmentError;
            match error {
                EnrollmentError::MissingCatalog => "missing_catalog",
                EnrollmentError::AlreadyInitialized => "already_initialized",
                EnrollmentError::Clock => "clock",
                EnrollmentError::Invalid => "enrollment_invalid",
                EnrollmentError::Authentication => "enrollment_authentication",
                EnrollmentError::Expired => "enrollment_expired",
                EnrollmentError::Capacity => "enrollment_capacity",
                EnrollmentError::Missing => "enrollment_missing",
                EnrollmentError::Conflict => "enrollment_conflict",
                EnrollmentError::RequestConflict => "enrollment_request_conflict",
                EnrollmentError::NotOwner => "enrollment_not_owner",
                EnrollmentError::NotJoined => "enrollment_not_joined",
                EnrollmentError::Corrupt => "enrollment_corrupt",
                EnrollmentError::Entropy => "enrollment_entropy",
                EnrollmentError::Crypto => "enrollment_crypto",
                EnrollmentError::Storage(_) | EnrollmentError::Database(_) => "enrollment_storage",
                EnrollmentError::Retry(_) => "enrollment_retry",
                EnrollmentError::AlreadyJoined => "already_joined",
                EnrollmentError::GroupClosed => "group_closed",
                EnrollmentError::OwnerMustClose => "owner_must_close",
                EnrollmentError::MemberNotFound => "member_not_found",
                EnrollmentError::StaleMembership => "stale_membership",
                EnrollmentError::UnsupportedVersion => "unsupported_version",
                EnrollmentError::UnsupportedProfile => "unsupported_profile",
            }
        }
        Error::Busy => "store_busy",
        Error::Unavailable => "unavailable",
        Error::Storage(_) | Error::StorageFull => "storage",
        Error::Service(_) => "service",
        Error::Clock => "clock",
        Error::Corrupt => "corrupt",
        Error::Retry(_) => "retry",
        Error::NotProvisioned
        | Error::NoRecipients
        | Error::InvalidInput
        | Error::IdempotencyConflict
        | Error::Configuration => "configuration",
    }
}

pub(crate) fn network_category(error: &NetworkError) -> &'static str {
    match error {
        NetworkError::Request(error) => match error {
            SendError::PayloadTooLarge => "request_payload_too_large",
            SendError::NodeStopped => "request_node_stopped",
            SendError::Busy => "request_busy",
            SendError::Failed(failure) => match failure {
                SendRequestFailure::Rejected(_) => "request_rejected",
                SendRequestFailure::WriteFailed => "request_write_failed",
                SendRequestFailure::Culled => "request_culled",
                SendRequestFailure::Timeout => "request_timeout",
                SendRequestFailure::LinkClosed => "request_link_closed",
                SendRequestFailure::ResponseTooLarge => "response_too_large",
                SendRequestFailure::ResourceCapacity => "resource_capacity",
                SendRequestFailure::RequestTransferFailed(cause) => match cause {
                    SendResourceFailure::Rejected(_) => "request_resource_rejected",
                    SendResourceFailure::WriteFailed => "request_resource_write_failed",
                    SendResourceFailure::RejectedByPeer => "request_resource_rejected_by_peer",
                    SendResourceFailure::Sequencing => "request_resource_sequencing",
                    SendResourceFailure::Timeout => "request_resource_timeout",
                    SendResourceFailure::LinkClosed => "request_resource_link_closed",
                    SendResourceFailure::PredecessorFailed => "request_resource_predecessor_failed",
                },
                SendRequestFailure::ResponseTransferFailed(cause) => match cause {
                    ResourceFailureCause::CancelledBySender => "response_resource_cancelled",
                    ResourceFailureCause::RefusedHashmapUpdate(_) => "response_resource_hashmap",
                    ResourceFailureCause::RetriesExhausted => "response_resource_retries_exhausted",
                    ResourceFailureCause::LinkVanished => "response_resource_link_vanished",
                    ResourceFailureCause::TransferUnopenable => "response_resource_unopenable",
                    ResourceFailureCause::TransferCorrupt => "response_resource_corrupt",
                    ResourceFailureCause::ProofUnsendable => "response_resource_proof_unsendable",
                    ResourceFailureCause::DecompressionFailed => {
                        "response_resource_decompression_failed"
                    }
                    ResourceFailureCause::DecompressionTimedOut => {
                        "response_resource_decompression_timeout"
                    }
                    ResourceFailureCause::OpenTimedOut => "response_resource_open_timeout",
                    ResourceFailureCause::MetadataOverrun => "response_resource_metadata_overrun",
                },
            },
        },
        NetworkError::Timeout => "exchange_timeout",
        NetworkError::Path(_) => "path_failed",
        NetworkError::Establish(error) => match error {
            SendError::PayloadTooLarge => "link_payload_too_large",
            SendError::NodeStopped => "link_node_stopped",
            SendError::Busy => "link_busy",
            SendError::Failed(failure) => match failure {
                EstablishLinkFailure::Timeout => "link_timeout",
                EstablishLinkFailure::Rejected(EstablishLinkRejection::NoRouteToDestination) => {
                    "link_no_route"
                }
                EstablishLinkFailure::Rejected(EstablishLinkRejection::NotDirectlyReachable) => {
                    "link_not_directly_reachable"
                }
                EstablishLinkFailure::WriteFailed(cause) => match cause {
                    WriteEstablishLinkRejection::RouteVanished => "link_route_vanished",
                    WriteEstablishLinkRejection::Serialize => "link_serialize",
                    WriteEstablishLinkRejection::LinkTableFull => "link_capacity",
                    WriteEstablishLinkRejection::DuplicateLinkId => "link_duplicate_id",
                },
            },
        },
        NetworkError::Identify(_) => "link_identification_failed",
        NetworkError::Session(_) => "host_session_failed",
        NetworkError::Scope(_) => "service_scope_unavailable",
        NetworkError::Command(_) => "host_command_failed",
        NetworkError::IdentityMismatch => "identity_mismatch",
        NetworkError::Closed => "closed",
        NetworkError::Allocation => "allocation",
        NetworkError::FrameTooLarge => "frame_too_large",
        NetworkError::UnexpectedOutcome => "unexpected_response",
        NetworkError::InvalidLimits | NetworkError::InvalidPeer | NetworkError::UnknownPeer => {
            "network_configuration"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_budget_is_finite_and_cannot_wrap() {
        let count = AtomicU32::new(0);
        for _ in 0..PER_EVENT_LIMIT {
            assert!(take_budget(&count));
        }
        assert!(!take_budget(&count));
        assert_eq!(count.load(Ordering::Relaxed), PER_EVENT_LIMIT);
        count.store(u32::MAX, Ordering::Relaxed);
        assert!(!take_budget(&count));
    }
    #[test]
    fn request_and_response_failures_have_static_distinct_categories() {
        let request = NetworkError::Request(SendError::Failed(
            SendRequestFailure::RequestTransferFailed(SendResourceFailure::Timeout),
        ));
        let response = NetworkError::Request(SendError::Failed(
            SendRequestFailure::ResponseTransferFailed(ResourceFailureCause::RetriesExhausted),
        ));
        assert_eq!(network_category(&request), "request_resource_timeout");
        assert_eq!(
            network_category(&response),
            "response_resource_retries_exhausted"
        );
        assert_eq!(
            error_category(&crate::Error::Enrollment(
                crate::enrollment::EnrollmentError::Authentication
            )),
            "enrollment_authentication"
        );
    }

    #[test]
    fn link_timeout_route_and_local_admission_have_distinct_categories() {
        let cases = [
            (SendError::Busy, "link_busy"),
            (SendError::NodeStopped, "link_node_stopped"),
            (
                SendError::Failed(EstablishLinkFailure::Timeout),
                "link_timeout",
            ),
            (
                SendError::Failed(EstablishLinkFailure::Rejected(
                    EstablishLinkRejection::NoRouteToDestination,
                )),
                "link_no_route",
            ),
            (
                SendError::Failed(EstablishLinkFailure::WriteFailed(
                    WriteEstablishLinkRejection::LinkTableFull,
                )),
                "link_capacity",
            ),
        ];
        for (failure, category) in cases {
            assert_eq!(
                network_category(&NetworkError::Establish(failure)),
                category
            );
        }
    }
}
