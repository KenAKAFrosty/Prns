use super::{accepted_reply, private, unexpected, SamExchange};
use crate::interfaces::i2p::sam::{
    I2pAddress, I2pPublicDestination, SamCommand, SamProtocolError, SamReply, SamReplyKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveName {
    name: I2pAddress,
}

impl ResolveName {
    pub fn new(name: I2pAddress) -> Self {
        Self { name }
    }
}

impl private::Sealed for ResolveName {}

impl SamExchange for ResolveName {
    type Output = I2pPublicDestination;

    fn command(&self) -> SamCommand {
        SamCommand::NamingLookup {
            name: self.name.clone(),
        }
    }

    fn conclude(self, reply: SamReply) -> Result<Self::Output, SamProtocolError> {
        match accepted_reply(SamReplyKind::Naming, reply)? {
            SamReply::NameResolved { destination } => Ok(destination),
            reply => Err(unexpected(SamReplyKind::Naming, reply)),
        }
    }
}
