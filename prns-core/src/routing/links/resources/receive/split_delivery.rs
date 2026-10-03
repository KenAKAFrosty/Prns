use super::conclude::response_application_data;
use crate::engine::{EngineReaction, Journaled};
use crate::routing::delivery::receipts::{ReceiptTable, Receipts};
use crate::routing::links::resources::assembly::{IncomingAssemblies, IncomingAssemblyTable};
use crate::routing::links::resources::{ResourceCorrelation, ResourceFailureCause, ResourceHash};
use crate::routing::links::LinkId;

pub(super) enum SplitDeliveryFailure {
    Resource(ResourceFailureCause),
    ResponseTooLarge,
}

pub(super) struct VerifiedSplitSegment<'a> {
    pub link_id: &'a LinkId,
    pub original_hash: ResourceHash,
    pub correlation: ResourceCorrelation,
    pub segment_index: u64,
    pub total_segments: u64,
    pub metadata: Option<&'a [u8]>,
    pub data: &'a [u8],
}

/// Responses expose only verified value bytes, charged before publication.
/// Other correlations retain the ordinary Resource delivery lane.
#[inline(never)]
pub(super) fn deliver_split_segment<C: ReceiptTable, A: IncomingAssemblyTable, Work>(
    receipts: &Receipts<C>,
    assemblies: &IncomingAssemblies<A>,
    segment: VerifiedSplitSegment<'_>,
    sink: &mut dyn FnMut(EngineReaction<'_, Work>),
) -> Result<u64, SplitDeliveryFailure> {
    let VerifiedSplitSegment {
        link_id,
        original_hash,
        correlation,
        segment_index,
        total_segments,
        metadata,
        data,
    } = segment;

    let answers = match correlation {
        ResourceCorrelation::Response(id) => receipts
            .pending_request_command(link_id, id)
            .map(|command_id| (command_id, id)),
        ResourceCorrelation::Request { .. } | ResourceCorrelation::Unsolicited => None,
    };
    let value_bytes = match answers {
        Some((command_id, request_id)) => {
            let data = if segment_index == 1 {
                let Some(data) = response_application_data(request_id, metadata, data) else {
                    return Err(SplitDeliveryFailure::Resource(
                        ResourceFailureCause::TransferCorrupt,
                    ));
                };
                data
            } else {
                data
            };
            if !receipts
                .pending_request_response_limit(link_id, request_id)
                .is_some_and(|limit| assemblies.fits_value_limit(link_id, data.len() as u64, limit))
            {
                return Err(SplitDeliveryFailure::ResponseTooLarge);
            }
            sink(EngineReaction::Journaled(
                Journaled::ResponseSegmentReceived {
                    command_id,
                    link_id: *link_id,
                    request_id,
                    segment_index,
                    total_segments,
                    data,
                },
            ));
            data.len() as u64
        }
        None => {
            sink(EngineReaction::Journaled(
                Journaled::ResourceSegmentReceived {
                    link_id: *link_id,
                    original_hash,
                    segment_index,
                    total_segments,
                    metadata,
                    data,
                },
            ));
            data.len() as u64
        }
    };
    Ok(value_bytes)
}
