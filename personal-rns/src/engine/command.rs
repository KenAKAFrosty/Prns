use crate::engine::{
    AnnounceNowFailure, AnnounceTarget, CommandOutcome, CommandedAnnounceWriteOutcome, Directive,
    EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, PathFound,
    PathRequestWriteOutcome, RatchetEntropy, RequestPathFailure, SendSingleEntropy,
    SendSingleFailure, SendSingleWriteOutcome, Settlement, WakeSchedules, WriteSendSingleError,
};
use crate::interfaces::{InterfaceConfig, InterfaceId};
use crate::routing::announce::AnnounceEntropy;
use crate::routing::storage::EngineStorage;
use crate::wire::MTU;

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
        view: &[InterfaceConfig],
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut delta = WakeSchedules::UNCHANGED;
        match self.ingest_command(issued, view) {
            CommandOutcome::OwesAnnounce { id, announce } => {
                let mut announce_entropy_bytes = [0u8; AnnounceEntropy::LEN];
                fill_entropy(&mut announce_entropy_bytes);
                let announce_entropy = AnnounceEntropy::new(announce_entropy_bytes);
                let mut ratchet_bytes = [0u8; RatchetEntropy::LEN];
                fill_entropy(&mut ratchet_bytes);
                let ratchet = RatchetEntropy::new(ratchet_bytes);

                let mut buf = [0u8; MTU];
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
                        fan_self_originated(view, only, &buf[..len], sink);
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

                let mut buf = [0u8; MTU];
                match self.write_commanded_send_single(id, &send, now, entropy, &mut buf) {
                    SendSingleWriteOutcome::Written(dispatch) => {
                        fan_self_originated(
                            view,
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
                delta.send_single_timeout = self.send_timeout_lane();
            }
            CommandOutcome::SendSingleRejected { id, error } => {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendSingle(Err(SendSingleFailure::Rejected(error))),
                }));
            }
            CommandOutcome::OwesPathRequest { id, request } => {
                let mut buf = [0u8; MTU];
                match self.write_commanded_path_request(id, &request, now, &mut buf) {
                    PathRequestWriteOutcome::AlreadyReachable { hops } => {
                        sink(EngineReaction::Journaled(Journaled::CommandSettled {
                            id,
                            settlement: Settlement::RequestPath(Ok(PathFound { hops })),
                        }));
                    }
                    PathRequestWriteOutcome::Written { wire_len, culled } => {
                        fan_self_originated(view, None, &buf[..wire_len], sink);
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
                delta.path_request_timeout = self.path_timeout_lane();
            }
        }
        delta
    }
}

/// Fan one self-originated payload to its targets: every interface (`only` = `None`) or a
/// single named one, taking each that is live and may transmit. The same gate the legacy
/// runtime's `fan_to_handles` applied with `FanoutClass::SelfOriginated`; the bytes are
/// lent to each `Send` in turn, never copied into a staging buffer.
fn fan_self_originated(
    view: &[InterfaceConfig],
    only: Option<InterfaceId>,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    for config in view {
        let targeted = only.is_none_or(|id| config.id == id);
        if targeted && config.capabilities.allows_transmit() {
            sink(EngineReaction::Directive(Directive::Send {
                target: config.id,
                bytes,
            }));
        }
    }
}
