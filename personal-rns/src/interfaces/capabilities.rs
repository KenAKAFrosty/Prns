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
    Enabled(TransitCapability),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitCapability {
    NoTransit,
    CrossInterfaceOnly,
    SameInterfaceRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceCapabilitiesError {
    TransitRequiresTransmit,
    SameInterfaceRepeatRequiresTransit,
}

impl InterfaceCapabilities {
    pub const fn allows_transmit(self) -> bool {
        !matches!(self.egress, EgressCapability::Disabled)
    }

    pub const fn allows_local_egress(self) -> bool {
        self.allows_transmit()
    }

    pub const fn allows_transit(self) -> bool {
        matches!(
            self.egress,
            EgressCapability::Enabled(
                TransitCapability::CrossInterfaceOnly | TransitCapability::SameInterfaceRepeat
            )
        )
    }

    pub const fn allows_same_interface_repeat(self) -> bool {
        matches!(
            self.egress,
            EgressCapability::Enabled(TransitCapability::SameInterfaceRepeat)
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
            (true, false, false) => EgressCapability::Enabled(TransitCapability::NoTransit),
            (true, true, false) => EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly),
            (true, true, true) => EgressCapability::Enabled(TransitCapability::SameInterfaceRepeat),
            (false, true, _) => return Err(InterfaceCapabilitiesError::TransitRequiresTransmit),
            (false, false, true) => {
                return Err(InterfaceCapabilitiesError::SameInterfaceRepeatRequiresTransit);
            }
            (true, false, true) => {
                return Err(InterfaceCapabilitiesError::SameInterfaceRepeatRequiresTransit);
            }
        };

        Ok(Self { ingress, egress })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_non_transit_transmit_shape() {
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
            EgressCapability::Enabled(TransitCapability::NoTransit)
        );
    }

    #[test]
    fn normalizes_cross_interface_transit_shape() {
        let normalized = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: false,
        })
        .unwrap();

        assert_eq!(
            normalized.egress,
            EgressCapability::Enabled(TransitCapability::CrossInterfaceOnly)
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
            EgressCapability::Enabled(TransitCapability::SameInterfaceRepeat)
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
        assert!(!disabled.allows_local_egress());
        assert!(!disabled.allows_transit());
        assert!(!disabled.allows_same_interface_repeat());

        let local_only = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: false,
            repeats: false,
        })
        .unwrap();
        assert!(local_only.allows_transmit());
        assert!(local_only.allows_local_egress());
        assert!(!local_only.allows_transit());
        assert!(!local_only.allows_same_interface_repeat());

        let cross_interface = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: false,
        })
        .unwrap();
        assert!(cross_interface.allows_transit());
        assert!(!cross_interface.allows_same_interface_repeat());

        let same_interface = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: true,
        })
        .unwrap();
        assert!(same_interface.allows_transit());
        assert!(same_interface.allows_same_interface_repeat());
    }

    #[test]
    fn rejects_transit_without_transmit() {
        assert_eq!(
            InterfaceCapabilities::try_from(Capabilities {
                receives: true,
                transmits: false,
                forwards: true,
                repeats: false,
            }),
            Err(InterfaceCapabilitiesError::TransitRequiresTransmit)
        );
    }

    #[test]
    fn rejects_same_interface_repeat_without_transit() {
        assert_eq!(
            InterfaceCapabilities::try_from(Capabilities {
                receives: true,
                transmits: true,
                forwards: false,
                repeats: true,
            }),
            Err(InterfaceCapabilitiesError::SameInterfaceRepeatRequiresTransit)
        );
    }
}
