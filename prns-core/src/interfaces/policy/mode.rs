prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[repr(u8)]
    pub enum InterfaceMode {
        Full = 0,
        PointToPoint = 1,
        AccessPoint = 2,
        Roaming = 3,
        Boundary = 4,
        Gateway = 5,
        Internal = 6,
    }
}

impl InterfaceMode {
    /// RNS 1.4.2 `Interface.DISCOVER_PATHS_FOR = [ACCESS_POINT, GATEWAY, ROAMING, INTERNAL]`; other modes answer only from paths they already hold.
    pub fn recursively_forwards_unknown_paths(self) -> bool {
        matches!(
            self,
            InterfaceMode::AccessPoint
                | InterfaceMode::Gateway
                | InterfaceMode::Roaming
                | InterfaceMode::Internal
        )
    }

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Full),
            1 => Some(Self::PointToPoint),
            2 => Some(Self::AccessPoint),
            3 => Some(Self::Roaming),
            4 => Some(Self::Boundary),
            5 => Some(Self::Gateway),
            6 => Some(Self::Internal),
            _ => None,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::PointToPoint => "Point-to-point",
            Self::AccessPoint => "Access point",
            Self::Roaming => "Roaming",
            Self::Boundary => "Boundary",
            Self::Gateway => "Gateway",
            Self::Internal => "Internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_unknown_path_discovery_matches_rns_1_4_2() {
        for mode in [
            InterfaceMode::AccessPoint,
            InterfaceMode::Gateway,
            InterfaceMode::Roaming,
            InterfaceMode::Internal,
        ] {
            assert!(mode.recursively_forwards_unknown_paths(), "{mode:?}");
        }
        for mode in [
            InterfaceMode::Full,
            InterfaceMode::PointToPoint,
            InterfaceMode::Boundary,
        ] {
            assert!(!mode.recursively_forwards_unknown_paths(), "{mode:?}");
        }
    }

    #[test]
    fn every_mode_owns_a_stable_wire_byte_and_label() {
        assert_eq!(
            InterfaceMode::ALL,
            [
                InterfaceMode::Full,
                InterfaceMode::PointToPoint,
                InterfaceMode::AccessPoint,
                InterfaceMode::Roaming,
                InterfaceMode::Boundary,
                InterfaceMode::Gateway,
                InterfaceMode::Internal,
            ]
        );
        assert_eq!(
            InterfaceMode::ALL.map(InterfaceMode::wire_value),
            [0, 1, 2, 3, 4, 5, 6]
        );
        assert_eq!(
            InterfaceMode::ALL.map(InterfaceMode::label),
            [
                "Full",
                "Point-to-point",
                "Access point",
                "Roaming",
                "Boundary",
                "Gateway",
                "Internal",
            ]
        );
        for mode in InterfaceMode::ALL {
            assert_eq!(InterfaceMode::from_wire(mode.wire_value()), Some(mode));
        }
        assert_eq!(InterfaceMode::from_wire(7), None);
    }
}
