//! The conclusion: the sealed transfer opens, verifies against the advertised hash, proves back to the sender, and delivers, with failures surfaced by name. The host-side decompression seam parks here.

use crate::engine::Journaled;
use crate::engine::{CommandId, SendRequestFailure};
use crate::engine::{DeliveryEvidence, PacketReceiptDelivered, Settlement};
use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis};
use crate::routing::delivery::receipts::{ReceiptTable, Receipts};
use crate::routing::links::data::link_raw_frame_ceiling;
use crate::routing::links::data::write_link_raw_packet;
use crate::routing::links::request::{
    parse_request_plaintext, parse_response_plaintext, request_response_timeout_ms, RequestId,
};
use crate::routing::links::resources::assemble_incoming::{
    open_transfer, verify_and_prove, OpenTransferError,
};
use crate::routing::links::resources::assembly::AssemblyProgress;
use crate::routing::links::resources::control::{write_proof_plaintext, PROOF_PLAINTEXT_LEN};
use crate::routing::links::resources::streamed_open::{OpenProgress, OpenedStream};
use crate::routing::links::resources::table::{IncomingResourceState, IncomingResourceStatus};
use crate::routing::links::resources::{
    ResourceCompression, ResourceCorrelation, ResourceFailureCause, ResourceHash, ResourceProof,
    DECOMPRESSION_GRACE_MS, OPEN_VERDICT_GRACE_MS,
};
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{PacketType, WireContext};

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.4.2 `Resource.assemble` + `prove`
    pub(crate) fn conclude_resource(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> ConcludeResourceOutcome {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return ConcludeResourceOutcome::NotTracked;
        };
        let state = *self.incoming_resources.state(index);
        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            rtt,
            role,
            remote_identity,
            ..
        }) = self.links.phase_for(link_id)
        else {
            return ConcludeResourceOutcome::LinkNotActive;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;
        let link_rtt = *rtt;
        let requester = *remote_identity;
        let responder_destination = match role {
            LinkRole::Responder { destination, .. } => Some(*destination),
            LinkRole::Initiator { .. } => None,
        };

        if let (_, OpenProgress::Chewing { .. }) =
            self.incoming_resources.transfer_and_streamed_open(index)
        {
            self.incoming_resources.state_mut(index).status = IncomingResourceStatus::AwaitingOpen;
            self.incoming_resources.set_timeout_at(
                index,
                Some(InstantMillis(now.0.saturating_add(OPEN_VERDICT_GRACE_MS))),
            );
            return ConcludeResourceOutcome::AwaitingOpenVerdict;
        }

        if state.compression == ResourceCompression::Bz2 {
            let opened = {
                let (transfer, streamed) = self
                    .incoming_resources
                    .transfer_and_streamed_open_mut(index);
                let stream = match core::mem::take(streamed) {
                    OpenProgress::Parked(open) => {
                        open.conclude(transfer).map(|opened| opened.stream)
                    }
                    OpenProgress::NotBegun | OpenProgress::Chewing { .. } => {
                        open_transfer(key, transfer)
                    }
                };
                match stream {
                    Ok(stream) => {
                        sink(EngineReaction::Journaled(
                            Journaled::ResourceNeedsDecompression {
                                link_id: *link_id,
                                hash: *hash,
                                stream,
                                uncompressed_data_bytes: state.uncompressed_data_bytes,
                            },
                        ));
                        true
                    }
                    Err(_) => false,
                }
            };
            if !opened {
                return self.fail_incoming_resource(
                    link_id,
                    hash,
                    ResourceFailureCause::TransferUnopenable,
                    sink,
                );
            }
            self.incoming_resources.state_mut(index).status =
                IncomingResourceStatus::AwaitingDecompression;
            self.incoming_resources.set_timeout_at(
                index,
                Some(InstantMillis(now.0.saturating_add(DECOMPRESSION_GRACE_MS))),
            );
            return ConcludeResourceOutcome::AwaitingInflate;
        }

        let multi_segment = state.total_segments > 1;
        let original_hash = self
            .incoming_assemblies
            .original_hash(link_id)
            .unwrap_or(*hash);

        let delivery = {
            let (transfer, streamed) = self
                .incoming_resources
                .transfer_and_streamed_open_mut(index);
            let opened = match core::mem::take(streamed) {
                OpenProgress::Parked(open) => open.conclude(transfer),
                OpenProgress::NotBegun | OpenProgress::Chewing { .. } => {
                    open_transfer(key, transfer).map(OpenedStream::rehashing)
                }
            };
            match verify_prove_split(opened, &state, hash, link_id, mtu) {
                Err(cause) => Err(cause),
                Ok(verified) => {
                    emit_proof(verified.prove, fire_on, sink);
                    self.links.note_outbound(link_id, now);
                    if multi_segment {
                        deliver_split_segment(
                            &self.receipts,
                            VerifiedSplitSegment {
                                link_id,
                                original_hash,
                                correlation: state.correlation,
                                segment_index: state.segment_index,
                                total_segments: state.total_segments,
                                metadata: verified.metadata,
                                data: verified.data,
                            },
                            sink,
                        );
                    } else {
                        let request_permitted = request_is_permitted(
                            &self.request_handlers,
                            state.correlation,
                            responder_destination,
                            requester,
                            verified.data,
                        );
                        deliver_single_segment(
                            &mut self.receipts,
                            AssembledSingleSegment {
                                destination: responder_destination,
                                link_id,
                                hash,
                                correlation: state.correlation,
                                link_rtt,
                                requester,
                                request_permitted,
                                metadata: verified.metadata,
                                data: verified.data,
                            },
                            now,
                            sink,
                        );
                    }
                    Ok(verified.stream_byte_len)
                }
            }
        };
        match delivery {
            Err(cause) => self.fail_incoming_resource(link_id, hash, cause, sink),
            Ok(segment_bytes) => {
                self.retire_incoming_resource(link_id, hash);
                if multi_segment {
                    self.advance_split_assembly(
                        link_id,
                        ConcludedSegment {
                            original_hash,
                            correlation: state.correlation,
                            segment_bytes,
                        },
                        link_rtt,
                        now,
                        sink,
                    );
                }
                ConcludeResourceOutcome::Delivered
            }
        }
    }

    /// A completed response chain settles its request in place of the `ResourceAssembled` journal; a chain still assembling hands the claimed request's timeout back until the next advertisement re-claims it.
    fn advance_split_assembly(
        &mut self,
        link_id: &LinkId,
        segment: ConcludedSegment,
        link_rtt: RttMillis,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let ConcludedSegment {
            original_hash,
            correlation,
            segment_bytes,
        } = segment;
        match self.incoming_assemblies.advance(link_id, segment_bytes) {
            Some(AssemblyProgress::Complete { total_size_bytes }) => {
                let settled = match correlation {
                    ResourceCorrelation::Response(id) => self.receipts.settle_by_request_id(id),
                    ResourceCorrelation::Request { .. } | ResourceCorrelation::Unsolicited => None,
                };
                match settled {
                    Some(proven) => sink(EngineReaction::Journaled(Journaled::CommandSettled {
                        id: proven.command_id,
                        settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                            rtt: RttMillis::measured_between(proven.sent_at, now),
                            evidence: DeliveryEvidence::Response,
                        })),
                    })),
                    None => sink(EngineReaction::Journaled(Journaled::ResourceAssembled {
                        link_id: *link_id,
                        original_hash,
                        total_size_bytes,
                    })),
                }
                self.incoming_assemblies.clear(link_id);
            }
            Some(AssemblyProgress::Assembling) => {
                if let ResourceCorrelation::Response(id) = correlation {
                    self.receipts.arm_request_timeout(
                        id,
                        InstantMillis(now.0.saturating_add(request_response_timeout_ms(link_rtt))),
                    );
                }
            }
            None => {}
        }
    }

    /// The one exit every dead incoming transfer leaves through: the slot retires (window and rate bequeathed to the link), the failure event carries the cause, and a response transfer's claimed request settles with it.
    pub(super) fn fail_incoming_resource(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        cause: ResourceFailureCause,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> ConcludeResourceOutcome {
        let settled_request = self.settle_response_claim(link_id, hash);
        self.retire_incoming_resource(link_id, hash);
        sink(EngineReaction::Journaled(Journaled::ResourceFailed {
            link_id: *link_id,
            hash: *hash,
            cause,
        }));
        if let Some(command_id) = settled_request {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: command_id,
                settlement: Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
            }));
        }
        ConcludeResourceOutcome::Failed(cause)
    }

    /// The receipts half of a response transfer's death: RNS 1.4.2 concludes any non-`COMPLETE` response resource through `request_timed_out`, so the pending request settles with the transfer.
    /// The caller journals the failure settlement for the returned command.
    pub(super) fn settle_response_claim(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
    ) -> Option<CommandId> {
        let index = self.incoming_resources.lookup(link_id, hash)?;
        let ResourceCorrelation::Response(request_id) =
            self.incoming_resources.state(index).correlation
        else {
            return None;
        };
        let proven = self.receipts.settle_by_request_id(request_id)?;
        Some(proven.command_id)
    }

    /// Verified exactly like an uncompressed assembly.
    /// The host signals its own inflate failure with an empty slice.
    /// A borrow-taking entry point beside the command queue (so a mebibyte never rides an enum).
    pub fn provide_decompressed(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        plaintext: &[u8],
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules {
        let mut wake_schedule_changes = crate::engine::WakeSchedules::UNCHANGED;
        let Some(index) = self.incoming_resources.lookup(&link_id, &hash) else {
            return wake_schedule_changes;
        };
        let state = *self.incoming_resources.state(index);
        if state.status != IncomingResourceStatus::AwaitingDecompression {
            return wake_schedule_changes;
        }
        self.retire_incoming_resource(&link_id, &hash);
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();

        let is_split = state.total_segments > 1;
        let inflated_whole = if is_split {
            !plaintext.is_empty()
        } else {
            u64::try_from(plaintext.len()) == Ok(state.uncompressed_data_bytes)
        };
        if !inflated_whole {
            self.fail_retired_incoming_resource(
                &link_id,
                &hash,
                state.correlation,
                ResourceFailureCause::DecompressionFailed,
                sink,
            );
            wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            return wake_schedule_changes;
        }
        let Some(LinkPhase::Active {
            mtu,
            attached_interface,
            rtt,
            role,
            remote_identity,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            self.fail_retired_incoming_resource(
                &link_id,
                &hash,
                state.correlation,
                ResourceFailureCause::LinkVanished,
                sink,
            );
            wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            return wake_schedule_changes;
        };
        let (mtu, fire_on, link_rtt) = (*mtu, *attached_interface, *rtt);
        let requester = *remote_identity;
        let responder_destination = match role {
            LinkRole::Responder { destination, .. } => Some(*destination),
            LinkRole::Initiator { .. } => None,
        };
        let Ok(proof) = verify_and_prove(plaintext, &state.salt_nonce, &hash) else {
            self.fail_retired_incoming_resource(
                &link_id,
                &hash,
                state.correlation,
                ResourceFailureCause::TransferCorrupt,
                sink,
            );
            wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            return wake_schedule_changes;
        };
        let Ok((metadata, data)) = split_metadata_block(&state, plaintext) else {
            self.fail_retired_incoming_resource(
                &link_id,
                &hash,
                state.correlation,
                ResourceFailureCause::MetadataOverrun,
                sink,
            );
            wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            return wake_schedule_changes;
        };
        let Some(prove) = proof_emission(&link_id, &hash, &proof, mtu) else {
            self.fail_retired_incoming_resource(
                &link_id,
                &hash,
                state.correlation,
                ResourceFailureCause::ProofUnsendable,
                sink,
            );
            wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            return wake_schedule_changes;
        };

        emit_proof(prove, fire_on, sink);
        self.links.note_outbound(&link_id, now);
        wake_schedule_changes.link_deadlines = self.link_deadlines_wake();

        if is_split {
            let original_hash = self
                .incoming_assemblies
                .original_hash(&link_id)
                .unwrap_or(hash);
            deliver_split_segment(
                &self.receipts,
                VerifiedSplitSegment {
                    link_id: &link_id,
                    original_hash,
                    correlation: state.correlation,
                    segment_index: state.segment_index,
                    total_segments: state.total_segments,
                    metadata,
                    data,
                },
                sink,
            );
            self.advance_split_assembly(
                &link_id,
                ConcludedSegment {
                    original_hash,
                    correlation: state.correlation,
                    segment_bytes: plaintext.len() as u64,
                },
                link_rtt,
                now,
                sink,
            );
        } else {
            let request_permitted = request_is_permitted(
                &self.request_handlers,
                state.correlation,
                responder_destination,
                requester,
                data,
            );
            deliver_single_segment(
                &mut self.receipts,
                AssembledSingleSegment {
                    destination: responder_destination,
                    link_id: &link_id,
                    hash: &hash,
                    correlation: state.correlation,
                    link_rtt,
                    requester,
                    request_permitted,
                    metadata,
                    data,
                },
                now,
                sink,
            );
        }
        wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
        wake_schedule_changes
    }

    /// [`Self::fail_incoming_resource`] for a slot already retired (the inflate seam retires before it judges), so the claim settles off the caller's copied correlation.
    fn fail_retired_incoming_resource(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        correlation: ResourceCorrelation,
        cause: ResourceFailureCause,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        sink(EngineReaction::Journaled(Journaled::ResourceFailed {
            link_id: *link_id,
            hash: *hash,
            cause,
        }));
        if let ResourceCorrelation::Response(request_id) = correlation {
            if let Some(proven) = self.receipts.settle_by_request_id(request_id) {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id: proven.command_id,
                    settlement: Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
                }));
            }
        }
    }
}

