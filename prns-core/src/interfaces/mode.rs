#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceMode {
    Full,
    PointToPoint,
    AccessPoint,
    Roaming,
    Boundary,
    Gateway,
    Internal,
}

impl InterfaceMode {
    /// RNS 1.3.9 `Interface.DISCOVER_PATHS_FOR = [ACCESS_POINT, GATEWAY, ROAMING, INTERNAL]`:
    /// the modes on which a transport node will recursively forward a path request
    /// for an unknown destination on the requester's behalf. Other modes answer
    /// only from what they already hold.
    pub fn recursively_forwards_unknown_paths(self) -> bool {
        matches!(
            self,
            InterfaceMode::AccessPoint
                | InterfaceMode::Gateway
                | InterfaceMode::Roaming
                | InterfaceMode::Internal
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_unknown_path_discovery_matches_rns_1_3_9() {
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
}
