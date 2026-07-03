use heapless::Vec as HeaplessVec;

use crate::identity::IdentityHash;
use crate::routing::links::data::LinkDataError;
use crate::routing::links::establish::WriteEstablishLinkError;
use crate::routing::links::LinkId;
use crate::wire::DestinationHash;

use super::{Delivered, EngineCommand, Settleable, Settlement};

/// RNS 1.3.1 `Link(destination)`: bring a session up with a peer whose
/// announce we hold. Settles established when the LRPROOF validates, or fails
/// on rejection, a write error, or the establishment timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstablishLink {
    pub destination: DestinationHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishLinkError {
    NoRouteToDestination,
    NotDirectlyReachable,
}

pub const MAX_SEND_LINK_PLAINTEXT_LEN: usize = 431;

pub type SendLinkPayload = HeaplessVec<u8, MAX_SEND_LINK_PLAINTEXT_LEN>;

/// RNS 1.3.1 `Link.identify`: reveal a held identity to the responder over the
/// encrypted link — initiator-only, shown to the peer and no one else, and
/// fire-and-forget (the reference neither proves nor acknowledges one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identify {
    pub link_id: LinkId,
    pub identity: IdentityHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyError {
    NoSuchLink,
    LinkNotActive,
    NotInitiator,
    IdentityNotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyFailure {
    Rejected(IdentifyError),
    WriteFailed,
}

/// One data packet sealed under an ACTIVE link's session key, fired on the
/// interface the link rides — RNS 1.3.1 `Packet(link, data).send()` with its
/// `PacketReceipt`. Settles Delivered when the responder's proof validates,
/// or Timeout at the link's traffic deadline — never at emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendLink {
    pub link_id: LinkId,
    pub payload: SendLinkPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLinkError {
    NoSuchLink,
    LinkNotActive,
}

/// RNS 1.3.1 `Link.teardown`: close an ACTIVE link deliberately, telling the
/// peer with the sealed LINKCLOSE and purging the session key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseLink {
    pub link_id: LinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseLinkError {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkEstablished {
    pub link_id: LinkId,
    pub rtt_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishLinkFailure {
    Rejected(EstablishLinkError),
    WriteFailed(WriteEstablishLinkError),
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLinkFailure {
    Rejected(SendLinkError),
    WriteFailed(LinkDataError),
    Culled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseLinkFailure {
    Rejected(CloseLinkError),
    WriteFailed,
}

impl Settleable for EstablishLink {
    type Success = LinkEstablished;
    type Failure = EstablishLinkFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::EstablishLink(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<LinkEstablished, EstablishLinkFailure>> {
        match settlement {
            Settlement::EstablishLink(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}

impl Settleable for SendLink {
    type Success = Delivered;
    type Failure = SendLinkFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendLink(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<Delivered, SendLinkFailure>> {
        match settlement {
            Settlement::SendLink(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}

impl Settleable for Identify {
    type Success = ();
    type Failure = IdentifyFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::Identify(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), IdentifyFailure>> {
        match settlement {
            Settlement::Identify(result) => Some(result),
            Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}

impl Settleable for CloseLink {
    type Success = ();
    type Failure = CloseLinkFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::CloseLink(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), CloseLinkFailure>> {
        match settlement {
            Settlement::CloseLink(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn establish_link_recovers_its_typed_settlement() {
        let verb = EstablishLink {
            destination: DestinationHash::new([0x11; 16]),
        };

        assert_eq!(verb.into_command(), EngineCommand::EstablishLink(verb));
        assert_eq!(
            EstablishLink::from_settlement(Settlement::EstablishLink(Ok(LinkEstablished {
                link_id: LinkId::new([0x22; 16]),
                rtt_ms: 250,
            }))),
            Some(Ok(LinkEstablished {
                link_id: LinkId::new([0x22; 16]),
                rtt_ms: 250,
            })),
        );
        assert_eq!(
            EstablishLink::from_settlement(Settlement::SendGroup(Ok(()))),
            None,
            "an establishment never reads another verb's settlement",
        );
    }
}