/// RNS 1.4.2's assemble tail for a metadata transfer: segment one's verified stream opens with `3-byte-BE-length ‖ packed block`, split off ahead of delivery. Every byte count around this point (the advertised `d`, the assembly advance) stays on the whole pre-split stream.
///
/// A declared length past the stream's end fails by name where the reference's Python slicing would silently deliver a truncated block and empty data.
fn split_metadata_block<'p>(
    state: &IncomingResourceState,
    plaintext: &'p [u8],
) -> Result<(Option<&'p [u8]>, &'p [u8]), ResourceFailureCause> {
    if !state.has_metadata || state.segment_index != 1 {
        return Ok((None, plaintext));
    }
    let [a, b, c, tail @ ..] = plaintext else {
        return Err(ResourceFailureCause::MetadataOverrun);
    };
    let declared = usize::from(*a) << 16 | usize::from(*b) << 8 | usize::from(*c);
    if declared > tail.len() {
        return Err(ResourceFailureCause::MetadataOverrun);
    }
    let (packed, data) = tail.split_at(declared);
    Ok((Some(packed), data))
}

struct AssembledSingleSegment<'a> {
    destination: Option<crate::wire::DestinationHash>,
    link_id: &'a LinkId,
    hash: &'a ResourceHash,
    correlation: ResourceCorrelation,
    link_rtt: RttMillis,
    requester: Option<crate::identity::IdentityHash>,
    request_permitted: bool,
    metadata: Option<&'a [u8]>,
    data: &'a [u8],
}

