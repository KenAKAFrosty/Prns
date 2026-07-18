use super::{accepted_reply, private, unexpected, SamExchange};
use crate::interfaces::i2p::sam::{
    I2pPublicDestination, SamCommand, SamProtocolError, SamReply, SamReplyKind, SamSessionId,
};

pub fn parse_incoming_peer_destination(
    line: &str,
) -> Result<I2pPublicDestination, SamProtocolError> {
    if !line.starts_with("STREAM STATUS ") {
        return I2pPublicDestination::new(line.trim_end_matches(['\r', '\n']))
            .map_err(SamProtocolError::InvalidPeerDestination);
    }
    match super::super::parse_reply(line)? {
        SamReply::Rejected {
            kind,
            rejection,
            message,
        } => Err(SamProtocolError::Rejected {
            kind,
            rejection,
            message,
        }),
        reply => Err(SamProtocolError::UnexpectedPeerIdentification(reply.kind())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectStream {
    id: SamSessionId,
    destination: I2pPublicDestination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamStreamReady;

impl ConnectStream {
    pub fn new(id: SamSessionId, destination: I2pPublicDestination) -> Self {
        Self { id, destination }
    }
}

impl private::Sealed for ConnectStream {}

impl SamExchange for ConnectStream {
    type Output = SamStreamReady;

    fn command(&self) -> SamCommand {
        SamCommand::StreamConnect {
            id: self.id.clone(),
            destination: self.destination.clone(),
        }
    }

    fn conclude(self, reply: SamReply) -> Result<Self::Output, SamProtocolError> {
        conclude_stream(reply)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptStream {
    id: SamSessionId,
}

impl AcceptStream {
    pub fn new(id: SamSessionId) -> Self {
        Self { id }
    }
}

impl private::Sealed for AcceptStream {}

impl SamExchange for AcceptStream {
    type Output = SamStreamReady;

    fn command(&self) -> SamCommand {
        SamCommand::StreamAccept {
            id: self.id.clone(),
        }
    }

    fn conclude(self, reply: SamReply) -> Result<Self::Output, SamProtocolError> {
        conclude_stream(reply)
    }
}

fn conclude_stream(reply: SamReply) -> Result<SamStreamReady, SamProtocolError> {
    match accepted_reply(SamReplyKind::Stream, reply)? {
        SamReply::StreamReady => Ok(SamStreamReady),
        reply => Err(unexpected(SamReplyKind::Stream, reply)),
    }
}
