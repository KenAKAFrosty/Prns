use crate::engine::{
    AnnounceNowFailure, AnnounceTarget, CommandOutcome, CommandedAnnounceWriteOutcome, Directive,
    EngineReaction, EngineState, EstablishLinkFailure, EstablishLinkWriteOutcome, InstantMillis,
    IssuedCommand, Journaled, PathFound, PathRequestWriteOutcome, RatchetEntropy,
    RequestPathFailure, SendGroupFailure, SendSingleEntropy, SendSingleFailure,
    SendSingleWriteOutcome, Settlement, WakeSchedules, WriteSendSingleError,
};
use crate::engine::{CloseLinkFailure, SendLinkError, SendLinkFailure};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::routing::announce::AnnounceEntropy;
use crate::routing::links::data::SendLinkWriteError;
use crate::routing::links::establish::EstablishLinkEntropy;
use crate::routing::storage::EngineStorage;
use crate::wire::BROADCAST_MTU;

impl<S: EngineStorage> EngineState<S> {
    /// Run one app command and stream its result to `sink`: the `Directive`s it fans (an
    /// announce, a send, a path request — each to its self-originated targets) and a
    /// `Journaled` `CommandSettled` for whatever resolves at emission — an immediate
    /// rejection, an already-known route, or a command the table culled to make room. A
    /// send or a fresh path request that resolves later settles through the inbound or
    /// timer edge instead. `fill_entropy` is pulled only when an announce or a send is
    /// actually sealed. Returns a [`WakeSchedules`] delta: a send arms the send-timeout lane,
    /// a fresh path request the path-timeout lane; an announce tunnels straight through and
    /// moves nothing the reactor schedules.
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
                        let only = match announce.target {
                            AnnounceTarget::AllInterfaces => None,
                            AnnounceTarget::Interface(interface) => Some(interface),
                        };
                        fan_self_originated(interfaces, only, &buf[..len], sink);
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
            CommandOutcome::AnnounceRejected { id, error } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(error))),
                }));
            }
            CommandOutcome::OwesSendSingle { id, send } => {
                let mut entropy_bytes = [0u8; SendSingleEntropy::LEN];
                fill_entropy(&mut entropy_bytes);
                let entropy = SendSingleEntropy::new(entropy_bytes);

                let mut buf = [0u8; BROADCAST_MTU];
                match self.write_commanded_send_single(id, &send, now, entropy, &mut buf) {
                    SendSingleWriteOutcome::Written(dispatch) => {
                        fan_self_originated(
                            interfaces,
                            Some(dispatch.fire_on),
                            &buf[..dispatch.wire_len],
                            sink,
                        );
                        if let Some(culled) = dispatch.culled {
                            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                                id: culled.command_id,
                                settlement: Settlement::SendSingle(Err(SendSingleFailure::Culled)),
                            }));
                        }
                    }
                    SendSingleWriteOutcome::Rejected { rejection, .. } => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendSingle(Err(
                                SendSingleFailure::WriteFailed(rejection.into()),
                            )),
                        }));
                    }
                    SendSingleWriteOutcome::Failed { failure } => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::SendSingle(Err(
                                SendSingleFailure::WriteFailed(WriteSendSingleError::Seal(failure)),
                            )),
                        }));
                    }
                }
                wake_schedule_changes.send_single_timeout =
                    self.send_single_receipts_timeout_wake();
            }
            CommandOutcome::SendSingleRejected { id, error } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendSingle(Err(SendSingleFailure::Rejected(error))),
                }));
            }
            CommandOutcome::OwesSendGroup { id, send } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);

                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_send_group(&send, &iv, &mut buf) {
                    Ok(wire_len) => {
                        fan_self_originated(interfaces, None, &buf[..wire_len], sink);
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
                            settlement: Settlement::RequestPath(Ok(PathFound { hops })),
                        }));
                    }
                    PathRequestWriteOutcome::Written { wire_len, culled } => {
                        fan_self_originated(interfaces, None, &buf[..wire_len], sink);
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
                match self.write_commanded_link_request(id, &establish, now, entropy, &mut buf) {
                    EstablishLinkWriteOutcome::Written(dispatch) => {
                        fan_self_originated(
                            interfaces,
                            Some(dispatch.fire_on),
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
            CommandOutcome::OwesSendLink { id, send } => {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_entropy(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                let settlement = match self.write_commanded_send_link(&send, &iv, &mut buf) {
                    Ok(dispatch) => {
                        fan_self_originated(
                            interfaces,
                            Some(dispatch.fire_on),
                            &buf[..dispatch.wire_len],
                            sink,
                        );
                        Settlement::SendLink(Ok(()))
                    }
                    Err(SendLinkWriteError::LinkVanished) => Settlement::SendLink(Err(
                        SendLinkFailure::Rejected(SendLinkError::NoSuchLink),
                    )),
                    Err(SendLinkWriteError::Frame(error)) => {
                        Settlement::SendLink(Err(SendLinkFailure::WriteFailed(error)))
                    }
                };
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement,
                }));
            }
            CommandOutcome::SendLinkRejected { id, error } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendLink(Err(SendLinkFailure::Rejected(error))),
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
                                Some(fire_on),
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
            CommandOutcome::CloseLinkRejected { id, error } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::CloseLink(Err(CloseLinkFailure::Rejected(error))),
                }));
            }
            CommandOutcome::EstablishLinkRejected { id, error } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(
                        error,
                    ))),
                }));
            }
        }
        wake_schedule_changes
    }
}

/// Fan one self-originated payload to its targets: every interface (`only` = `None`) or a
/// single named one, taking each that is live and may transmit. The same gate the legacy
/// runtime's `fan_to_handles` applied with `FanoutClass::SelfOriginated`; the bytes are
/// lent to each `Send` in turn, never copied into a staging buffer.
fn fan_self_originated(
    interfaces: &[InterfaceConfig],
    only: Option<InterfaceId>,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    for config in interfaces {
        let targeted = only.is_none_or(|id| config.id == id);
        if targeted && config.capabilities.allows_transmit() {
            sink(EngineReaction::Directive(Directive::Send {
                target: config.id,
                bytes,
            }));
        }
    }
}
