use crate::crypto::ratchets::RatchetRotation;
use crate::crypto::{X25519PublicKey, X25519SharedSecret};
use crate::engine::node_egress::{fan_announce, fan_frame};
use crate::engine::settlement::{culled_settlement, settle};
#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceCommandOutcome;
use crate::engine::{
    AllowRequesterFailure, AnnounceNowFailure, AnnounceTarget, AnnounceWriteFailure,
    CloseLinkFailure, CommandOutcome, CommandedAnnounceWriteOutcome, Directive, EncryptOwed,
    EngineReaction, EngineState, EstablishLinkFailure, EstablishLinkWriteOutcome, FanTarget,
    FinishSendSinglePacketOutcome, IdentifyFailure, IdentifyRejection, InstantMillis,
    IssuedCommand, Journaled, PathRequestWriteOutcome, RequestPathFailure, RespondFailure,
    RespondRejection, SendGroupEntropy, SendGroupFailure, SendRequestFailure, SendRequestRejection,
    SendSinglePacketEntropy, SendSinglePacketFailure, SendSinglePacketWriteError,
    SendSinglePacketWriteOutcome, SendToChannelFailure, SendToChannelRejection, SendToLinkFailure,
    SendToLinkRejection, SetResourceStrategyFailure, Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::InterfaceId;
use crate::routing::delivery::receipts::ReceiptKind;
use crate::routing::links::channel::send::SendToChannelWriteError;
use crate::routing::links::channel::CHANNEL_ENVELOPE_HEADER_LEN;
use crate::routing::links::data::{link_data_frame_ceiling, LinkDataError, SendToLinkWriteError};
use crate::routing::links::establish::EstablishLinkEntropy;
use crate::routing::links::identify::IdentifyWriteError;
use crate::routing::links::request::{
    response_data_wire_len, LinkRequestWriteError, REQUEST_WIRE_OVERHEAD, RESPONSE_WIRE_OVERHEAD,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;

impl<S: StorageLayout> EngineState<S> {
    /// Resolves the link's interface only so the grant-first directive can name its target; the manifold must know which lane to offer a slot from before `fill` runs. The write inside `fill` looks the link up again and that second lookup is the authority: a link gone by then fails there as `LinkVanished`.
    fn active_link_interface(&self, link_id: &LinkId) -> Option<InterfaceId> {
        match self.links.phase_for(link_id)? {
            LinkPhase::Active {
                attached_interface, ..
            } => Some(*attached_interface),
            _ => None,
        }
    }

    pub fn ingest_command_into<F>(
        &mut self,
        issued: IssuedCommand,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut wake_schedule_changes = WakeSchedules::UNCHANGED;
        match self.ingest_command(issued, interfaces) {
            CommandOutcome::OwesAnnounce { id, announce } => {
                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_announce(
                    &announce,
                    now,
                    &mut *fill_entropy,
                    &mut buf,
                ) {
                    CommandedAnnounceWriteOutcome::Written {
                        wire_bytes,
                        ratchet_rotation,
                    } => {
                        #[cfg(feature = "runtime-metrics")]
                        self.record_announce_command(AnnounceCommandOutcome::Succeeded);
                        let fanout = match announce.target {
                            AnnounceTarget::AllInterfaces => FanTarget::All,
                            AnnounceTarget::Interface(interface) => FanTarget::Only(interface),
                        };
                        fan_announce(interfaces, fanout, &buf[..wire_bytes], sink);
                        if ratchet_rotation == RatchetRotation::Minted {
                            sink(EngineReaction::Journaled(Journaled::SelfRatchetRotated {
                                destination: announce.destination,
                            }));
                        }
                        Settlement::AnnounceNow(Ok(()))
                    }
                    CommandedAnnounceWriteOutcome::Rejected { rejection } => {
                        #[cfg(feature = "runtime-metrics")]
                        self.record_announce_command(AnnounceCommandOutcome::Rejected);
                        Settlement::AnnounceNow(Err(AnnounceNowFailure::WriteFailed(
                            AnnounceWriteFailure::Rejected(rejection),
                        )))
                    }
                    CommandedAnnounceWriteOutcome::Failed { failure } => {
                        #[cfg(feature = "runtime-metrics")]
                        self.record_announce_command(AnnounceCommandOutcome::WriteFailed);
                        Settlement::AnnounceNow(Err(AnnounceNowFailure::WriteFailed(
                            AnnounceWriteFailure::Errored(failure),
                        )))
                    }
                };
                settle(sink, id, settlement);
            }
            CommandOutcome::AnnounceRejected { id, rejection } => {
                #[cfg(feature = "runtime-metrics")]
                self.record_announce_command(AnnounceCommandOutcome::Rejected);

                settle(
                    sink,
                    id,
                    Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::OwesSendSinglePacket { id, send } => {
                let mut entropy_bytes = [0u8; SendSinglePacketEntropy::LEN];
                fill_entropy(&mut entropy_bytes);
                let entropy = SendSinglePacketEntropy::new(entropy_bytes);

                let mut buf = [0u8; BROADCAST_MTU];
                match self.write_commanded_send_single_packet(id, &send, now, entropy, &mut buf) {
                    SendSinglePacketWriteOutcome::Written(dispatch) => {
                        fan_frame(
                            interfaces,
                            FanTarget::Only(dispatch.fire_on),
                            &buf[..dispatch.wire_bytes],
                            sink,
                        );
                        if let Some(culled) = dispatch.culled {
                            if matches!(culled.kind, ReceiptKind::SendRequest { .. }) {
                                wake_schedule_changes.resource_deadlines =
                                    self.resource_deadlines_wake();
                            }
                            settle(sink, culled.command_id, culled_settlement(culled.kind));
                        }
                    }
                    SendSinglePacketWriteOutcome::Rejected { rejection, .. } => {
                        settle(
                            sink,
                            id,
                            Settlement::SendSinglePacket(Err(
                                SendSinglePacketFailure::WriteFailed(rejection.into()),
                            )),
                        );
                    }
                    SendSinglePacketWriteOutcome::Failed { failure } => {
                        settle(
                            sink,
                            id,
                            Settlement::SendSinglePacket(Err(
                                SendSinglePacketFailure::WriteFailed(
                                    SendSinglePacketWriteError::Seal(failure),
                                ),
                            )),
                        );
                    }
                }
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            CommandOutcome::SendSinglePacketRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::OwesSendGroup { id, send } => {
                let mut entropy_bytes = [0u8; SendGroupEntropy::LEN];
                fill_entropy(&mut entropy_bytes);
                let entropy = SendGroupEntropy::new(entropy_bytes);

                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_send_group(&send, entropy, &mut buf) {
                    Ok(wire_bytes) => {
                        fan_frame(interfaces, FanTarget::All, &buf[..wire_bytes], sink);
                        Settlement::SendGroup(Ok(()))
                    }
                    Err(error) => Settlement::SendGroup(Err(SendGroupFailure::WriteFailed(error))),
                };
                settle(sink, id, settlement);
            }
            CommandOutcome::SendGroupRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::SendGroup(Err(SendGroupFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::OwesPathRequest { id, request } => {
                let mut buf = [0u8; BROADCAST_MTU];
                match self.write_commanded_path_request(id, &request, now, &mut buf) {
                    PathRequestWriteOutcome::Written { wire_bytes, culled } => {
                        fan_frame(interfaces, FanTarget::All, &buf[..wire_bytes], sink);
                        if let Some(culled) = culled {
                            settle(
                                sink,
                                culled.command_id,
                                Settlement::RequestPath(Err(RequestPathFailure::Culled)),
                            );
                        }
                    }
                    PathRequestWriteOutcome::SerializeFailed(error) => {
                        settle(
                            sink,
                            id,
                            Settlement::RequestPath(Err(RequestPathFailure::WriteFailed(error))),
                        );
                    }
                }
                wake_schedule_changes.path_request_timeouts = self.path_request_timeouts_wake();
            }
            CommandOutcome::OwesLinkRequest { id, establish } => {
                let mut entropy_bytes = [0u8; EstablishLinkEntropy::LEN];
                fill_entropy(&mut entropy_bytes);
                let entropy = EstablishLinkEntropy::new(entropy_bytes);

                let mut buf = [0u8; BROADCAST_MTU];
                match self.write_commanded_link_request(
                    id, &establish, now, entropy, interfaces, &mut buf,
                ) {
                    EstablishLinkWriteOutcome::Written(dispatch) => {
                        fan_frame(
                            interfaces,
                            FanTarget::Only(dispatch.fire_on),
                            &buf[..dispatch.wire_bytes],
                            sink,
                        );
                    }
                    EstablishLinkWriteOutcome::Rejected { rejection } => {
                        settle(
                            sink,
                            id,
                            Settlement::EstablishLink(Err(EstablishLinkFailure::WriteFailed(
                                rejection,
                            ))),
                        );
                    }
                }
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::OwesSendToLink { id, send } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                match self.active_link_interface(&send.link_id) {
                    None => {
                        settle(
                            sink,
                            id,
                            Settlement::SendToLink(Err(SendToLinkFailure::Rejected(
                                SendToLinkRejection::NoSuchLink,
                            ))),
                        );
                    }
                    Some(fire_on) => {
                        let mut wrote = None;
                        let mut fill = |slot: &mut [u8]| match self
                            .write_commanded_send_to_link(id, &send, now, &iv, slot)
                        {
                            Ok(dispatch) => {
                                let wire_bytes = dispatch.wire_bytes;
                                self.links.note_outbound(&send.link_id, now);
                                wrote = Some(Ok(dispatch.culled));
                                Some(wire_bytes)
                            }
                            Err(error) => {
                                wrote = Some(Err(error));
                                None
                            }
                        };
                        sink(EngineReaction::Directive(Directive::EmitFrame {
                            target: fire_on,
                            size_hint: link_data_frame_ceiling(send.payload.len()),
                            fill: &mut fill,
                        }));
                        match wrote {
                            Some(Ok(Some(culled))) => {
                                if matches!(culled.kind, ReceiptKind::SendRequest { .. }) {
                                    wake_schedule_changes.resource_deadlines =
                                        self.resource_deadlines_wake();
                                }
                                settle(sink, culled.command_id, culled_settlement(culled.kind));
                            }
                            Some(Ok(None)) => {}
                            None => settle(
                                sink,
                                id,
                                Settlement::SendToLink(Err(SendToLinkFailure::WriteFailed(
                                    LinkDataError::BufferTooShort,
                                ))),
                            ),
                            Some(Err(SendToLinkWriteError::LinkVanished)) => {
                                settle(
                                    sink,
                                    id,
                                    Settlement::SendToLink(Err(SendToLinkFailure::Rejected(
                                        SendToLinkRejection::NoSuchLink,
                                    ))),
                                );
                            }
                            Some(Err(SendToLinkWriteError::Frame(error))) => {
                                settle(
                                    sink,
                                    id,
                                    Settlement::SendToLink(Err(SendToLinkFailure::WriteFailed(
                                        error,
                                    ))),
                                );
                            }
                        }
                    }
                }
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::SendToLinkRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::SendToLink(Err(SendToLinkFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::OwesSendToChannel { id, send } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                match self.active_link_interface(&send.link_id) {
                    None => {
                        settle(
                            sink,
                            id,
                            Settlement::SendToChannel(Err(SendToChannelFailure::Rejected(
                                SendToChannelRejection::NoSuchLink,
                            ))),
                        );
                    }
                    Some(fire_on) => {
                        let mut wrote = None;
                        let mut fill = |slot: &mut [u8]| match self
                            .write_commanded_send_to_channel(id, &send, now, &iv, slot)
                        {
                            Ok(dispatch) => {
                                self.links.note_outbound(&send.link_id, now);
                                wrote = Some(Ok(()));
                                Some(dispatch.wire_bytes)
                            }
                            Err(error) => {
                                wrote = Some(Err(error));
                                None
                            }
                        };
                        sink(EngineReaction::Directive(Directive::EmitFrame {
                            target: fire_on,
                            size_hint: link_data_frame_ceiling(
                                CHANNEL_ENVELOPE_HEADER_LEN + send.body.len(),
                            ),
                            fill: &mut fill,
                        }));
                        match wrote {
                            Some(Ok(())) => {}
                            Some(Err(error)) => settle(
                                sink,
                                id,
                                Settlement::SendToChannel(Err(send_to_channel_failure(error))),
                            ),
                            None => settle(
                                sink,
                                id,
                                Settlement::SendToChannel(Err(SendToChannelFailure::WriteFailed(
                                    LinkDataError::BufferTooShort,
                                ))),
                            ),
                        }
                    }
                }
                wake_schedule_changes.channel_timeouts = self.channel_timeouts_wake();
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::SendToChannelRejected { id, failure } => {
                settle(sink, id, Settlement::SendToChannel(Err(failure)));
            }
            CommandOutcome::OwesIdentify { id, identify } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_identify(&identify, &iv, &mut buf) {
                    Ok(dispatch) => {
                        self.links.note_outbound(&identify.link_id, now);
                        fan_frame(
                            interfaces,
                            FanTarget::Only(dispatch.fire_on),
                            &buf[..dispatch.wire_bytes],
                            sink,
                        );
                        Settlement::Identify(Ok(()))
                    }
                    Err(IdentifyWriteError::LinkVanished) => Settlement::Identify(Err(
                        IdentifyFailure::Rejected(IdentifyRejection::NoSuchLink),
                    )),
                    Err(IdentifyWriteError::IdentityVanished) => Settlement::Identify(Err(
                        IdentifyFailure::Rejected(IdentifyRejection::IdentityNotHeld),
                    )),
                    Err(IdentifyWriteError::BufferTooShort) => {
                        Settlement::Identify(Err(IdentifyFailure::WriteFailed))
                    }
                };
                settle(sink, id, settlement);
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::OwesSendRequest { id, request } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                match self.active_link_interface(&request.link_id) {
                    None => {
                        settle(
                            sink,
                            id,
                            Settlement::SendRequest(Err(SendRequestFailure::Rejected(
                                SendRequestRejection::NoSuchLink,
                            ))),
                        );
                    }
                    Some(fire_on) => {
                        let mut wrote = None;
                        let mut fill = |slot: &mut [u8]| match self
                            .write_commanded_send_request(id, &request, now, &iv, slot)
                        {
                            Ok(dispatch) => {
                                let wire_bytes = dispatch.wire_bytes;
                                self.links.note_outbound(&request.link_id, now);
                                wrote = Some(Ok(dispatch.culled));
                                Some(wire_bytes)
                            }
                            Err(error) => {
                                wrote = Some(Err(error));
                                None
                            }
                        };
                        sink(EngineReaction::Directive(Directive::EmitFrame {
                            target: fire_on,
                            size_hint: link_data_frame_ceiling(
                                REQUEST_WIRE_OVERHEAD + request.data.len(),
                            ),
                            fill: &mut fill,
                        }));
                        match wrote {
                            Some(Ok(Some(culled))) => {
                                if matches!(culled.kind, ReceiptKind::SendRequest { .. }) {
                                    wake_schedule_changes.resource_deadlines =
                                        self.resource_deadlines_wake();
                                }
                                settle(sink, culled.command_id, culled_settlement(culled.kind));
                            }
                            Some(Ok(None)) => {}
                            Some(Err(LinkRequestWriteError::LinkVanished)) => {
                                settle(
                                    sink,
                                    id,
                                    Settlement::SendRequest(Err(SendRequestFailure::Rejected(
                                        SendRequestRejection::NoSuchLink,
                                    ))),
                                );
                            }
                            Some(Err(
                                LinkRequestWriteError::PayloadTooLong
                                | LinkRequestWriteError::BufferTooShort,
                            )) => {
                                settle(
                                    sink,
                                    id,
                                    Settlement::SendRequest(Err(SendRequestFailure::WriteFailed)),
                                );
                            }
                            None => settle(
                                sink,
                                id,
                                Settlement::SendRequest(Err(SendRequestFailure::WriteFailed)),
                            ),
                        }
                    }
                }
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::SendRequestRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::SendRequest(Err(SendRequestFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::OwesRespond { id, respond } => {
                let data_len = match &respond.payload {
                    crate::engine::RespondPayload::Packed(data) => data.len(),
                    crate::engine::RespondPayload::StaticBytes(_) => 0,
                    #[cfg(any(feature = "large-static-responses", test))]
                    crate::engine::RespondPayload::StaticFile { .. } => 0,
                };
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let settlement = match self.active_link_interface(&respond.link_id) {
                    None => Settlement::Respond(Err(RespondFailure::Rejected(
                        RespondRejection::NoSuchLink,
                    ))),
                    Some(fire_on) => {
                        let mut wrote = None;
                        let mut fill = |slot: &mut [u8]| match self
                            .write_commanded_respond(&respond, &iv, slot)
                        {
                            Ok(dispatch) => {
                                let wire_bytes = dispatch.wire_bytes;
                                self.links.note_outbound(&respond.link_id, now);
                                wrote = Some(Ok(()));
                                Some(wire_bytes)
                            }
                            Err(error) => {
                                wrote = Some(Err(error));
                                None
                            }
                        };
                        sink(EngineReaction::Directive(Directive::EmitFrame {
                            target: fire_on,
                            size_hint: link_data_frame_ceiling(
                                RESPONSE_WIRE_OVERHEAD + response_data_wire_len(data_len),
                            ),
                            fill: &mut fill,
                        }));
                        match wrote {
                            Some(Ok(())) => Settlement::Respond(Ok(())),
                            Some(Err(LinkRequestWriteError::LinkVanished)) => Settlement::Respond(
                                Err(RespondFailure::Rejected(RespondRejection::NoSuchLink)),
                            ),
                            Some(Err(
                                LinkRequestWriteError::PayloadTooLong
                                | LinkRequestWriteError::BufferTooShort,
                            ))
                            | None => Settlement::Respond(Err(RespondFailure::WriteFailed)),
                        }
                    }
                };
                settle(sink, id, settlement);
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::OwesResourceResponse { id, respond } => {
                wake_schedule_changes =
                    self.ingest_send_static_response_into(id, &respond, now, fill_entropy, sink);
            }
            CommandOutcome::RespondRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::Respond(Err(RespondFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::IdentifyRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::Identify(Err(IdentifyFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::OwesLinkClose { id, close } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_owed_link_close(&close.link_id, &iv, &mut buf) {
                    Ok(dispatch) => {
                        if let Some(fire_on) = dispatch.fire_on {
                            fan_frame(
                                interfaces,
                                FanTarget::Only(fire_on),
                                &buf[..dispatch.wire_bytes],
                                sink,
                            );
                        }
                        Settlement::CloseLink(Ok(()))
                    }
                    Err(_) => Settlement::CloseLink(Err(CloseLinkFailure::WriteFailed)),
                };
                settle(sink, id, settlement);
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
                wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
            }
            CommandOutcome::CloseLinkRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::CloseLink(Err(CloseLinkFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::ResourceStrategySet { id } => {
                settle(sink, id, Settlement::SetResourceStrategy(Ok(())));
            }
            CommandOutcome::SetResourceStrategyRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::SetResourceStrategy(Err(SetResourceStrategyFailure::Rejected(
                        rejection,
                    ))),
                );
            }
            CommandOutcome::RequesterAllowed { id } => {
                settle(sink, id, Settlement::AllowRequester(Ok(())));
            }
            CommandOutcome::AllowRequesterRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::AllowRequester(Err(AllowRequesterFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::EstablishLinkRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(rejection))),
                );
            }
        }
        wake_schedule_changes
    }

    pub fn complete_send_single_packet_deferred(
        &mut self,
        owed: EncryptOwed,
        ephemeral_public: X25519PublicKey,
        shared: X25519SharedSecret,
        interfaces: AttachedInterfaces<'_>,
        buf: &mut [u8],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let id = owed.command_id;
        let mut culled_request = false;
        match self.finish_send_single_packet_deferred(owed, ephemeral_public, shared, buf) {
            FinishSendSinglePacketOutcome::Written(dispatch) => {
                fan_frame(
                    interfaces,
                    FanTarget::Only(dispatch.fire_on),
                    &buf[..dispatch.wire_bytes],
                    sink,
                );
                if let Some(culled) = dispatch.culled {
                    culled_request = matches!(culled.kind, ReceiptKind::SendRequest { .. });
                    settle(sink, culled.command_id, culled_settlement(culled.kind));
                }
            }
            FinishSendSinglePacketOutcome::Failed(error) => {
                settle(
                    sink,
                    id,
                    Settlement::SendSinglePacket(Err(SendSinglePacketFailure::WriteFailed(error))),
                );
            }
        }
        let mut wake = WakeSchedules::UNCHANGED;
        wake.receipt_timeouts = self.receipt_timeouts_wake();
        if culled_request {
            wake.resource_deadlines = self.resource_deadlines_wake();
        }
        wake
    }
}

fn send_to_channel_failure(error: SendToChannelWriteError) -> SendToChannelFailure {
    match error {
        SendToChannelWriteError::LinkVanished => {
            SendToChannelFailure::Rejected(SendToChannelRejection::NoSuchLink)
        }
        SendToChannelWriteError::Untrackable => SendToChannelFailure::Untrackable,
        SendToChannelWriteError::WindowFull => SendToChannelFailure::WindowFull,
        SendToChannelWriteError::Frame(error) => SendToChannelFailure::WriteFailed(error),
    }
}
