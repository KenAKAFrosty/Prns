use crate::crypto::{X25519PublicKey, X25519SharedSecret};
use crate::engine::{
    AllowRequesterFailure, CloseLinkFailure, IdentifyFailure, IdentifyRejection, RespondFailure,
    RespondRejection, SendRequestFailure, SendRequestRejection, SendToChannelFailure,
    SendToChannelRejection, SendToLinkFailure, SendToLinkRejection, SetResourceStrategyFailure,
};
use crate::engine::{
    AnnounceNowFailure, AnnounceTarget, CommandOutcome, CommandedAnnounceWriteOutcome, Directive,
    EncryptOwed, EngineReaction, EngineState, EstablishLinkFailure, EstablishLinkWriteOutcome,
    FanTarget, FinishSendSinglePacketOutcome, InstantMillis, IssuedCommand, Journaled, PathFound,
    PathRequestWriteOutcome, RatchetEntropy, RequestPathFailure, SendGroupFailure,
    SendSinglePacketEntropy, SendSinglePacketFailure, SendSinglePacketWriteOutcome, Settlement,
    WakeSchedules, WriteSendSinglePacketError,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{InterfaceConfig, InterfaceId, InterfaceKind, InterfaceMode};
use crate::routing::announce::AnnounceEntropy;
use crate::routing::delivery::receipts::{CulledReceipt, ReceiptKind};
use crate::routing::links::channel::send::SendToChannelWriteError;
use crate::routing::links::channel::CHANNEL_ENVELOPE_HEADER_LEN;
use crate::routing::links::data::{link_data_frame_ceiling, SendToLinkWriteError};
use crate::routing::links::establish::EstablishLinkEntropy;
use crate::routing::links::identify::IdentifyWriteError;
use crate::routing::links::request::{
    LinkRequestWriteError, REQUEST_WIRE_OVERHEAD, RESPONSE_WIRE_OVERHEAD,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;

impl<S: StorageLayout> EngineState<S> {
    /// Resolved before grant-first emission; the write re-resolves and stays the source of truth.
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
        interfaces: &[InterfaceConfig],
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
                let mut announce_entropy_bytes = [0u8; AnnounceEntropy::LEN];
                fill_entropy(&mut announce_entropy_bytes);
                let announce_entropy = AnnounceEntropy::new(announce_entropy_bytes);
                let mut ratchet_bytes = [0u8; RatchetEntropy::LEN];
                fill_entropy(&mut ratchet_bytes);
                let ratchet = RatchetEntropy::new(ratchet_bytes);

                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_announce(
                    &announce,
                    now,
                    announce_entropy,
                    ratchet,
                    &mut buf,
                ) {
                    CommandedAnnounceWriteOutcome::Written { len, .. } => {
                        let fanout = match announce.target {
                            AnnounceTarget::AllInterfaces => FanTarget::All,
                            AnnounceTarget::Interface(interface) => FanTarget::Only(interface),
                        };
                        fan_self_announce(interfaces, fanout, &buf[..len], sink);
                        Settlement::AnnounceNow(Ok(()))
                    }
                    CommandedAnnounceWriteOutcome::Rejected { rejection, .. } => {
                        Settlement::AnnounceNow(Err(AnnounceNowFailure::WriteFailed(
                            rejection.into(),
                        )))
                    }
                    CommandedAnnounceWriteOutcome::Failed { failure, .. } => {
                        Settlement::AnnounceNow(Err(AnnounceNowFailure::WriteFailed(
                            failure.into(),
                        )))
                    }
                };
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement,
                }));
            }
            CommandOutcome::AnnounceRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(
                        rejection,
                    ))),
                }));
            }
            CommandOutcome::OwesSendSinglePacket { id, send } => {
                let mut entropy_bytes = [0u8; SendSinglePacketEntropy::LEN];
                fill_entropy(&mut entropy_bytes);
                let entropy = SendSinglePacketEntropy::new(entropy_bytes);

                let mut buf = [0u8; BROADCAST_MTU];
                match self.write_commanded_send_single_packet(id, &send, now, entropy, &mut buf) {
                    SendSinglePacketWriteOutcome::Written(dispatch) => {
                        fan_self_originated(
                            interfaces,
                            FanTarget::Only(dispatch.fire_on),
                            &buf[..dispatch.wire_len],
                            sink,
                        );
                        if let Some(culled) = dispatch.culled {
                            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                id: culled.command_id,
                                settlement: culled_settlement(culled),
                            }));
                        }
                    }
                    SendSinglePacketWriteOutcome::Rejected { rejection, .. } => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendSinglePacket(Err(
                                SendSinglePacketFailure::WriteFailed(rejection.into()),
                            )),
                        }));
                    }
                    SendSinglePacketWriteOutcome::Failed { failure } => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendSinglePacket(Err(
                                SendSinglePacketFailure::WriteFailed(
                                    WriteSendSinglePacketError::Seal(failure),
                                ),
                            )),
                        }));
                    }
                }
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            CommandOutcome::SendSinglePacketRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendSinglePacket(Err(
                        SendSinglePacketFailure::Rejected(rejection),
                    )),
                }));
            }
            CommandOutcome::OwesSendGroup { id, send } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);

                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_send_group(&send, &iv, &mut buf) {
                    Ok(wire_len) => {
                        fan_self_originated(interfaces, FanTarget::All, &buf[..wire_len], sink);
                        Settlement::SendGroup(Ok(()))
                    }
                    Err(_) => Settlement::SendGroup(Err(SendGroupFailure::WriteFailed)),
                };
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement,
                }));
            }
            CommandOutcome::SendGroupRejected { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendGroup(Err(SendGroupFailure::NoGroupKey)),
                }));
            }
            CommandOutcome::OwesPathRequest { id, request } => {
                let mut buf = [0u8; BROADCAST_MTU];
                match self.write_commanded_path_request(id, &request, now, &mut buf) {
                    PathRequestWriteOutcome::AlreadyReachable { hops } => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::RequestPath(Ok(PathFound {
                                hops: crate::units::HopCount(hops),
                            })),
                        }));
                    }
                    PathRequestWriteOutcome::Written { wire_len, culled } => {
                        fan_self_originated(interfaces, FanTarget::All, &buf[..wire_len], sink);
                        if let Some(culled) = culled {
                            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                id: culled.command_id,
                                settlement: Settlement::RequestPath(Err(
                                    RequestPathFailure::Culled,
                                )),
                            }));
                        }
                    }
                    PathRequestWriteOutcome::SerializeFailed(error) => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::RequestPath(Err(
                                RequestPathFailure::WriteFailed(error),
                            )),
                        }));
                    }
                }
                wake_schedule_changes.path_request_timeout = self.path_request_timeout_wake();
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
                        fan_self_originated(
                            interfaces,
                            FanTarget::Only(dispatch.fire_on),
                            &buf[..dispatch.wire_len],
                            sink,
                        );
                    }
                    EstablishLinkWriteOutcome::Failed { failure } => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::EstablishLink(Err(
                                EstablishLinkFailure::WriteFailed(failure),
                            )),
                        }));
                    }
                }
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::OwesSendToLink { id, send } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                match self.active_link_interface(&send.link_id) {
                    None => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendToLink(Err(SendToLinkFailure::Rejected(
                                SendToLinkRejection::NoSuchLink,
                            ))),
                        }));
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
                                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                    id: culled.command_id,
                                    settlement: culled_settlement(culled),
                                }));
                            }
                            Some(Ok(None)) | None => {}
                            Some(Err(SendToLinkWriteError::LinkVanished)) => {
                                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                    id,
                                    settlement: Settlement::SendToLink(Err(
                                        SendToLinkFailure::Rejected(
                                            SendToLinkRejection::NoSuchLink,
                                        ),
                                    )),
                                }));
                            }
                            Some(Err(SendToLinkWriteError::Frame(error))) => {
                                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                    id,
                                    settlement: Settlement::SendToLink(Err(
                                        SendToLinkFailure::WriteFailed(error),
                                    )),
                                }));
                            }
                        }
                    }
                }
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            CommandOutcome::SendToLinkRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendToLink(Err(SendToLinkFailure::Rejected(rejection))),
                }));
            }
            CommandOutcome::OwesSendToChannel { id, send } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                match self.active_link_interface(&send.link_id) {
                    None => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendToChannel(Err(
                                SendToChannelFailure::Rejected(SendToChannelRejection::NoSuchLink),
                            )),
                        }));
                    }
                    Some(fire_on) => {
                        let mut wrote = None;
                        let mut fill = |slot: &mut [u8]| match self
                            .write_commanded_send_to_channel(id, &send, now, &iv, slot)
                        {
                            Ok(dispatch) => Some(dispatch.wire_len),
                            Err(error) => {
                                wrote = Some(error);
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
                        if let Some(error) = wrote {
                            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                id,
                                settlement: Settlement::SendToChannel(Err(
                                    send_to_channel_failure(error),
                                )),
                            }));
                        }
                    }
                }
                wake_schedule_changes.channel_timeouts = self.channel_timeouts_wake();
            }
            CommandOutcome::SendToChannelRejected { id, failure } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendToChannel(Err(failure)),
                }));
            }
            CommandOutcome::OwesIdentify { id, identify } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_identify(&identify, &iv, &mut buf) {
                    Ok(dispatch) => {
                        fan_self_originated(
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
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement,
                }));
            }
            CommandOutcome::OwesSendRequest { id, request } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                match self.active_link_interface(&request.link_id) {
                    None => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendRequest(Err(SendRequestFailure::Rejected(
                                SendRequestRejection::NoSuchLink,
                            ))),
                        }));
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
                                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                    id: culled.command_id,
                                    settlement: culled_settlement(culled),
                                }));
                            }
                            Some(Ok(None)) | None => {}
                            Some(Err(LinkRequestWriteError::LinkVanished)) => {
                                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                    id,
                                    settlement: Settlement::SendRequest(Err(
                                        SendRequestFailure::Rejected(
                                            SendRequestRejection::NoSuchLink,
                                        ),
                                    )),
                                }));
                            }
                            Some(Err(
                                LinkRequestWriteError::PayloadTooLong
                                | LinkRequestWriteError::BufferTooShort,
                            )) => {
                                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                    id,
                                    settlement: Settlement::SendRequest(Err(
                                        SendRequestFailure::WriteFailed,
                                    )),
                                }));
                            }
                        }
                    }
                }
                wake_schedule_changes.receipt_timeouts = self.receipt_timeouts_wake();
            }
            CommandOutcome::SendRequestRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendRequest(Err(SendRequestFailure::Rejected(
                        rejection,
                    ))),
                }));
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
                                RESPONSE_WIRE_OVERHEAD + respond.data.len().max(1),
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
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement,
                }));
            }
            CommandOutcome::RespondRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::Respond(Err(RespondFailure::Rejected(rejection))),
                }));
            }
            CommandOutcome::IdentifyRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::Identify(Err(IdentifyFailure::Rejected(rejection))),
                }));
            }
            CommandOutcome::OwesLinkClose { id, close } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_owed_link_close(&close.link_id, &iv, &mut buf) {
                    Ok(dispatch) => {
                        if let Some(fire_on) = dispatch.fire_on {
                            fan_self_originated(
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
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement,
                }));
                wake_schedule_changes.link_deadlines = self.link_deadlines_wake();
            }
            CommandOutcome::CloseLinkRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::CloseLink(Err(CloseLinkFailure::Rejected(rejection))),
                }));
            }
            CommandOutcome::ResourceStrategySet { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SetResourceStrategy(Ok(())),
                }));
            }
            CommandOutcome::SetResourceStrategyRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SetResourceStrategy(Err(
                        SetResourceStrategyFailure::Rejected(rejection),
                    )),
                }));
            }
            CommandOutcome::RequesterAllowed { id } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::AllowRequester(Ok(())),
                }));
            }
            CommandOutcome::AllowRequesterRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::AllowRequester(Err(AllowRequesterFailure::Rejected(
                        rejection,
                    ))),
                }));
            }
            CommandOutcome::EstablishLinkRejected { id, rejection } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(
                        rejection,
                    ))),
                }));
            }
            CommandOutcome::RpcQueryRead { id, result } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::RpcQuery(result),
                }));
            }
        }
        wake_schedule_changes
    }

    pub fn complete_send_single_packet_deferred(
        &mut self,
        owed: EncryptOwed,
        ephemeral_public: X25519PublicKey,
        shared: X25519SharedSecret,
        interfaces: &[InterfaceConfig],
        buf: &mut [u8],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let id = owed.command_id;
        match self.finish_send_single_packet_deferred(owed, ephemeral_public, shared, buf) {
            FinishSendSinglePacketOutcome::Written(dispatch) => {
                fan_self_originated(
                    interfaces,
                    FanTarget::Only(dispatch.fire_on),
                    &buf[..dispatch.wire_len],
                    sink,
                );
                if let Some(culled) = dispatch.culled {
                    sink(EngineReaction::Journaled(Journaled::CommandSettled {
                        id: culled.command_id,
                        settlement: culled_settlement(culled),
                    }));
                }
            }
            FinishSendSinglePacketOutcome::Failed(error) => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendSinglePacket(Err(
                        SendSinglePacketFailure::WriteFailed(error),
                    )),
                }));
            }
        }
        let mut wake = WakeSchedules::UNCHANGED;
        wake.receipt_timeouts = self.receipt_timeouts_wake();
        wake
    }
}

