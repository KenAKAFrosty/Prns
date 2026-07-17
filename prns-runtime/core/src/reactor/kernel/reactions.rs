#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{Directive, EngineReaction, FanTarget, Journaled};
use crate::interfaces::{InterfaceId, InterfaceKind};

pub(crate) trait DirectiveEgress {
    fn send(&mut self, target: InterfaceId, bytes: &[u8]);

    fn send_announce(
        &mut self,
        target: InterfaceId,
        bytes: &[u8],
        hops: u8,
        #[cfg(feature = "runtime-metrics")] origin: AnnounceOrigin,
    );

    fn send_to_fleet(&mut self, supervisor: InterfaceKind, fan: FanTarget, bytes: &[u8]);

    fn send_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
        hops: u8,
        #[cfg(feature = "runtime-metrics")] origin: AnnounceOrigin,
    );

    fn emit_frame(
        &mut self,
        target: InterfaceId,
        size_hint: usize,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    );

    #[cfg(feature = "runtime-metrics")]
    fn send_measured_local_announce(&mut self, target: InterfaceId, bytes: &[u8]);

    #[cfg(feature = "runtime-metrics")]
    fn send_measured_local_announce_to_fleet(
        &mut self,
        supervisor: InterfaceKind,
        fan: FanTarget,
        bytes: &[u8],
    );
}

pub(crate) fn route_reaction(
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
                bytes,
                hops,
                #[cfg(feature = "runtime-metrics")]
                origin,
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
                bytes,
                hops,
                #[cfg(feature = "runtime-metrics")]
                origin,
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
