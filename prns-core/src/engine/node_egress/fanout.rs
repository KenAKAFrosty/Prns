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
