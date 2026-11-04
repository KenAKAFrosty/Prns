use prns_host::CommandFailure;
#[cfg(feature = "network")]
use prns_host_runtime::{owner::SessionError, service_scope::HostServiceScopeError};

/// Carrier failures preserve the distinction between unavailable transport and
/// invalid local configuration. None of these failures establish group delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    InvalidLimits,
    InvalidPeer,
    UnknownPeer,
    Allocation,
    FrameTooLarge,
    Timeout,
    IdentityMismatch,
    Closed,
    UnexpectedOutcome,
    #[cfg(feature = "network")]
    Path(personal_rns::runtime::RequestPathError),
    #[cfg(feature = "network")]
    Establish(personal_rns::SendError<personal_rns::engine::EstablishLinkFailure>),
    #[cfg(feature = "network")]
    Identify(personal_rns::SendError<personal_rns::engine::IdentifyFailure>),
    #[cfg(feature = "network")]
    Request(personal_rns::SendError<personal_rns::engine::SendRequestFailure>),
    #[cfg(feature = "network")]
    Session(SessionError),
    #[cfg(feature = "network")]
    Scope(HostServiceScopeError),
    Command(CommandFailure),
}

impl core::fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "group transport: {self:?}")
    }
}

impl std::error::Error for NetworkError {}