/// Stock RNS resource responses carry the same msgpack `[request_id, data]` envelope as packet
/// responses. Accept data that is not a valid envelope as the former Prns raw-body form for wire
/// continuity, but a valid envelope must name the request advertised by the resource.
fn response_application_data(request_id: RequestId, data: &[u8]) -> Option<&[u8]> {
    if data.first() != Some(&0x92) {
        return Some(data);
    }
    match parse_response_plaintext(data) {
        Ok((enclosed, body)) => (enclosed == request_id).then_some(body),
        Err(_) => Some(data),
    }
}

/// Correlated deliveries (a request or a settled response) carry no metadata lane because the reference's request/response machinery never reads it. A block on those transfers therefore strips and drops.
fn deliver_single_segment<C: ReceiptTable>(
    receipts: &mut Receipts<C>,
    segment: AssembledSingleSegment<'_>,
    now: InstantMillis,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    let AssembledSingleSegment {
        destination,
        link_id,
        hash,
        correlation,
        link_rtt,
        requester,
        request_permitted,
        metadata,
        data,
    } = segment;

    match correlation {
        ResourceCorrelation::Response(id) => {
            let Some(data) = response_application_data(id, data) else {
                return;
            };
            if let Some(proven) = receipts.settle_by_request_id(id) {
                sink(EngineReaction::Journaled(Journaled::ResponseReceived {
                    command_id: proven.command_id,
                    link_id: *link_id,
                    request_id: id,
                    data,
                }));
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id: proven.command_id,
                    settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::measured_between(proven.sent_at, now),
                        evidence: DeliveryEvidence::Response,
                    })),
                }));
            } else {
                sink(EngineReaction::Journaled(Journaled::ResourceReceived {
                    link_id: *link_id,
                    hash: *hash,
                    metadata,
                    data,
                }));
            }
        }
        ResourceCorrelation::Request { .. } => {
            if let (true, Some(destination)) = (request_permitted, destination) {
                let Ok(parsed) = parse_request_plaintext(data) else {
                    return;
                };
                sink(EngineReaction::Journaled(Journaled::RequestReceived {
                    destination,
                    link_id: *link_id,
                    request_id: RequestId::of_request_data(data),
                    requester,
                    path_hash: parsed.path_hash,
                    requested_at: parsed.requested_at,
                    rtt: link_rtt,
                    data: parsed.data,
                }));
            }
        }
        ResourceCorrelation::Unsolicited => {
            sink(EngineReaction::Journaled(Journaled::ResourceReceived {
                link_id: *link_id,
                hash: *hash,
                metadata,
                data,
            }));
        }
    }
}

fn request_is_permitted<C: crate::routing::request_handlers::RequestHandlerTable>(
    handlers: &crate::routing::request_handlers::RequestHandlers<C>,
    correlation: ResourceCorrelation,
    destination: Option<crate::wire::DestinationHash>,
    requester: Option<crate::identity::IdentityHash>,
    data: &[u8],
) -> bool {
    if !matches!(correlation, ResourceCorrelation::Request { .. }) {
        return true;
    }
    let Some(destination) = destination else {
        return false;
    };
    let Ok(parsed) = parse_request_plaintext(data) else {
        return false;
    };
    handlers.permits(&destination, &parsed.path_hash, requester.as_ref())
}

struct ConcludedSegment {
    original_hash: ResourceHash,
    correlation: ResourceCorrelation,
    segment_bytes: u64,
}

struct VerifiedSplitSegment<'a> {
    link_id: &'a LinkId,
    original_hash: ResourceHash,
    correlation: ResourceCorrelation,
    segment_index: u64,
    total_segments: u64,
    metadata: Option<&'a [u8]>,
    data: &'a [u8],
}

/// The split mirror of [`deliver_single_segment`]: a response chain's segments answer their pending request, with the metadata lane stripped and dropped the same way; everything else journals a plain segment.
fn deliver_split_segment<C: ReceiptTable>(
    receipts: &Receipts<C>,
    segment: VerifiedSplitSegment<'_>,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
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
            .pending_request_command(id)
            .map(|command_id| (command_id, id)),
        ResourceCorrelation::Request { .. } | ResourceCorrelation::Unsolicited => None,
    };
    match answers {
        Some((command_id, request_id)) => {
            let data = if segment_index == 1 {
                let Some(data) = response_application_data(request_id, data) else {
                    return;
                };
                data
            } else {
                data
            };
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcludeResourceOutcome {
    NotTracked,
    LinkNotActive,
    /// The sealed stream opened and went to the host for inflation; the slot waits under the decompression grace deadline.
    AwaitingInflate,
    /// A pool worker still holds the streamed open; the span verdict re-concludes, under its own grace deadline.
    AwaitingOpenVerdict,
    Delivered,
    Failed(ResourceFailureCause),
}

struct ProofEmission {
    link_id: LinkId,
    plaintext: [u8; PROOF_PLAINTEXT_LEN],
    mtu: usize,
}

struct VerifiedSegment<'t> {
    prove: ProofEmission,
    metadata: Option<&'t [u8]>,
    data: &'t [u8],
    stream_byte_len: u64,
}

fn verify_prove_split<'t>(
    opened: Result<OpenedStream<'t>, OpenTransferError>,
    state: &IncomingResourceState,
    hash: &ResourceHash,
    link_id: &LinkId,
    mtu: usize,
) -> Result<VerifiedSegment<'t>, ResourceFailureCause> {
    let Ok(opened) = opened else {
        return Err(ResourceFailureCause::TransferUnopenable);
    };
    let Ok(proof) = opened.verify_and_prove(&state.salt_nonce, hash) else {
        return Err(ResourceFailureCause::TransferCorrupt);
    };
    let Some(prove) = proof_emission(link_id, hash, &proof, mtu) else {
        return Err(ResourceFailureCause::ProofUnsendable);
    };
    let (metadata, data) = split_metadata_block(state, opened.stream)?;
    Ok(VerifiedSegment {
        prove,
        metadata,
        data,
        stream_byte_len: opened.stream.len() as u64,
    })
}

