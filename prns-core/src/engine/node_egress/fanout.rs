use crate::engine::{Directive, EngineReaction, FanTarget};
use crate::interfaces::{AttachedInterfaces, InterfaceKind, InterfaceMode};

pub(in crate::engine) fn fan_frame(
    interfaces: AttachedInterfaces<'_>,
    fanout: FanTarget,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    fan(interfaces, fanout, bytes, EmissionKind::Frame, sink);
}

pub(in crate::engine) fn fan_announce(
    interfaces: AttachedInterfaces<'_>,
    fanout: FanTarget,
    bytes: &[u8],
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    fan(interfaces, fanout, bytes, EmissionKind::Announce, sink);
}

#[derive(Clone, Copy)]
enum EmissionKind {
    Frame,
    Announce,
}

fn fan(
    interfaces: AttachedInterfaces<'_>,
    fanout: FanTarget,
    bytes: &[u8],
    emission: EmissionKind,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
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
                        EmissionKind::Frame => {
                            sink(EngineReaction::Directive(Directive::SendToFleet {
                                supervisor,
                                fan: fanout,
                                bytes,
                            }));
                        }
                        EmissionKind::Announce => {
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
                if matches!(emission, EmissionKind::Announce)
                    && descriptor.mode == InterfaceMode::AccessPoint
                {
                    continue;
                }
                match emission {
                    EmissionKind::Frame => sink(EngineReaction::Directive(Directive::Send {
                        target: descriptor.id,
                        bytes,
                    })),
                    EmissionKind::Announce => {
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
    use crate::interfaces::{InterfaceDescriptor, InterfaceId};

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn local_announces_use_every_mode_except_access_point() {
        let interfaces = [
            InterfaceDescriptor {
                mode: InterfaceMode::Full,
                ..routable_descriptor(iface(0xE1))
            },
            InterfaceDescriptor {
                mode: InterfaceMode::PointToPoint,
                ..routable_descriptor(iface(0xE2))
            },
            InterfaceDescriptor {
                mode: InterfaceMode::AccessPoint,
                ..routable_descriptor(iface(0xE3))
            },
            InterfaceDescriptor {
                mode: InterfaceMode::Roaming,
                ..routable_descriptor(iface(0xE4))
            },
            InterfaceDescriptor {
                mode: InterfaceMode::Boundary,
                ..routable_descriptor(iface(0xE5))
            },
            InterfaceDescriptor {
                mode: InterfaceMode::Gateway,
                ..routable_descriptor(iface(0xE6))
            },
            InterfaceDescriptor {
                mode: InterfaceMode::Internal,
                ..routable_descriptor(iface(0xE7))
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
            std::vec![
                iface(0xE1),
                iface(0xE2),
                iface(0xE4),
                iface(0xE5),
                iface(0xE6),
                iface(0xE7),
            ],
        );
    }
}
