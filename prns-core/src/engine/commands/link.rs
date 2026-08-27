use heapless::Vec as HeaplessVec;

use crate::identity::IdentityHash;
use crate::routing::links::data::{link_mdu, LinkDataError};
use crate::routing::links::establish::WriteEstablishLinkRejection;
use crate::routing::links::LinkId;
use crate::wire::DestinationHash;

use super::{PacketReceiptDelivered, PrnsCommand, Settleable, Settlement};

/// RNS 1.4.2 `Link(destination)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstablishLink {
    pub destination: DestinationHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishLinkRejection {
    NoRouteToDestination,
    NotDirectlyReachable,
}

pub const MAX_SEND_TO_LINK_PLAINTEXT_LEN: usize = link_mdu(crate::wire::BROADCAST_MTU);

pub type SendToLinkPayload = HeaplessVec<u8, MAX_SEND_TO_LINK_PLAINTEXT_LEN>;

/// RNS 1.4.2 `Link.identify`, initiator-only.
/// Fire-and-forget in the reference: the peer validates it and fires its callback but sends nothing back, no proof and no ack, so there is no delivery confirmation to await and success settles at write time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identify {
    pub link_id: LinkId,
    pub identity: IdentityHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyRejection {
    NoSuchLink,
    LinkNotActive,
    NotInitiator,
    IdentityNotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyFailure {
    Rejected(IdentifyRejection),
    WriteFailed,
}

/// RNS 1.4.2 `Packet(link, data).send()`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendToLink {
    pub link_id: LinkId,
    pub payload: SendToLinkPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendToLinkRejection {
    NoSuchLink,
    LinkNotActive,
}

/// RNS 1.4.2 `Link.teardown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseLink {
    pub link_id: LinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseLinkRejection {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkEstablished {
    pub link_id: LinkId,
    pub rtt_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishLinkFailure {
    Rejected(EstablishLinkRejection),
    WriteFailed(WriteEstablishLinkRejection),
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendToLinkFailure {
    Rejected(SendToLinkRejection),
    WriteFailed(LinkDataError),
    Culled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseLinkFailure {
    Rejected(CloseLinkRejection),
    WriteFailed,
}

impl Settleable for EstablishLink {
    type Success = LinkEstablished;
    type Failure = EstablishLinkFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::EstablishLink(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<LinkEstablished, EstablishLinkFailure>> {
        match settlement {
            Settlement::EstablishLink(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::SendToLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}

impl Settleable for SendToLink {
    type Success = PacketReceiptDelivered;
    type Failure = SendToLinkFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SendToLink(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<PacketReceiptDelivered, SendToLinkFailure>> {
        match settlement {
            Settlement::SendToLink(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}

impl Settleable for Identify {
    type Success = ();
    type Failure = IdentifyFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::Identify(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), IdentifyFailure>> {
        match settlement {
            Settlement::Identify(result) => Some(result),

            Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}

impl Settleable for CloseLink {
    type Success = ();
    type Failure = CloseLinkFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::CloseLink(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), CloseLinkFailure>> {
        match settlement {
            Settlement::CloseLink(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendPlainPacket(_) => None,
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

        assert_eq!(verb.into_command(), PrnsCommand::EstablishLink(verb));
        assert_eq!(
            EstablishLink::from_settlement(Settlement::EstablishLink(Ok(LinkEstablished {
                link_id: LinkId::new([0x22; 16]),
                rtt_millis: 250,
            }))),
            Some(Ok(LinkEstablished {
                link_id: LinkId::new([0x22; 16]),
                rtt_millis: 250,
            })),
        );
        assert_eq!(
            EstablishLink::from_settlement(Settlement::SendGroup(Ok(()))),
            None,
            "an establishment never reads another verb's settlement",
        );
    }
}
