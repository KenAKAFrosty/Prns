mod command;
mod control;
mod error;
mod reply;
mod session;
mod value;

const MAX_SAM_LINE_BYTES: u64 = 16 * 1024;

pub use command::{SamCommand, SamSessionDestination};
pub use control::SamControl;
pub use error::{SamProtocolError, SamStreamError};
pub use reply::{SamRejection, SamReply, SamReplyKind, SamSessionReplyDestination, SamVersion};
pub use session::{
    generate_destination, resolve_destination, I2pAcceptedStream, I2pGeneratedDestination,
    SamSession,
};
pub use value::{
    I2pAddress, I2pBase32Address, I2pDestinationKind, I2pPrivateDestination, I2pPublicDestination,
    SamSessionId, SamValueError, I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES,
};

#[cfg(test)]
mod tests;
