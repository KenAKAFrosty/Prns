use crate::crypto::ratchets::RatchetRotation;
use crate::crypto::{X25519PublicKey, X25519SharedSecret};
#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceCommandOutcome;
use crate::engine::{
    AllowRequesterFailure, AnnounceNowFailure, AnnounceTarget, AnnounceWriteFailure,
    CloseLinkFailure, CommandId, CommandOutcome, CommandedAnnounceWriteOutcome, Directive,
    EncryptOwed, EngineReaction, EngineState, EstablishLinkFailure, EstablishLinkWriteOutcome,
    FanTarget, FinishSendSinglePacketOutcome, IdentifyFailure, IdentifyRejection, InstantMillis,
    IssuedCommand, Journaled, PathRequestWriteOutcome, RequestPathFailure, RespondFailure,
    RespondRejection, SendGroupEntropy, SendGroupFailure, SendRequestFailure, SendRequestRejection,
    SendSinglePacketEntropy, SendSinglePacketFailure, SendSinglePacketWriteError,
    SendSinglePacketWriteOutcome, SendToChannelFailure, SendToChannelRejection, SendToLinkFailure,
    SendToLinkRejection, SetResourceStrategyFailure, Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::{InterfaceId, InterfaceKind, InterfaceMode};
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
    /// Resolves the link's interface only so the grant-first directive can name its target; the reactor must know which lane to offer a slot from before `fill` runs.
    /// The write inside `fill` looks the link up again and that second lookup is the authority: a link gone by then fails there as `LinkVanished`.
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
        //Match arms that set or clear a deadline will write that lane's recompute into this delta as they go. Most leave it UNCHANGED.
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
                        wire_len,
                        ratchet_rotation,
                    } => {
                        #[cfg(feature = "runtime-metrics")]
                        self.record_announce_command(AnnounceCommandOutcome::Succeeded);
                        let fanout = match announce.target {
                            AnnounceTarget::AllInterfaces => FanTarget::All,
                            AnnounceTarget::Interface(interface) => FanTarget::Only(interface),
                        };
                        fan_announce(interfaces, fanout, &buf[..wire_len], sink);
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
                            &buf[..dispatch.wire_len],
                            sink,
                        );
                        if let Some(culled) = dispatch.culled {
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
                    Ok(wire_len) => {
                        fan_frame(interfaces, FanTarget::All, &buf[..wire_len], sink);
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
                    PathRequestWriteOutcome::Written { wire_len, culled } => {
                        fan_frame(interfaces, FanTarget::All, &buf[..wire_len], sink);
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
                            &buf[..dispatch.wire_len],
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
                                let wire_len = dispatch.wire_len;
                                wrote = Some(Ok(dispatch.culled));
                                Some(wire_len)
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
                                settle(sink, culled.command_id, culled_settlement(culled.kind));
                            }
                            Some(Ok(None)) => {}
                            //A fill the host never ran settles like a discard-slot grant: nothing went out, so the write failed.
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
                                wrote = Some(Ok(()));
                                Some(dispatch.wire_len)
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
                            //A fill the host never ran settles like a discard-slot grant: nothing went out, so the write failed.
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
                        fan_frame(
                            interfaces,
                            FanTarget::Only(dispatch.fire_on),
                            &buf[..dispatch.wire_len],
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
                                let wire_len = dispatch.wire_len;
                                wrote = Some(Ok(dispatch.culled));
                                Some(wire_len)
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
                            //A fill the host never ran settles like a discard-slot grant: nothing went out, so the write failed.
                            None => settle(
                                sink,
                                id,
                                Settlement::SendRequest(Err(SendRequestFailure::WriteFailed)),
                            ),
                        }
                    }
                }
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            CommandOutcome::SendRequestRejected { id, rejection } => {
                settle(
                    sink,
                    id,
                    Settlement::SendRequest(Err(SendRequestFailure::Rejected(rejection))),
                );
            }
            CommandOutcome::OwesRespond { id, respond } => {
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
                                let wire_len = dispatch.wire_len;
                                wrote = Some(Ok(()));
                                Some(wire_len)
                            }
                            Err(error) => {
                                wrote = Some(Err(error));
                                None
                            }
                        };
                        sink(EngineReaction::Directive(Directive::EmitFrame {
                            target: fire_on,
                            size_hint: link_data_frame_ceiling(
                                RESPONSE_WIRE_OVERHEAD + response_data_wire_len(respond.data.len()),
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
                                &buf[..dispatch.wire_len],
                                sink,
                            );
                        }
                        Settlement::CloseLink(Ok(()))
                    }
                    Err(_) => Settlement::CloseLink(Err(CloseLinkFailure::WriteFailed)),
                };
                settle(sink, id, settlement);
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
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
            CommandOutcome::InspectionRead { id, result } => {
                settle(sink, id, Settlement::Inspection(result));
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
        match self.finish_send_single_packet_deferred(owed, ephemeral_public, shared, buf) {
            FinishSendSinglePacketOutcome::Written(dispatch) => {
                fan_frame(
                    interfaces,
                    FanTarget::Only(dispatch.fire_on),
                    &buf[..dispatch.wire_len],
                    sink,
                );
                if let Some(culled) = dispatch.culled {
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
        wake
    }
}

pub(crate) fn settle(
    sink: &mut impl FnMut(EngineReaction<'_>),
    id: CommandId,
    settlement: Settlement,
) {
    sink(EngineReaction::Journaled(Journaled::CommandSettled {
        id,
        settlement,
    }));
}

fn culled_settlement(kind: ReceiptKind) -> Settlement {
    match kind {
        ReceiptKind::SendSinglePacket => {
            Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Culled))
        }
        ReceiptKind::SendToLink => Settlement::SendToLink(Err(SendToLinkFailure::Culled)),
        ReceiptKind::SendRequest => Settlement::SendRequest(Err(SendRequestFailure::Culled)),
    }
}

pub(crate) fn timeout_settlement(kind: ReceiptKind) -> Settlement {
    match kind {
        ReceiptKind::SendSinglePacket => {
            Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Timeout))
        }
        ReceiptKind::SendToLink => Settlement::SendToLink(Err(SendToLinkFailure::Timeout)),
        ReceiptKind::SendRequest => Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
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

pub(crate) fn fan_frame(
    interfaces: AttachedInterfaces<'_>,
    fanout: FanTarget,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    fan(interfaces, fanout, bytes, FanKind::Frame, sink);
}

pub(crate) fn fan_announce(
    interfaces: AttachedInterfaces<'_>,
    fanout: FanTarget,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    fan(interfaces, fanout, bytes, FanKind::Announce, sink);
}

#[derive(Clone, Copy)]
enum FanKind {
    Frame,
    Announce,
}

fn fan(
    interfaces: AttachedInterfaces<'_>,
    fanout: FanTarget,
    bytes: &[u8],
    emission: FanKind,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    //One SendToFleet per supervisor kind replaces a Send per member: the supervisor fans to its own fleet, so a second member of the same kind must not trigger another emission.
    //The u128 is a seen-bitmask indexed by the supervisor kind's discriminant.
    let mut fleets_emitted: u128 = 0;
    for descriptor in interfaces {
        if !descriptor.capabilities.allows_transmit() {
            continue;
        }
        let targeted = match fanout {
            FanTarget::All => true,
            FanTarget::Only(id) => descriptor.id == id,
            FanTarget::AllExcept(id) => descriptor.id != id,
        };
        if !targeted {
            continue;
        }
        match descriptor
            .id
            .kind()
            .and_then(InterfaceKind::supervisor_kind)
        {
            Some(supervisor) => {
                debug_assert!(
                    (supervisor as u8) < 128,
                    "InterfaceKind discriminants must stay below 128 to index the fleet seen-bitmask",
                );
                let bit = 1u128 << (supervisor as u8);
                if fleets_emitted & bit == 0 {
                    fleets_emitted |= bit;
                    match emission {
                        FanKind::Frame => {
                            sink(EngineReaction::Directive(Directive::SendToFleet {
                                supervisor,
                                fan: fanout,
                                bytes,
                            }));
                        }
                        FanKind::Announce => {
                            #[cfg(feature = "runtime-metrics")]
                            sink(EngineReaction::Directive(
                                Directive::SendMeasuredLocalAnnounceToFleet {
                                    supervisor,
                                    fan: fanout,
                                    bytes,
                                },
                            ));

                            #[cfg(not(feature = "runtime-metrics"))]
                            sink(EngineReaction::Directive(Directive::SendToFleet {
                                supervisor,
                                fan: fanout,
                                bytes,
                            }));
                        }
                    }
                }
            }
            None => {
                if matches!(emission, FanKind::Announce)
                    && descriptor.mode == InterfaceMode::AccessPoint
                // Withheld from an access-point interface, matching RNS 1.3.5 `Transport.outbound`.
                {
                    continue;
                }
                match emission {
                    FanKind::Frame => sink(EngineReaction::Directive(Directive::Send {
                        target: descriptor.id,
                        bytes,
                    })),
                    FanKind::Announce => {
                        #[cfg(feature = "runtime-metrics")]
                        sink(EngineReaction::Directive(
                            Directive::SendMeasuredLocalAnnounce {
                                target: descriptor.id,
                                bytes,
                            },
                        ));
                        #[cfg(not(feature = "runtime-metrics"))]
                        sink(EngineReaction::Directive(Directive::Send {
                            target: descriptor.id,
                            bytes,
                        }));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::routable_descriptor;
    use crate::interfaces::InterfaceDescriptor;

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn a_self_announce_is_withheld_from_an_access_point_interface() {
        let interfaces = [
            routable_descriptor(iface(0x01)),
            InterfaceDescriptor {
                mode: InterfaceMode::AccessPoint,
                ..routable_descriptor(iface(0x02))
            },
            InterfaceDescriptor {
                mode: InterfaceMode::Roaming,
                ..routable_descriptor(iface(0x03))
            },
        ];

        let mut targets = std::vec::Vec::new();
        fan_announce(
            AttachedInterfaces::new(&interfaces),
            FanTarget::All,
            &[0xAB],
            &mut |reaction| match reaction {
                #[cfg(feature = "runtime-metrics")]
                EngineReaction::Directive(Directive::SendMeasuredLocalAnnounce {
                    target, ..
                }) => {
                    targets.push(target);
                }
                #[cfg(not(feature = "runtime-metrics"))]
                EngineReaction::Directive(Directive::Send { target, .. }) => {
                    targets.push(target);
                }
                _ => {}
            },
        );

        assert_eq!(
            targets,
            std::vec![iface(0x01), iface(0x03)],
            "a full and a roaming interface carry our own announce; the access point does not",
        );
    }
}