fn proof_emission(
    link_id: &LinkId,
    hash: &ResourceHash,
    proof: &ResourceProof,
    mtu: usize,
) -> Option<ProofEmission> {
    let mut plaintext = [0u8; PROOF_PLAINTEXT_LEN];
    write_proof_plaintext(hash, proof, &mut plaintext).ok()?;
    Some(ProofEmission {
        link_id: *link_id,
        plaintext,
        mtu,
    })
}

fn emit_proof(
    prove: ProofEmission,
    fire_on: crate::interfaces::InterfaceId,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    let mut fill = |slot: &mut [u8]| -> Option<usize> {
        write_link_raw_packet(
            &prove.link_id,
            PacketType::Proof,
            WireContext::ResourceProof,
            prove.mtu,
            &prove.plaintext,
            slot,
        )
        .ok()
    };
    sink(EngineReaction::Directive(Directive::EmitFrame {
        target: fire_on,
        size_hint: link_raw_frame_ceiling(prove.plaintext.len()),
        fill: &mut fill,
    }));
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::CommandId;
    use crate::engine::IngestIo;
    use crate::engine::Journaled;
    use crate::engine::Settlement;
    use crate::interfaces::AttachedInterfaces;
    use crate::routing::links::resources::receive::tests_support::*;
    use crate::routing::links::resources::table::IncomingResourceStatus;
    use crate::routing::links::resources::{ResourceBody, ResourceMetadata, ResourceSend};

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

    #[test]
    fn response_resources_strip_only_a_matching_valid_stock_envelope() {
        use crate::routing::links::request::write_response_plaintext;

        let request_id = RequestId([0x41; 16]);
        let mut packed = [0u8; 64];
        let len = write_response_plaintext(&request_id, b"answer", &mut packed).unwrap();
        assert_eq!(
            response_application_data(request_id, &packed[..len]),
            Some(&b"answer"[..])
        );
        assert_eq!(
            response_application_data(RequestId([0x42; 16]), &packed[..len]),
            None,
            "a stock envelope cannot settle another request",
        );
        let former_raw_body = b"\x92not-a-valid-response-envelope";
        assert_eq!(
            response_application_data(request_id, former_raw_body),
            Some(&former_raw_body[..]),
            "a legacy raw body remains data even when its first byte resembles msgpack",
        );
    }

    #[test]
    fn resource_requests_reenter_the_registered_route_policy() {
        use crate::routing::links::request::write_request_plaintext;
        use crate::routing::request_handlers::{
            FixedRequestHandlerTable, RequestHandlers, RequestPathHash, RequestPolicy,
        };

        let destination = crate::wire::DestinationHash::new([0x31; 16]);
        let path_hash = RequestPathHash::of("command");
        let mut packed = [0u8; 64];
        let len =
            write_request_plaintext(InstantMillis(1_000), &path_hash, b"request", &mut packed)
                .unwrap();
        let correlation = ResourceCorrelation::Request {
            id: RequestId::of_request_data(&packed[..len]),
            response_timeout: Default::default(),
            maximum_response_bytes: Default::default(),
        };
        let mut handlers = RequestHandlers::<FixedRequestHandlerTable<1>>::default();
        assert!(!request_is_permitted(
            &handlers,
            correlation,
            Some(destination),
            None,
            &packed[..len],
        ));
        handlers
            .register(destination, path_hash, RequestPolicy::RequireIdentified)
            .unwrap();
        assert!(!request_is_permitted(
            &handlers,
            correlation,
            Some(destination),
            None,
            &packed[..len],
        ));
        assert!(request_is_permitted(
            &handlers,
            correlation,
            Some(destination),
            Some(crate::identity::IdentityHash::new([0x53; 16])),
            &packed[..len],
        ));
    }

    #[test]
    fn a_compressed_transfer_crosses_through_the_host_inflate_seam() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &plaintext,
                    compressed_candidate: Some(&candidate),
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 1, "the compressed stream is one part");

        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        receiver.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[
                    crate::engine::test_support::routable_descriptor(lane()),
                ]),
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        stream,
                        uncompressed_data_bytes,
                        ..
                    }) = reaction
                    {
                        needs = Some((hash, stream.to_vec(), uncompressed_data_bytes));
                    }
                },
            },
        );
        let (hash, stream, advertised_len) = needs.expect("the seam asks the host to inflate");
        assert_eq!(
            stream,
            bytes_from_hex(CASE1_BZ2),
            "the host receives exactly the bz2 stream the sender compressed",
        );
        assert_eq!(advertised_len, 1_360);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        assert_eq!(
            receiver.incoming_resources.state(index).status,
            IncomingResourceStatus::AwaitingDecompression,
        );

        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        receiver.provide_decompressed(
            link_id(),
            hash,
            &plaintext,
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceReceived { data, .. }) => {
                    received.push(data.to_vec());
                }
                _ => {}
            },
        );
        assert_eq!(received.len(), 1);
        assert_eq!(received[0], plaintext);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(frames.len(), 1, "the proof rides back");

        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(sender.outgoing_resources.is_empty());
    }

    #[test]
    fn a_compressed_metadata_transfer_inflates_and_splits_the_block() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let packed = bytes_from_hex(META_PACKED);
        let candidate = bytes_from_hex(META_CASE1_BZ2);
        let mut composite = metadata_block(&packed);
        composite.extend_from_slice(&plaintext);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(9),
                link_id: link_id(),
                body: ResourceBody {
                    data: &plaintext,
                    compressed_candidate: Some(&candidate),
                    metadata: ResourceMetadata::Packed(&packed),
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        receiver.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[
                    crate::engine::test_support::routable_descriptor(lane()),
                ]),
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        stream,
                        uncompressed_data_bytes,
                        ..
                    }) = reaction
                    {
                        needs = Some((hash, stream.to_vec(), uncompressed_data_bytes));
                    }
                },
            },
        );
        let (hash, stream, advertised_len) = needs.expect("the seam asks the host to inflate");
        assert_eq!(
            stream, candidate,
            "the host receives the bz2 of the whole composite",
        );
        assert_eq!(
            advertised_len,
            composite.len() as u64,
            "the inflate target counts the block",
        );

        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        receiver.provide_decompressed(
            link_id(),
            hash,
            &composite,
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceReceived {
                    metadata, data, ..
                }) => {
                    received.push((metadata.map(<[u8]>::to_vec), data.to_vec()));
                }
                _ => {}
            },
        );
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0],
            (Some(packed), plaintext),
            "the inflated composite splits into the block and the original data",
        );
        assert!(receiver.incoming_resources.is_empty());

        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(9), Settlement::SendResource(Ok(()))),
        ));
    }

    #[test]
    fn a_lying_metadata_prefix_fails_by_name() {
        use super::split_metadata_block;
        use crate::routing::links::resources::table::IncomingResourceState;
        let first_segment_with_block = IncomingResourceState {
            has_metadata: true,
            ..IncomingResourceState::default()
        };
        for lying_stream in [
            &[][..],
            &[0x00][..],
            &[0x00, 0x00][..],
            &[0x00, 0x00, 0x05, 0xAA, 0xBB][..],
            &[0xFF, 0xFF, 0xFF, 0xAA][..],
        ] {
            assert_eq!(
                split_metadata_block(&first_segment_with_block, lying_stream),
                Err(ResourceFailureCause::MetadataOverrun),
            );
        }
        assert_eq!(
            split_metadata_block(&first_segment_with_block, &[0x00, 0x00, 0x02, 0xAA, 0xBB]),
            Ok((Some(&[0xAA, 0xBB][..]), &[][..])),
            "an exact-fit block leaves empty data, like the reference",
        );
        let later_segment = IncomingResourceState {
            has_metadata: true,
            segment_index: 2,
            ..IncomingResourceState::default()
        };
        assert_eq!(
            split_metadata_block(&later_segment, &[0xFF, 0xFF]),
            Ok((None, &[0xFF, 0xFF][..])),
            "past segment one the stream passes through untouched",
        );
    }

    #[test]
    fn a_wrong_or_empty_inflate_fails_the_transfer_by_hash() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &plaintext,
                    compressed_candidate: Some(&candidate),
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let conclusion = feed(&mut receiver, &serve.frames[0].1, 2_200);
        assert!(conclusion.received.is_empty());
        let hash = *receiver.incoming_resources.hash_at(0);

        let mut corrupted = plaintext.clone();
        corrupted[0] ^= 1;
        let mut failed = std::vec::Vec::new();
        let mut frames = 0usize;
        receiver.provide_decompressed(
            link_id(),
            hash,
            &corrupted,
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::ResourceFailed { hash, .. }) => {
                    failed.push(hash);
                }
                EngineReaction::Directive(_) => frames += 1,
                _ => {}
            },
        );
        assert_eq!(failed.len(), 1);
        assert_eq!(frames, 0, "a failed inflate proves nothing");
        assert!(receiver.incoming_resources.is_empty());

        receiver.provide_decompressed(
            link_id(),
            hash,
            &plaintext,
            InstantMillis(2_500),
            &mut |_| {
                panic!("a retired transfer answers nothing");
            },
        );
    }

    #[test]
    fn the_seam_only_answers_transfers_awaiting_decompression() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let mut touched = false;
        receiver.provide_decompressed(
            link_id(),
            ResourceHash::new([0x42; 32]),
            b"anything",
            InstantMillis(2_400),
            &mut |_| touched = true,
        );
        assert!(!touched, "an unknown transfer answers nothing");
    }

    #[test]
    fn a_compressed_response_to_a_pending_request_is_accepted_and_settles_it() {
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x55; 16]);
        let request_data = b"a request too fat for a packet, ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let request_id = RequestId::of_request_data(&packed[..plain_len]);

        let mut requester = engine_with_active_link();
        requester.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(55),
                link_id: link_id(),
                body: ResourceBody {
                    data: &packed[..plain_len],
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Request {
                    id: request_id,
                    response_timeout: Default::default(),
                    maximum_response_bytes: Default::default(),
                },
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    let _ = filled_frame(fill);
                }
            },
        );
        assert!(requester.receipts.has_pending_request(request_id));

        let mut responder = engine_with_active_link();
        let response = case1_plaintext();
        let candidate = b"pretend bz2, just visibly shorter".to_vec();
        let mut advertisement = None;
        responder.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(9),
                link_id: link_id(),
                body: ResourceBody {
                    data: &response,
                    compressed_candidate: Some(&candidate),
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Response(request_id),
            },
            InstantMillis(1_600),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut requester, &advertisement.unwrap(), 2_000);
        assert!(
            !requester.incoming_resources.is_empty(),
            "a compressed response to a pending request bypasses the strategy like the reference",
        );
        let serve = feed(&mut responder, &pull.frames[0].1, 2_100);
        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        requester.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[
                    crate::engine::test_support::routable_descriptor(lane()),
                ]),
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        stream,
                        ..
                    }) = reaction
                    {
                        needs = Some((hash, stream.to_vec()));
                    }
                },
            },
        );
        let (hash, stream) = needs.expect("the compressed response reaches the inflate seam");
        assert_eq!(stream, candidate);

        let mut proof_frames = 0usize;
        let mut responses = std::vec::Vec::new();
        let mut settled_ok = false;
        requester.provide_decompressed(
            link_id(),
            hash,
            &response,
            InstantMillis(2_400),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { .. }) => proof_frames += 1,
                EngineReaction::Journaled(Journaled::ResponseReceived {
                    command_id,
                    request_id: rid,
                    data,
                    ..
                }) => responses.push((command_id, rid, data.to_vec())),
                EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendRequest(Ok(_)),
                }) => settled_ok = id == CommandId(55),
                _ => {}
            },
        );
        assert_eq!(proof_frames, 1, "the proof rides back");
        assert_eq!(responses, std::vec![(CommandId(55), request_id, response)]);
        assert!(
            settled_ok,
            "the inflated response settles the pending request it answers",
        );
        assert!(!requester.receipts.has_pending_request(request_id));
    }

    #[test]
    fn a_compressed_request_resource_is_accepted_and_delivered_as_a_request() {
        use crate::routing::links::request::{write_request_plaintext, RequestId};
        use crate::routing::links::resources::ResourceCorrelation;
        use crate::routing::request_handlers::RequestPathHash;

        let path_hash = RequestPathHash::new([0x66; 16]);
        let request_data = b"a fat, highly compressible request ".repeat(40);
        let mut packed = std::vec![0u8; request_data.len() + 64];
        let plain_len =
            write_request_plaintext(InstantMillis(1_400), &path_hash, &request_data, &mut packed)
                .unwrap();
        let packed_request = packed[..plain_len].to_vec();
        let request_id = RequestId::of_request_data(&packed_request);
        let candidate = b"pretend bz2 for the request body".to_vec();

        let mut requester = engine_with_active_link();
        let mut advertisement = None;
        requester.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(56),
                link_id: link_id(),
                body: ResourceBody {
                    data: &packed_request,
                    compressed_candidate: Some(&candidate),
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Request {
                    id: request_id,
                    response_timeout: Default::default(),
                    maximum_response_bytes: Default::default(),
                },
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let mut responder = engine_with_responding_link();
        responder
            .request_handlers
            .register(
                RESPONDER_DESTINATION,
                path_hash,
                crate::routing::request_handlers::RequestPolicy::AllowAll,
            )
            .unwrap();
        let pull = feed(&mut responder, &advertisement.unwrap(), 2_000);
        assert!(
            !responder.incoming_resources.is_empty(),
            "a compressed request resource is accepted, like the reference's unconditional Resource.accept",
        );
        let serve = feed(&mut requester, &pull.frames[0].1, 2_100);
        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        responder.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[
                    crate::engine::test_support::routable_descriptor(lane()),
                ]),
                now: InstantMillis(2_200),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        ..
                    }) = reaction
                    {
                        needs = Some(hash);
                    }
                },
            },
        );
        let hash = needs.expect("the compressed request reaches the inflate seam");

        let mut requests = std::vec::Vec::new();
        responder.provide_decompressed(
            link_id(),
            hash,
            &packed_request,
            InstantMillis(2_400),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RequestReceived {
                    destination,
                    request_id: rid,
                    path_hash: ph,
                    data,
                    ..
                }) = reaction
                {
                    assert_eq!(destination, RESPONDER_DESTINATION);
                    requests.push((rid, ph, data.to_vec()));
                }
            },
        );
        assert_eq!(requests, std::vec![(request_id, path_hash, request_data)]);
    }

    type SegmentsSeen = std::vec::Vec<(ResourceHash, u64, u64, std::vec::Vec<u8>)>;
    type AssembliesSeen = std::vec::Vec<(ResourceHash, u64)>;

    fn pump_compressed_segment(
        sender: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        receiver: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        id: CommandId,
        data: &[u8],
        candidate: &[u8],
        segment: crate::routing::links::resources::ResourceSegment,
        at: u64,
    ) -> (SegmentsSeen, AssembliesSeen) {
        use crate::routing::links::resources::ResourceCorrelation;

        let mut advertisement = None;
        sender.ingest_send_resource_segment_into(
            &ResourceSend {
                id,
                link_id: link_id(),
                body: ResourceBody {
                    data,
                    compressed_candidate: Some(candidate),
                    metadata: ResourceMetadata::None,
                },
                correlation: ResourceCorrelation::Unsolicited,
            },
            segment,
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(receiver, &advertisement.unwrap(), at + 100);
        let serve = feed(sender, &pull.frames[0].1, at + 200);
        assert_eq!(serve.frames.len(), 1, "a tiny candidate is one part");

        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        receiver.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(at + 300),
                source_interface: lane(),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&[
                    crate::engine::test_support::routable_descriptor(lane()),
                ]),
                now: InstantMillis(at + 300),
                fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                should_prove: &mut |_: &crate::engine::ProofRequest| false,
                should_accept_resource:
                    &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                        hash,
                        stream,
                        ..
                    }) = reaction
                    {
                        needs = Some((hash, stream.to_vec()));
                    }
                },
            },
        );
        let (hash, stream) = needs.expect("the compressed segment reaches the inflate seam");
        assert_eq!(stream, candidate);

        let mut segments = std::vec::Vec::new();
        let mut assembled = std::vec::Vec::new();
        let mut proof_frame = None;
        receiver.provide_decompressed(
            link_id(),
            hash,
            data,
            InstantMillis(at + 400),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    proof_frame = filled_frame(fill);
                }
                EngineReaction::Journaled(Journaled::ResourceSegmentReceived {
                    original_hash,
                    segment_index,
                    total_segments,
                    data,
                    ..
                }) => segments.push((original_hash, segment_index, total_segments, data.to_vec())),
                EngineReaction::Journaled(Journaled::ResourceAssembled {
                    original_hash,
                    total_size_bytes,
                    ..
                }) => assembled.push((original_hash, total_size_bytes)),
                _ => {}
            },
        );
        let settled = feed(
            sender,
            &proof_frame.expect("the proof rides back"),
            at + 500,
        );
        assert!(matches!(
            settled.settlements[0],
            (settled_id, Settlement::SendResource(Ok(()))) if settled_id == id,
        ));
        (segments, assembled)
    }

    #[test]
    fn a_compressed_split_transfer_inflates_per_segment_and_assembles() {
        use crate::routing::links::resources::ResourceSegment;

        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let segment_one = b"segment one rides the link compressed ".repeat(40);
        let segment_two = b"segment two rides the link compressed ".repeat(40);
        let total = (segment_one.len() + segment_two.len()) as u64;

        let (segments_one, assembled_one) = pump_compressed_segment(
            &mut sender,
            &mut receiver,
            CommandId(21),
            &segment_one,
            b"pretend bz2 for segment one",
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: total,
            },
            2_000,
        );
        assert_eq!(segments_one.len(), 1);
        let original_hash = segments_one[0].0;
        assert_eq!(segments_one[0].1, 1);
        assert_eq!(segments_one[0].2, 2);
        assert_eq!(segments_one[0].3, segment_one);
        assert!(
            assembled_one.is_empty(),
            "the assembly does not complete on the first segment",
        );

        let (segments_two, assembled_two) = pump_compressed_segment(
            &mut sender,
            &mut receiver,
            CommandId(22),
            &segment_two,
            b"pretend bz2 for segment two",
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: total,
            },
            4_000,
        );
        assert_eq!(
            segments_two[0].0, original_hash,
            "every segment re-advertises the chain's original hash",
        );
        assert_eq!(segments_two[0].1, 2);
        assert_eq!(segments_two[0].3, segment_two);
        assert_eq!(
            assembled_two,
            std::vec![(original_hash, total)],
            "the last inflated segment completes the assembly at the running byte total",
        );
        assert!(
            receiver
                .incoming_assemblies
                .original_hash(&link_id())
                .is_none(),
            "the receiver's chain retires with the completed assembly",
        );
    }

    fn feed_parts(
        receiver: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
        parts: &[(crate::interfaces::InterfaceId, std::vec::Vec<u8>)],
        from: u64,
    ) -> (InboundCapture, u64) {
        let mut conclusion = None;
        let mut concluded_at = from;
        for (arrived, (_, part)) in parts.iter().enumerate() {
            let at = from + arrived as u64;
            let capture = feed(receiver, part, at);
            if !capture.settlements.is_empty()
                || !capture.response_segments.is_empty()
                || !capture.segments.is_empty()
            {
                conclusion = Some(capture);
                concluded_at = at;
            }
        }
        (
            conclusion.expect("the last part concludes the segment"),
            concluded_at,
        )
    }

    #[test]
    fn a_two_segment_response_settles_its_request_at_final_assembly() {
        use crate::routing::links::request::request_response_timeout_ms;
        use crate::routing::links::resources::ResourceSegment;
        use crate::units::RttMillis;

        let mut requester = engine_with_active_link();
        let request_id = track_pending_request(&mut requester, CommandId(42), 1_800, 20_000);
        let mut responder = engine_with_active_link();
        let segment_one = b"segment one of the fat response ".repeat(40);
        let segment_two = b"segment two of the fat response ".repeat(40);
        let total = (segment_one.len() + segment_two.len()) as u64;

        let advertisement = advertise_response_segment_from(
            &mut responder,
            CommandId(21),
            request_id,
            &segment_one,
            None,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: total,
            },
            1_900,
        );
        let pull = feed(&mut requester, &advertisement, 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "a split response to a request we sent is pulled, default strategy notwithstanding",
        );
        assert_eq!(
            requester.receipts.earliest_timeout_at(),
            None,
            "the accepted transfer claims the request's timeout",
        );

        let serve = feed(&mut responder, &pull.frames[0].1, 2_100);
        let (first, first_concluded_at) = feed_parts(&mut requester, &serve.frames, 2_200);
        assert_eq!(
            first.response_segments,
            std::vec![(CommandId(42), request_id, 1, segment_one.clone())],
            "a response chain's segment answers the pending request, not the resource sinks",
        );
        assert!(first.segments.is_empty());
        assert!(
            first.settlements.is_empty(),
            "no settle before the chain completes"
        );
        assert_eq!(
            requester.receipts.earliest_timeout_at(),
            Some(InstantMillis(
                first_concluded_at + request_response_timeout_ms(RttMillis::new(250)),
            )),
            "between segments the request's own timeout re-arms",
        );
        let proof = &first.frames[0].1;
        let settled_send = feed(&mut responder, proof, 2_300);
        assert!(matches!(
            settled_send.settlements[0],
            (CommandId(21), Settlement::Respond(Ok(()))),
        ));

        let advertisement = advertise_response_segment_from(
            &mut responder,
            CommandId(22),
            request_id,
            &segment_two,
            None,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: total,
            },
            2_400,
        );
        let pull = feed(&mut requester, &advertisement, 2_500);
        assert_eq!(pull.frames.len(), 1);
        assert_eq!(
            requester.receipts.earliest_timeout_at(),
            None,
            "the next segment's acceptance re-claims the timeout",
        );

        let serve = feed(&mut responder, &pull.frames[0].1, 2_600);
        let (last, _) = feed_parts(&mut requester, &serve.frames, 2_700);
        assert_eq!(
            last.response_segments,
            std::vec![(CommandId(42), request_id, 2, segment_two.clone())],
        );
        assert!(matches!(
            last.settlements[0],
            (CommandId(42), Settlement::SendRequest(Ok(_))),
        ));
        assert!(
            last.assembled.is_empty(),
            "the settle replaces the ResourceAssembled journal for a response chain",
        );
        assert!(!requester.receipts.has_pending_request(request_id));
        assert!(requester.incoming_resources.is_empty());
        assert!(requester
            .incoming_assemblies
            .original_hash(&link_id())
            .is_none());
    }

    #[test]
    fn the_request_timeout_parks_while_a_response_transfer_is_live() {
        let mut requester = engine_with_active_link();
        let request_id = track_pending_request(&mut requester, CommandId(42), 1_800, 5_000);
        let mut responder = engine_with_active_link();
        let response = case1_plaintext();

        let mut advertisement = None;
        responder.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(9),
                link_id: link_id(),
                body: ResourceBody {
                    data: &response,
                    compressed_candidate: None,
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Response(
                    request_id,
                ),
            },
            InstantMillis(1_900),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(&mut requester, &advertisement.unwrap(), 2_000);
        assert_eq!(pull.frames.len(), 1);

        let mut settled = std::vec::Vec::new();
        requester.settle_timed_out_receipts(InstantMillis(50_000), &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        });
        assert!(
            settled.is_empty(),
            "the request cannot time out under its claimed transfer (the reference's RECEIVING flip)",
        );

        let serve = feed(&mut responder, &pull.frames[0].1, 60_000);
        let (conclusion, _) = feed_parts(&mut requester, &serve.frames, 60_100);
        assert!(matches!(
            conclusion.settlements[0],
            (CommandId(42), Settlement::SendRequest(Ok(_))),
        ));
    }

    #[test]
    fn a_dead_response_transfer_settles_its_request_by_name() {
        use crate::engine::SendRequestFailure;

        let mut requester = engine_with_active_link();
        let request_id = track_pending_request(&mut requester, CommandId(42), 1_800, 5_000);
        let mut responder = engine_with_active_link();
        let advertisement = advertise_response_segment_from(
            &mut responder,
            CommandId(21),
            request_id,
            &case1_plaintext(),
            None,
            crate::routing::links::resources::ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: 4_000,
            },
            1_900,
        );
        feed(&mut requester, &advertisement, 2_000);
        let hash = *requester.incoming_resources.hash_at(0);
        let index = requester
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        requester.incoming_resources.state_mut(index).retries_left = 0;

        let mut failed = std::vec::Vec::new();
        let mut settled = std::vec::Vec::new();
        requester.fire_due_resource_deadlines(
            InstantMillis(120_000),
            &mut |bytes: &mut [u8]| bytes.fill(0xF2),
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::ResourceFailed { cause, .. }) => {
                    failed.push(cause);
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    settled.push((id, settlement));
                }
                _ => {}
            },
        );
        assert_eq!(failed, [ResourceFailureCause::RetriesExhausted]);
        assert!(matches!(
            settled[0],
            (
                CommandId(42),
                Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
            ),
        ));
        assert!(!requester.receipts.has_pending_request(request_id));
    }

    #[test]
    fn a_response_chain_stalled_between_segments_times_out_the_request() {
        use crate::engine::SendRequestFailure;
        use crate::routing::links::request::request_response_timeout_ms;
        use crate::routing::links::resources::ResourceSegment;
        use crate::units::RttMillis;

        let mut requester = engine_with_active_link();
        let request_id = track_pending_request(&mut requester, CommandId(42), 1_800, 20_000);
        let mut responder = engine_with_active_link();
        let segment_one = b"segment one, and then silence ".repeat(40);

        let advertisement = advertise_response_segment_from(
            &mut responder,
            CommandId(21),
            request_id,
            &segment_one,
            None,
            ResourceSegment {
                index: 1,
                total_segments: 2,
                total_data_bytes: (2 * segment_one.len()) as u64,
            },
            1_900,
        );
        let pull = feed(&mut requester, &advertisement, 2_000);
        let serve = feed(&mut responder, &pull.frames[0].1, 2_100);
        let (first, concluded_at) = feed_parts(&mut requester, &serve.frames, 2_200);
        feed(&mut responder, &first.frames[0].1, concluded_at + 50);

        let deadline = concluded_at + request_response_timeout_ms(RttMillis::new(250));
        let mut settled = std::vec::Vec::new();
        requester.settle_timed_out_receipts(InstantMillis(deadline + 1), &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        });
        assert!(matches!(
            settled[0],
            (
                CommandId(42),
                Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
            ),
        ));

        let late_second = advertise_response_segment_from(
            &mut responder,
            CommandId(22),
            request_id,
            &segment_one,
            None,
            ResourceSegment {
                index: 2,
                total_segments: 2,
                total_data_bytes: (2 * segment_one.len()) as u64,
            },
            deadline + 100,
        );
        let refused = feed(&mut requester, &late_second, deadline + 200);
        assert!(refused.frames.is_empty());
        assert!(
            requester.incoming_resources.is_empty(),
            "a segment for a settled request no longer names a pending request and drops",
        );
    }

    #[test]
    fn a_compressed_split_response_inflates_per_segment_and_settles() {
        use crate::engine::IngestIo;
        use crate::interfaces::AttachedInterfaces;
        use crate::routing::links::resources::ResourceSegment;

        let mut requester = engine_with_active_link();
        let request_id = track_pending_request(&mut requester, CommandId(42), 1_800, 20_000);
        let mut responder = engine_with_active_link();
        let segment_one = b"segment one rides the link compressed ".repeat(40);
        let segment_two = b"segment two rides the link compressed ".repeat(40);
        let total = (segment_one.len() + segment_two.len()) as u64;

        for (index, data, candidate, expect_settle) in [
            (
                1u64,
                &segment_one,
                &b"pretend bz2 for segment one"[..],
                false,
            ),
            (
                2u64,
                &segment_two,
                &b"pretend bz2 for segment two"[..],
                true,
            ),
        ] {
            let at = 2_000 + index * 1_000;
            let advertisement = advertise_response_segment_from(
                &mut responder,
                CommandId(20 + index),
                request_id,
                data,
                Some(candidate),
                ResourceSegment {
                    index,
                    total_segments: 2,
                    total_data_bytes: total,
                },
                at,
            );
            let pull = feed(&mut requester, &advertisement, at + 100);
            let serve = feed(&mut responder, &pull.frames[0].1, at + 200);

            let mut needs = None;
            let mut raw = serve.frames[0].1.clone();
            requester.ingest_packet_into(
                crate::interfaces::InboundPacket {
                    arrived_at: InstantMillis(at + 300),
                    source_interface: lane(),
                    bytes: &mut raw,
                },
                IngestIo {
                    interfaces: AttachedInterfaces::new(&[
                        crate::engine::test_support::routable_descriptor(lane()),
                    ]),
                    now: InstantMillis(at + 300),
                    fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0xC7),
                    should_prove: &mut |_: &crate::engine::ProofRequest| false,
                    should_accept_resource:
                        &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                    sink: &mut |reaction| {
                        if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                            hash,
                            ..
                        }) = reaction
                        {
                            needs = Some(hash);
                        }
                    },
                },
            );
            let hash = needs.expect("the compressed response segment reaches the inflate seam");

            let mut response_segments = std::vec::Vec::new();
            let mut settled = std::vec::Vec::new();
            let mut proof_frame = None;
            requester.provide_decompressed(
                link_id(),
                hash,
                data,
                InstantMillis(at + 400),
                &mut |reaction| match reaction {
                    EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                        proof_frame = filled_frame(fill);
                    }
                    EngineReaction::Journaled(Journaled::ResponseSegmentReceived {
                        command_id,
                        segment_index,
                        data,
                        ..
                    }) => response_segments.push((command_id, segment_index, data.to_vec())),
                    EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                        settled.push((id, settlement));
                    }
                    _ => {}
                },
            );
            assert_eq!(
                response_segments,
                std::vec![(CommandId(42), index, data.clone())],
                "the inflated segment answers the pending request",
            );
            if expect_settle {
                assert!(matches!(
                    settled[0],
                    (CommandId(42), Settlement::SendRequest(Ok(_))),
                ));
            } else {
                assert!(settled.is_empty(), "no settle before the chain completes");
            }
            feed(
                &mut responder,
                &proof_frame.expect("the proof rides back"),
                at + 500,
            );
        }
        assert!(!requester.receipts.has_pending_request(request_id));
        assert!(requester.incoming_resources.is_empty());
    }

    #[test]
    fn an_unanswered_inflate_fails_the_transfer_at_its_grace_deadline() {
        use crate::engine::WakeSchedule;
        use crate::routing::links::resources::DECOMPRESSION_GRACE_MS;

        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let candidate = bytes_from_hex(CASE1_BZ2);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            &ResourceSend {
                id: CommandId(7),
                link_id: link_id(),
                body: ResourceBody {
                    data: &plaintext,
                    compressed_candidate: Some(&candidate),
                    metadata: ResourceMetadata::None,
                },
                correlation: crate::routing::links::resources::ResourceCorrelation::Unsolicited,
            },
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let parked = feed(&mut receiver, &serve.frames[0].1, 2_200);
        assert!(parked.received.is_empty());
        assert_eq!(
            receiver.resource_deadlines_wake(),
            WakeSchedule::At(InstantMillis(2_200 + DECOMPRESSION_GRACE_MS)),
            "a parked inflate holds a deadline instead of pinning the slot forever",
        );

        let mut failed = std::vec::Vec::new();
        receiver.fire_due_resource_deadlines(
            InstantMillis(2_200 + DECOMPRESSION_GRACE_MS + 1),
            &mut |bytes: &mut [u8]| bytes.fill(0xF2),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::ResourceFailed { cause, .. }) = reaction
                {
                    failed.push(cause);
                }
            },
        );
        assert_eq!(
            failed,
            [ResourceFailureCause::DecompressionTimedOut],
            "the grace deadline fails the transfer by name",
        );
        assert!(receiver.incoming_resources.is_empty());
    }
}
