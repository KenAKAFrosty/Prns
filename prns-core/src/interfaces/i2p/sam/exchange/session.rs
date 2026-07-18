use super::{accepted_reply, private, unexpected, SamExchange};
use crate::interfaces::i2p::sam::{
    I2pPrivateDestination, SamCommand, SamProtocolError, SamReply, SamReplyKind,
    SamSessionDestination, SamSessionId, SamSessionReplyDestination,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSession {
    id: SamSessionId,
    destination: SamSessionDestination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstablishedSession {
    pub id: SamSessionId,
    pub private_destination: I2pPrivateDestination,
}

impl CreateSession {
    pub fn new(id: SamSessionId, destination: SamSessionDestination) -> Self {
        Self { id, destination }
    }
}

impl private::Sealed for CreateSession {}

impl SamExchange for CreateSession {
    type Output = EstablishedSession;

    fn command(&self) -> SamCommand {
        SamCommand::SessionCreate {
            id: self.id.clone(),
            destination: self.destination.clone(),
        }
    }

    fn conclude(self, reply: SamReply) -> Result<Self::Output, SamProtocolError> {
        let returned_destination = match accepted_reply(SamReplyKind::Session, reply)? {
            SamReply::SessionCreated { destination } => destination,
            reply => return Err(unexpected(SamReplyKind::Session, reply)),
        };
        let private_destination = match (self.destination, returned_destination) {
            (SamSessionDestination::Persistent(destination), _) => destination,
            (
                SamSessionDestination::Transient,
                SamSessionReplyDestination::Returned(destination),
            ) => destination,
            (SamSessionDestination::Transient, SamSessionReplyDestination::Omitted) => {
                return Err(SamProtocolError::MissingTransientSessionDestination);
            }
        };
        Ok(EstablishedSession {
            id: self.id,
            private_destination,
        })
    }
}
