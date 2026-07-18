mod bridge;
mod destination;
mod runtime;
mod session_id;
mod transport;

pub mod sam;

pub use sam::I2pBase32Address;

pub use bridge::{
    SamBridgeAddress, SamBridgeAddressError, SamBridgeError, TokioSamBridge, TokioSamSession,
};
pub use destination::{
    load_destination, persist_destination, I2pDestinationKeyPath, I2pDestinationKeyPathError,
    I2pDestinationStorageError,
};
pub use runtime::{
    DuplicateI2pPeer, I2pInterface, I2pInterfaceName, I2pInterfaceNameError, I2pInterfaceStatus,
    I2pPeerAddress, I2pPeerAddressError, I2pPeers, I2pReachability, I2pRetryPolicy,
    I2pRetryPolicyError, I2pRuntimeConfig, I2pRuntimeIssue,
};
pub use session_id::{generate_session_id, I2pSessionIdError};
pub use transport::{SamBridgeTransport, SamFailureClass, SamSessionTransport, SamTransportError};

#[cfg(test)]
mod bridge_tests;
