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

    fn send_if_online(&mut self, target: InterfaceId, bytes: &[u8], on_send: &mut dyn FnMut()) {
        on_send();
        self.send(target, bytes);
    }

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

pub fn route_reaction<Work>(
    reaction: EngineReaction<'_, Work>,
    egress: &mut impl DirectiveEgress,
    app: &mut impl FnMut(Journaled<'_>),
    fulfill: &mut impl FnMut(Work),
) {
    match reaction {
        EngineReaction::Directive(Directive::Fulfill(work)) => fulfill(work),
        EngineReaction::Directive(Directive::Send { target, bytes }) => {
            egress.send(target, bytes);
        }
        EngineReaction::Directive(Directive::SendIfOnline {
            target,
            bytes,
            on_send,
        }) => {
            egress.send_if_online(target, bytes, on_send);
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

#[cfg(test)]
mod tests {
    use super::*;

    struct CountingEgress {
        sends: usize,
    }

    impl DirectiveEgress for CountingEgress {
        fn send(&mut self, _target: InterfaceId, _bytes: &[u8]) {
            self.sends += 1;
        }

        fn send_announce(&mut self, _target: InterfaceId, _announce: AnnounceDirective<'_>) {}

        fn send_to_fleet(&mut self, _supervisor: InterfaceKind, _fan: FanTarget, _bytes: &[u8]) {}

        fn send_announce_to_fleet(
            &mut self,
            _supervisor: InterfaceKind,
            _fan: FanTarget,
            _announce: AnnounceDirective<'_>,
        ) {
        }

        fn emit_frame(
            &mut self,
            _target: InterfaceId,
            _size_hint: usize,
            _fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
        ) {
        }
    }

    #[test]
    fn an_online_only_send_records_after_egress_accepts_it() {
        let target = InterfaceId::new([0x6c; 8]);
        let mut records = 0;
        let mut on_send = || records += 1;
        let mut egress = CountingEgress { sends: 0 };

        route_reaction::<crate::engine::NoOwedWork>(
            EngineReaction::Directive(Directive::SendIfOnline {
                target,
                bytes: b"path request",
                on_send: &mut on_send,
            }),
            &mut egress,
            &mut |_| {},
            &mut |_| {},
        );

        assert_eq!((records, egress.sends), (1, 1));
    }
}
