mod control;
mod error;
mod session;

const MAX_SAM_LINE_BYTES: u64 = 16 * 1024;

pub use control::SamControl;
pub use error::{SamControlError, SamStreamError};
pub use prns_core::interfaces::i2p::sam::{
    I2pAddress, I2pBase32Address, I2pDestinationKind, I2pGeneratedDestination,
    I2pPrivateDestination, I2pPublicDestination, SamCommand, SamProtocolError, SamRejection,
    SamReply, SamReplyKind, SamSessionDestination, SamSessionId, SamSessionReplyDestination,
    SamValueError, SamVersion, I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES,
};
pub use session::{generate_destination, resolve_destination, I2pAcceptedStream, SamSession};

#[cfg(test)]
mod tests;
