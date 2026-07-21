mod bridge;
mod persistence;
mod session_id;
mod supervision;
mod transport;

pub mod sam;

pub use sam::I2pBase32Address;

pub use bridge::{
    SamBridgeAddress, SamBridgeAddressError, SamBridgeError, SamBridgeScope, TokioSamBridge,
    TokioSamSession,
};
pub use persistence::{
    load_destination, persist_destination, I2pDestinationKeyPath, I2pDestinationKeyPathError,
    I2pDestinationStorageError, RnsI2pStorage,
};
pub use session_id::{generate_session_id, I2pSessionIdError};
pub use supervision::{
    DuplicateI2pPeer, I2pInterface, I2pInterfaceConfig, I2pInterfaceIssue, I2pInterfaceName,
    I2pInterfaceNameError, I2pInterfaceStatus, I2pPeerAddress, I2pPeerAddressError, I2pPeers,
    I2pReachability, I2pRetryPolicy, I2pRetryPolicyError,
};
pub use transport::{SamBridgeTransport, SamFailureClass, SamSessionTransport, SamTransportError};

#[cfg(test)]
mod test_support;
