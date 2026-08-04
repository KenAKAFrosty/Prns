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
}