fn culled_settlement(culled: CulledReceipt) -> Settlement {
    match culled.kind {
        ReceiptKind::SendSinglePacket => {
            Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Culled))
        }
        ReceiptKind::SendToLink => Settlement::SendToLink(Err(SendToLinkFailure::Culled)),
        ReceiptKind::SendRequest => Settlement::SendRequest(Err(SendRequestFailure::Culled)),
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

pub(crate) fn fan_self_originated(
    interfaces: &[InterfaceConfig],
    fanout: FanTarget,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    fan_self(interfaces, fanout, bytes, false, sink);
}

/// Withheld from an access-point interface — RNS Transport.py:1193.
pub(crate) fn fan_self_announce(
    interfaces: &[InterfaceConfig],
    fanout: FanTarget,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    fan_self(interfaces, fanout, bytes, true, sink);
}

fn fan_self(
    interfaces: &[InterfaceConfig],
    fanout: FanTarget,
    bytes: &[u8],
    withhold_access_point: bool,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    let mut fleets_emitted: u128 = 0;
    for config in interfaces {
        let targeted = match fanout {
            FanTarget::All => true,
            FanTarget::Only(id) => config.id == id,
            FanTarget::AllExcept(id) => config.id != id,
        };
        if !targeted || !config.capabilities.allows_transmit() {
            continue;
        }
        match config.id.kind().and_then(InterfaceKind::supervisor_kind) {
            Some(supervisor) => {
                let bit = 1u128 << (supervisor as u8);
                if fleets_emitted & bit == 0 {
                    fleets_emitted |= bit;
                    sink(EngineReaction::Directive(Directive::Broadcast {
                        supervisor,
                        fan: fanout,
                        bytes,
                    }));
                }
            }
            None => {
                if withhold_access_point && config.mode == InterfaceMode::AccessPoint {
                    continue;
                }
                sink(EngineReaction::Directive(Directive::Send {
                    target: config.id,
                    bytes,
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::routable_descriptor;

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn a_self_announce_is_withheld_from_an_access_point_interface() {
        let view = [
            routable_descriptor(iface(0x01)),
            InterfaceConfig {
                mode: InterfaceMode::AccessPoint,
                ..routable_descriptor(iface(0x02))
            },
            InterfaceConfig {
                mode: InterfaceMode::Roaming,
                ..routable_descriptor(iface(0x03))
            },
        ];

        let mut targets = std::vec::Vec::new();
        fan_self_announce(&view, FanTarget::All, &[0xAB], &mut |reaction| {
            if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                targets.push(target);
            }
        });

        assert_eq!(
            targets,
            std::vec![iface(0x01), iface(0x03)],
            "a full and a roaming interface carry our own announce; the access point does not",
        );
    }
}
