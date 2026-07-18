mod destination;
mod hello;
mod naming;
mod session;
mod stream;

use super::{SamCommand, SamProtocolError, SamReply, SamReplyKind};

pub use destination::{GenerateDestination, I2pGeneratedDestination};
pub use hello::{SamHello, SamV3_1};
pub use naming::ResolveName;
pub use session::{CreateSession, EstablishedSession};
pub use stream::{parse_incoming_peer_destination, AcceptStream, ConnectStream, SamStreamReady};

pub trait SamExchange: private::Sealed {
    type Output;

    fn command(&self) -> SamCommand;
    fn conclude(self, reply: SamReply) -> Result<Self::Output, SamProtocolError>;
}

fn accepted_reply(expected: SamReplyKind, reply: SamReply) -> Result<SamReply, SamProtocolError> {
    if reply.kind() != expected {
        return Err(unexpected(expected, reply));
    }
    if let SamReply::Rejected {
        kind,
        rejection,
        message,
    } = reply
    {
        return Err(SamProtocolError::Rejected {
            kind,
            rejection,
            message,
        });
    }
    Ok(reply)
}

fn unexpected(expected: SamReplyKind, reply: SamReply) -> SamProtocolError {
    SamProtocolError::UnexpectedReply {
        expected,
        actual: reply.kind(),
    }
}

mod private {
    pub trait Sealed {}
}
