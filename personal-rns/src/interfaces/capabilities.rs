use core::convert::TryFrom;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub receives: bool,
    pub transmits: bool,
    pub forwards: bool,
    pub repeats: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceCapabilities {
    pub ingress: IngressCapability,
    pub egress: EgressCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressCapability {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressCapability {
    Disabled,
    Enabled(TransportCapability),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCapability {
    NoTransport,
    CrossInterfaceOnly,
    SameInterfaceRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceCapabilitiesError {
    TransportRequiresTransmit,
    SameInterfaceRepeatRequiresTransport,
}

impl InterfaceCapabilities {
    pub const fn allows_transmit(self) -> bool {
        !matches!(self.egress, EgressCapability::Disabled)
    }

    pub const fn allows_transport(self) -> bool {
        matches!(
            self.egress,
            EgressCapability::Enabled(
                TransportCapability::CrossInterfaceOnly | TransportCapability::SameInterfaceRepeat
            )
        )
    }

    pub const fn allows_same_interface_repeat(self) -> bool {
        matches!(
            self.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        )
    }
}

impl TryFrom<Capabilities> for InterfaceCapabilities {
    type Error = InterfaceCapabilitiesError;

    fn try_from(capabilities: Capabilities) -> Result<Self, Self::Error> {
        let ingress = if capabilities.receives {
            IngressCapability::Enabled
        } else {
            IngressCapability::Disabled
        };

        let egress = match (
            capabilities.transmits,
            capabilities.forwards,
            capabilities.repeats,
        ) {
            (false, false, false) => EgressCapability::Disabled,
            (true, false, false) => EgressCapability::Enabled(TransportCapability::NoTransport),
            (true, true, false) => {
                EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly)
            }
            (true, true, true) => {
                EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
            }
            (false, true, _) => return Err(InterfaceCapabilitiesError::TransportRequiresTransmit),
            (false, false, true) => {
                return Err(InterfaceCapabilitiesError::SameInterfaceRepeatRequiresTransport);
            }
            (true, false, true) => {
                return Err(InterfaceCapabilitiesError::SameInterfaceRepeatRequiresTransport);
            }
        };

        Ok(Self { ingress, egress })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_non_transport_transmit_shape() {
        let normalized = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: false,
            repeats: false,
        })
        .unwrap();

        assert_eq!(normalized.ingress, IngressCapability::Enabled);
        assert_eq!(
            normalized.egress,
            EgressCapability::Enabled(TransportCapability::NoTransport)
        );
    }

    #[test]
    fn normalizes_cross_interface_transport_shape() {
        let normalized = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: false,
        })
        .unwrap();

        assert_eq!(
            normalized.egress,
            EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly)
        );
    }

    #[test]
    fn normalizes_same_interface_repeat_shape() {
        let normalized = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: true,
        })
        .unwrap();

        assert_eq!(
            normalized.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        );
    }

    #[test]
    fn predicates_reflect_the_normalized_egress_shape() {
        let disabled = InterfaceCapabilities::try_from(Capabilities {
            receives: false,
            transmits: false,
            forwards: false,
            repeats: false,
        })
        .unwrap();
        assert!(!disabled.allows_transmit());
        assert!(!disabled.allows_transport());
        assert!(!disabled.allows_same_interface_repeat());

        let transmit_only = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: false,
            repeats: false,
        })
        .unwrap();
        assert!(transmit_only.allows_transmit());
        assert!(!transmit_only.allows_transport());
        assert!(!transmit_only.allows_same_interface_repeat());

        let cross_interface = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: false,
        })
        .unwrap();
        assert!(cross_interface.allows_transport());
        assert!(!cross_interface.allows_same_interface_repeat());

        let same_interface = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: true,
        })
        .unwrap();
        assert!(same_interface.allows_transport());
        assert!(same_interface.allows_same_interface_repeat());
    }

    #[test]
    fn rejects_transport_without_transmit() {
        assert_eq!(
            InterfaceCapabilities::try_from(Capabilities {
                receives: true,
                transmits: false,
                forwards: true,
                repeats: false,
            }),
            Err(InterfaceCapabilitiesError::TransportRequiresTransmit)
        );
    }

    #[test]
    fn rejects_same_interface_repeat_without_transport() {
        assert_eq!(
            InterfaceCapabilities::try_from(Capabilities {
                receives: true,
                transmits: true,
                forwards: false,
                repeats: true,
            }),
            Err(InterfaceCapabilitiesError::SameInterfaceRepeatRequiresTransport)
        );
    }
}
