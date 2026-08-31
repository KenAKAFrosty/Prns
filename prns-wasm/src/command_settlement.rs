use personal_rns::engine::{
    AllowRequesterFailure, AllowRequesterRejection, AnnounceNowFailure, AnnounceNowRejection,
    CloseLinkFailure, CloseLinkRejection, CommandId, DeliveryEvidence as EngineDeliveryEvidence,
    DeliveryProof, EstablishLinkFailure, EstablishLinkRejection, IdentifyFailure,
    IdentifyRejection, RequestPathFailure, RespondFailure, RespondRejection, SendRequestFailure,
    SendRequestRejection, SendResourceFailure, SendResourceRejection, SendSinglePacketFailure,
    SendSinglePacketRejection, SendToChannelFailure, SendToChannelRejection, SendToLinkFailure,
    SendToLinkRejection, SetResourceStrategyFailure, SetResourceStrategyRejection, Settlement,
};
use personal_rns::routing::links::resources::table::ApplyHashmapUpdateError;
use personal_rns::routing::links::resources::ResourceFailureCause;
use prns_host::{CommandFailure, CommandOutcome, DeliveryEvidence, LinkId, PacketHash};

const COMMAND_SETTLEMENT_BATCH_MAGIC: u32 = 0x4353_5250;
const COMMAND_SETTLEMENT_BATCH_FORMAT_VERSION: u16 = 1;
const COMMAND_SETTLEMENT_BATCH_HEADER_BYTES: usize = 16;

pub(crate) struct CapturedCommandSettlement {
    id: CommandId,
    result: CapturedCommandResult,
}

pub(crate) enum CapturedCommandResult {
    Tracked(Result<CommandOutcome, CommandFailure>),
    Untracked,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommandSettlementBatchError {
    TooManySettlements,
    DetailTooLong,
    CapacityExceeded,
}

impl CapturedCommandSettlement {
    pub(crate) fn capture(id: CommandId, settlement: Settlement) -> Self {
        Self {
            id,
            result: project_settlement(settlement),
        }
    }

    pub(crate) const fn id(&self) -> CommandId {
        self.id
    }

    pub(crate) fn result(&self) -> &CapturedCommandResult {
        &self.result
    }

