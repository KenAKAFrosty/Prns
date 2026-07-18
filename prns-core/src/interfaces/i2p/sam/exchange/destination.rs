use super::{accepted_reply, private, unexpected, SamExchange};
use crate::interfaces::i2p::sam::{
    I2pPrivateDestination, I2pPublicDestination, SamCommand, SamProtocolError, SamReply,
    SamReplyKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateDestination;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pGeneratedDestination {
    pub public: Option<I2pPublicDestination>,
    pub private: I2pPrivateDestination,
}

impl private::Sealed for GenerateDestination {}

impl SamExchange for GenerateDestination {
    type Output = I2pGeneratedDestination;

    fn command(&self) -> SamCommand {
        SamCommand::DestinationGenerate
    }

    fn conclude(self, reply: SamReply) -> Result<Self::Output, SamProtocolError> {
        match accepted_reply(SamReplyKind::Destination, reply)? {
            SamReply::DestinationGenerated { public, private } => {
                Ok(I2pGeneratedDestination { public, private })
            }
            reply => Err(unexpected(SamReplyKind::Destination, reply)),
        }
    }
}
