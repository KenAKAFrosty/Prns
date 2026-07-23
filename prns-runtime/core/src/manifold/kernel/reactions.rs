#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{Directive, EngineReaction, FanTarget, Journaled};
use crate::interfaces::{InterfaceId, InterfaceKind};

pub struct AnnounceDirective<'a> {
    bytes: &'a [u8],
    hops: u8,
    #[cfg(feature = "runtime-metrics")]
    origin: AnnounceOrigin,
}

impl<'a> AnnounceDirective<'a> {
    #[must_use]
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    #[must_use]
    pub fn hops(&self) -> u8 {
        self.hops
    }

    #[cfg(feature = "runtime-metrics")]
    #[must_use]
    pub fn origin(&self) -> AnnounceOrigin {
        self.origin
    }
}

pub trait DirectiveEgress {
    fn send(&mut self, target: InterfaceId, bytes: &[u8]);

    fn send_announce(&mut self, target: InterfaceId, announce: AnnounceDirective<'_>);

    fn send_to_fleet(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]);

    fn send_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        announce: AnnounceDirective<'_>,
    );

    fn emit_frame(
        &mut self,
        target: InterfaceId,
        size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    );

    fn send_measured_local_announce(&mut self, target: InterfaceId, bytes: &[u8]) {
        self.send(target, bytes);
    }

    fn send_measured_local_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
    ) {
        self.send_to_fleet(supervisor, fan, bytes);
    }
}

pub fn route_reaction(
    reaction: EngineReaction<'_>,
    egress: &mut impl DirectiveEgress,
    app: &mut impl FnMut(Journaled<'_>),
) {
    match reaction {
        EngineReaction::Directive(Directive::Send { target, bytes }) => {
            egress.send(target, bytes);
        }
        EngineReaction::Directive(Directive::SendAnnounce {
            target,
            bytes,
            hops,
            #[cfg(feature = "runtime-metrics")]
            origin,
        }) => {
            egress.send_announce(
                target,
                AnnounceDirective {
                    bytes,
                    hops,
                    #[cfg(feature = "runtime-metrics")]
                    origin,
                },
            );
        }
        EngineReaction::Directive(Directive::SendToFleet {
            supervisor,
            fan,
            bytes,
        }) => {
            egress.send_to_fleet(supervisor, fan, bytes);
        }
        EngineReaction::Directive(Directive::SendAnnounceToFleet {
            supervisor,
            fan,
            bytes,
            hops,
            #[cfg(feature = "runtime-metrics")]
            origin,
        }) => {
            egress.send_announce_to_fleet(
                supervisor,
                fan,
                AnnounceDirective {
                    bytes,
                    hops,
                    #[cfg(feature = "runtime-metrics")]
                    origin,
                },
            );
        }
        EngineReaction::Directive(Directive::EmitFrame {
            target,
            size_hint,
            fill,
        }) => {
            egress.emit_frame(target, size_hint, fill);
        }
        #[cfg(feature = "runtime-metrics")]
        EngineReaction::Directive(Directive::SendMeasuredLocalAnnounce { target, bytes }) => {
            egress.send_measured_local_announce(target, bytes);
        }
        #[cfg(feature = "runtime-metrics")]
        EngineReaction::Directive(Directive::SendMeasuredLocalAnnounceToFleet {
            supervisor,
            fan,
            bytes,
        }) => {
            egress.send_measured_local_announce_to_fleet(supervisor, fan, bytes);
        }
        EngineReaction::Journaled(journaled) => app(journaled),
    }
}