    pub(crate) fn into_result(self) -> CapturedCommandResult {
        self.result
    }
}

pub(crate) fn encode_batch(
    settlements: &[CapturedCommandSettlement],
) -> Result<Vec<u8>, CommandSettlementBatchError> {
    let count = u32::try_from(settlements.len())
        .map_err(|_| CommandSettlementBatchError::TooManySettlements)?;
    let mut writer = SettlementWriter::new()?;
    writer.u32(COMMAND_SETTLEMENT_BATCH_MAGIC)?;
    writer.u16(COMMAND_SETTLEMENT_BATCH_FORMAT_VERSION)?;
    writer.u16(0)?;
    writer.u32(prns_host::HOST_SCHEMA_VERSION)?;
    writer.u32(count)?;
    for settlement in settlements {
        writer.u64(settlement.id.0)?;
        match settlement.result() {
            CapturedCommandResult::Untracked => writer.u8(2)?,
            CapturedCommandResult::Tracked(Ok(outcome)) => {
                writer.u8(0)?;
                encode_outcome(outcome, &mut writer)?;
            }
            CapturedCommandResult::Tracked(Err(failure)) => {
                writer.u8(1)?;
                encode_failure(failure, &mut writer)?;
            }
        }
    }
    Ok(writer.finish())
}

fn project_settlement(settlement: Settlement) -> CapturedCommandResult {
    match settlement {
        Settlement::AnnounceNow(result) => CapturedCommandResult::Tracked(
            result
                .map(|()| CommandOutcome::Announced)
                .map_err(announce_failure),
        ),
        Settlement::SendSinglePacket(result) => CapturedCommandResult::Tracked(
            result.map(delivered_outcome).map_err(send_single_failure),
        ),
        Settlement::CloseLink(result) => CapturedCommandResult::Tracked(
            result
                .map(|()| CommandOutcome::LinkCloseQueued)
                .map_err(close_link_failure),
        ),
        Settlement::RequestPath(result) => CapturedCommandResult::Tracked(
            result
                .map(|path| CommandOutcome::PathDiscovered { hops: path.hops.0 })
                .map_err(request_path_failure),
        ),
        Settlement::EstablishLink(result) => CapturedCommandResult::Tracked(
            result
                .map(|link| CommandOutcome::LinkEstablished {
                    link_id: LinkId::new(*link.link_id.as_bytes()),
                    rtt_millis: link.rtt_millis,
                })
                .map_err(establish_link_failure),
        ),
        Settlement::SendToLink(result) => CapturedCommandResult::Tracked(
            result.map(delivered_outcome).map_err(send_to_link_failure),
        ),
        Settlement::Identify(result) => CapturedCommandResult::Tracked(
            result
                .map(|()| CommandOutcome::Identified)
                .map_err(identify_failure),
        ),
        Settlement::SendRequest(result) => CapturedCommandResult::Tracked(
            result.map(delivered_outcome).map_err(send_request_failure),
        ),
        Settlement::Respond(result) => CapturedCommandResult::Tracked(
            result
                .map(|()| CommandOutcome::ResponseSent { rtt_millis: 0 })
                .map_err(respond_failure),
        ),
        Settlement::SendResource(result) => CapturedCommandResult::Tracked(
            result
                .map(|()| CommandOutcome::ResourceSent)
                .map_err(resource_failure),
        ),
        Settlement::SetResourceStrategy(result) => CapturedCommandResult::Tracked(
            result
                .map(|()| CommandOutcome::ResourceStrategySet)
                .map_err(set_resource_strategy_failure),
        ),
        Settlement::SendToChannel(result) => CapturedCommandResult::Tracked(
            result
                .map(delivered_outcome)
                .map_err(send_to_channel_failure),
        ),
        Settlement::AllowRequester(result) => CapturedCommandResult::Tracked(
            result
                .map(|()| CommandOutcome::RequesterAllowed)
                .map_err(allow_requester_failure),
        ),
        Settlement::SetRegisteredAnnounceAppData(_)
        | Settlement::SendGroup(_)
        | Settlement::SendPlainPacket(_)
        | Settlement::OpenRemoteControlPairing(_)
        | Settlement::CloseRemoteControlPairing(_)
        | Settlement::ApproveRemoteControlTargetPairing(_)
        | Settlement::RejectRemoteControlTargetPairing(_)
        | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
        | Settlement::BeginRemoteControlControllerPairing(_)
        | Settlement::ApproveRemoteControlControllerPairing(_)
        | Settlement::RejectRemoteControlControllerPairing(_)
        | Settlement::RemoteControlControllerPairingRequest(_)
        | Settlement::SettleRemoteControlControllerPairingPersistence(_) => {
            CapturedCommandResult::Untracked
        }
    }
}

fn encode_outcome(
    outcome: &CommandOutcome,
    writer: &mut SettlementWriter,
) -> Result<(), CommandSettlementBatchError> {
    writer.u32(outcome.kind() as u32)?;
    match outcome {
        CommandOutcome::Announced
        | CommandOutcome::LinkCloseQueued
        | CommandOutcome::Identified
        | CommandOutcome::ResourceSent
        | CommandOutcome::ResourceStrategySet
        | CommandOutcome::RequesterAllowed => Ok(()),
        CommandOutcome::PacketDelivered {
            rtt_millis,
            evidence,
        } => {
            writer.u64(*rtt_millis)?;
            writer.u32(evidence.kind() as u32)?;
            match evidence {
                DeliveryEvidence::ExplicitProof(hash) | DeliveryEvidence::ImplicitProof(hash) => {
                    writer.u8(1)?;
                    writer.bytes(hash.as_bytes())
                }
                DeliveryEvidence::Response => writer.u8(0),
            }
        }
        CommandOutcome::InterfaceAttached { interface }
        | CommandOutcome::InterfaceDetached { interface } => writer.bytes(interface.as_bytes()),
        CommandOutcome::LinkEstablished {
            link_id,
            rtt_millis,
        } => {
            writer.bytes(link_id.as_bytes())?;
            writer.u64(*rtt_millis)
        }
        CommandOutcome::PathDiscovered { hops } => writer.u64(u64::from(*hops)),
        CommandOutcome::ResponseReceived { data, rtt_millis } => {
            writer.length_prefixed(data)?;
            writer.u64(*rtt_millis)
        }
        CommandOutcome::ResponseSent { rtt_millis } => writer.u64(*rtt_millis),
    }
}

fn encode_failure(
    failure: &CommandFailure,
    writer: &mut SettlementWriter,
) -> Result<(), CommandSettlementBatchError> {
    writer.u32(failure.kind() as u32)?;
    if let Some(detail) = failure.detail() {
        writer.length_prefixed(detail.as_bytes())?;
    }
    Ok(())
}

fn announce_failure(failure: AnnounceNowFailure) -> CommandFailure {
    match failure {
        AnnounceNowFailure::Rejected(rejection) => match rejection {
            AnnounceNowRejection::UnknownDestination => CommandFailure::UnknownDestination,
            AnnounceNowRejection::NotASingleDestination => CommandFailure::NotSingleDestination,
            AnnounceNowRejection::AppDataTooLong => CommandFailure::AnnounceAppDataTooLong,
            AnnounceNowRejection::UnknownInterface => CommandFailure::UnknownInterface,
        },
        AnnounceNowFailure::WriteFailed(error) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
    }
}

fn send_single_failure(failure: SendSinglePacketFailure) -> CommandFailure {
    match failure {
        SendSinglePacketFailure::Rejected(rejection) => match rejection {
            SendSinglePacketRejection::NoRouteToDestination => CommandFailure::NoRouteToDestination,
            SendSinglePacketRejection::NotDirectlyReachable => CommandFailure::NotDirectlyReachable,
        },
        SendSinglePacketFailure::WriteFailed(error) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
        SendSinglePacketFailure::Culled => CommandFailure::PacketCulled,
        SendSinglePacketFailure::Timeout => CommandFailure::DeliveryTimedOut,
    }
}

fn close_link_failure(failure: CloseLinkFailure) -> CommandFailure {
    match failure {
        CloseLinkFailure::Rejected(CloseLinkRejection::NoSuchLink) => CommandFailure::UnknownLink,
        CloseLinkFailure::Rejected(CloseLinkRejection::LinkNotActive) => {
            CommandFailure::LinkNotActive
        }
        CloseLinkFailure::WriteFailed => CommandFailure::WriteFailed {
            detail: "link write failed".to_string(),
        },
    }
}

fn request_path_failure(failure: RequestPathFailure) -> CommandFailure {
    match failure {
        RequestPathFailure::WriteFailed(error) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
        RequestPathFailure::Timeout => CommandFailure::DeliveryTimedOut,
        RequestPathFailure::Culled => CommandFailure::PacketCulled,
    }
}

fn establish_link_failure(failure: EstablishLinkFailure) -> CommandFailure {
    match failure {
        EstablishLinkFailure::Rejected(rejection) => match rejection {
            EstablishLinkRejection::NoRouteToDestination => CommandFailure::NoRouteToDestination,
            EstablishLinkRejection::NotDirectlyReachable => CommandFailure::NotDirectlyReachable,
        },
        EstablishLinkFailure::WriteFailed(error) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
        EstablishLinkFailure::Timeout => CommandFailure::DeliveryTimedOut,
    }
}

fn send_to_link_failure(failure: SendToLinkFailure) -> CommandFailure {
    match failure {
        SendToLinkFailure::Rejected(rejection) => match rejection {
            SendToLinkRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendToLinkRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
        SendToLinkFailure::WriteFailed(error) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
        SendToLinkFailure::Culled => CommandFailure::PacketCulled,
        SendToLinkFailure::Timeout => CommandFailure::DeliveryTimedOut,
        SendToLinkFailure::LinkClosed => CommandFailure::LinkClosed,
    }
}

fn identify_failure(failure: IdentifyFailure) -> CommandFailure {
    match failure {
        IdentifyFailure::Rejected(rejection) => match rejection {
            IdentifyRejection::NoSuchLink => CommandFailure::UnknownLink,
            IdentifyRejection::LinkNotActive => CommandFailure::LinkNotActive,
            IdentifyRejection::NotInitiator => CommandFailure::NotLinkInitiator,
            IdentifyRejection::IdentityNotHeld => CommandFailure::IdentityNotHeld,
        },
        IdentifyFailure::WriteFailed => CommandFailure::WriteFailed {
            detail: "identity write failed".to_string(),
        },
    }
}

fn send_request_failure(failure: SendRequestFailure) -> CommandFailure {
    match failure {
        SendRequestFailure::Rejected(rejection) => match rejection {
            SendRequestRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendRequestRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
        SendRequestFailure::WriteFailed => CommandFailure::WriteFailed {
            detail: "request write failed".to_string(),
        },
        SendRequestFailure::Culled => CommandFailure::PacketCulled,
        SendRequestFailure::Timeout => CommandFailure::DeliveryTimedOut,
        SendRequestFailure::LinkClosed => CommandFailure::LinkClosed,
        SendRequestFailure::ResponseTransferFailed(cause) => response_transfer_failure(cause),
        SendRequestFailure::ResponseTooLarge => CommandFailure::ResponseTooLarge,
        SendRequestFailure::ResourceCapacity => CommandFailure::ResourceTableFull,
    }
}

fn response_transfer_failure(cause: ResourceFailureCause) -> CommandFailure {
    match cause {
        ResourceFailureCause::CancelledBySender => CommandFailure::ResponseCancelledBySender,
        ResourceFailureCause::RefusedHashmapUpdate(refusal) => match refusal {
            ApplyHashmapUpdateError::BeyondPartCount => {
                CommandFailure::ResponseHashmapBeyondPartCount
            }
            ApplyHashmapUpdateError::SkipsAhead => CommandFailure::ResponseHashmapSkipsAhead,
            ApplyHashmapUpdateError::HashmapTooLong => CommandFailure::ResponseHashmapTooLong,
            ApplyHashmapUpdateError::HashmapRagged => CommandFailure::ResponseHashmapRagged,
        },
        ResourceFailureCause::RetriesExhausted => CommandFailure::ResponseRetriesExhausted,
        ResourceFailureCause::LinkVanished => CommandFailure::ResponseLinkVanished,
        ResourceFailureCause::TransferUnopenable => CommandFailure::ResponseTransferUnopenable,
        ResourceFailureCause::TransferCorrupt => CommandFailure::ResponseTransferCorrupt,
        ResourceFailureCause::ProofUnsendable => CommandFailure::ResponseProofUnsendable,
        ResourceFailureCause::DecompressionFailed => CommandFailure::ResponseDecompressionFailed,
        ResourceFailureCause::DecompressionTimedOut => {
            CommandFailure::ResponseDecompressionTimedOut
        }
        ResourceFailureCause::OpenTimedOut => CommandFailure::ResponseOpenTimedOut,
        ResourceFailureCause::MetadataOverrun => CommandFailure::ResponseMetadataOverrun,
    }
}

fn respond_failure(failure: RespondFailure) -> CommandFailure {
    match failure {
        RespondFailure::Rejected(rejection) => match rejection {
            RespondRejection::NoSuchLink => CommandFailure::UnknownLink,
            RespondRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
        RespondFailure::WriteFailed => CommandFailure::WriteFailed {
            detail: "response write failed".to_string(),
        },
        RespondFailure::Resource(failure) => resource_failure(failure),
    }
}

fn resource_failure(failure: SendResourceFailure) -> CommandFailure {
    match failure {
        SendResourceFailure::Rejected(rejection) => match rejection {
            SendResourceRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendResourceRejection::LinkNotActive => CommandFailure::LinkNotActive,
            SendResourceRejection::LinkBusy => CommandFailure::LinkBusy,
            SendResourceRejection::TableFull => CommandFailure::ResourceTableFull,
            SendResourceRejection::Build(
                personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::DataTooLarge,
            ) => CommandFailure::PayloadTooLarge,
            SendResourceRejection::Build(
                personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::MetadataTooLarge,
            )
            | SendResourceRejection::MetadataMisplaced => CommandFailure::ResourceMetadataTooLarge,
            SendResourceRejection::Build(error) => CommandFailure::WriteFailed {
                detail: format!("{error:?}"),
            },
        },
        SendResourceFailure::WriteFailed => CommandFailure::WriteFailed {
            detail: "resource write failed".to_string(),
        },
        SendResourceFailure::RejectedByPeer => CommandFailure::ResourceRejectedByPeer,
        SendResourceFailure::Sequencing => CommandFailure::ResourceSequencingFailed,
        SendResourceFailure::Timeout => CommandFailure::DeliveryTimedOut,
        SendResourceFailure::LinkClosed => CommandFailure::LinkClosed,
        SendResourceFailure::PredecessorFailed => CommandFailure::ResourcePredecessorFailed,
    }
}

fn set_resource_strategy_failure(failure: SetResourceStrategyFailure) -> CommandFailure {
    match failure {
        SetResourceStrategyFailure::Rejected(SetResourceStrategyRejection::NoSuchLink) => {
            CommandFailure::UnknownLink
        }
        SetResourceStrategyFailure::Rejected(SetResourceStrategyRejection::LinkNotActive) => {
            CommandFailure::LinkNotActive
        }
    }
}

fn send_to_channel_failure(failure: SendToChannelFailure) -> CommandFailure {
    match failure {
        SendToChannelFailure::Rejected(rejection) => match rejection {
            SendToChannelRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendToChannelRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
        SendToChannelFailure::WriteFailed(error) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
        SendToChannelFailure::WindowFull => CommandFailure::ChannelWindowFull,
        SendToChannelFailure::Untrackable => CommandFailure::ChannelUntrackable,
        SendToChannelFailure::Timeout => CommandFailure::DeliveryTimedOut,
        SendToChannelFailure::LinkClosed => CommandFailure::LinkClosed,
    }
}

fn allow_requester_failure(failure: AllowRequesterFailure) -> CommandFailure {
    match failure {
        AllowRequesterFailure::Rejected(rejection) => match rejection {
            AllowRequesterRejection::NoSuchHandler => CommandFailure::UnknownRequestHandler,
            AllowRequesterRejection::NoAllowList => CommandFailure::RequestPolicyNotAllowList,
            AllowRequesterRejection::AllowListFull => CommandFailure::RequestAllowListFull,
        },
    }
}

fn delivered_outcome(delivered: personal_rns::engine::PacketReceiptDelivered) -> CommandOutcome {
    let evidence = match delivered.evidence {
        EngineDeliveryEvidence::Proof(DeliveryProof::Explicit(hash)) => {
            DeliveryEvidence::ExplicitProof(PacketHash::new(*hash.as_bytes()))
        }
        EngineDeliveryEvidence::Proof(DeliveryProof::Implicit(hash)) => {
            DeliveryEvidence::ImplicitProof(PacketHash::new(*hash.as_bytes()))
        }
        EngineDeliveryEvidence::Response => DeliveryEvidence::Response,
    };
    CommandOutcome::PacketDelivered {
        rtt_millis: delivered.rtt.millis(),
        evidence,
    }
}

struct SettlementWriter {
    bytes: Vec<u8>,
}

impl SettlementWriter {
    fn new() -> Result<Self, CommandSettlementBatchError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve(COMMAND_SETTLEMENT_BATCH_HEADER_BYTES)
            .map_err(|_| CommandSettlementBatchError::CapacityExceeded)?;
        Ok(Self { bytes })
    }

    fn u8(&mut self, value: u8) -> Result<(), CommandSettlementBatchError> {
        self.bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), CommandSettlementBatchError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), CommandSettlementBatchError> {
        self.bytes(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), CommandSettlementBatchError> {
        self.bytes(&value.to_le_bytes())
    }

    fn length_prefixed(&mut self, value: &[u8]) -> Result<(), CommandSettlementBatchError> {
        let length =
            u32::try_from(value.len()).map_err(|_| CommandSettlementBatchError::DetailTooLong)?;
        self.u32(length)?;
        self.bytes(value)
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), CommandSettlementBatchError> {
        self.bytes
            .try_reserve(value.len())
            .map_err(|_| CommandSettlementBatchError::CapacityExceeded)?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use personal_rns::engine::CommandId;
    use prns_host::{CommandFailureKind, CommandOutcomeKind};

    use super::*;

    #[test]
    fn packed_settlements_use_the_host_contract_codes() {
        let settlements = vec![
            CapturedCommandSettlement::capture(CommandId(7), Settlement::AnnounceNow(Ok(()))),
            CapturedCommandSettlement::capture(
                CommandId(8),
                Settlement::CloseLink(Err(CloseLinkFailure::WriteFailed)),
            ),
            CapturedCommandSettlement::capture(
                CommandId(9),
                Settlement::SetRegisteredAnnounceAppData(Ok(())),
            ),
        ];

        let encoded = encode_batch(&settlements).expect("settlements encode");
        assert_eq!(
            &encoded[0..4],
            &COMMAND_SETTLEMENT_BATCH_MAGIC.to_le_bytes()
        );
        assert_eq!(&encoded[16..24], &7_u64.to_le_bytes());
        assert_eq!(encoded[24], 0);
        assert_eq!(
            &encoded[25..29],
            &(CommandOutcomeKind::Announced as u32).to_le_bytes()
        );
        assert_eq!(encoded[37], 1);
        assert_eq!(
            &encoded[38..42],
            &(CommandFailureKind::WriteFailed as u32).to_le_bytes()
        );
        assert_eq!(&encoded[42..46], &17_u32.to_le_bytes());
        assert_eq!(&encoded[46..63], b"link write failed");
        assert_eq!(encoded[71], 2);
    }
}
