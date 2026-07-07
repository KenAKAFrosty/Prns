#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceMode {
    Full,
    PointToPoint,
    AccessPoint,
    Roaming,
    Boundary,
    Gateway,
}

impl InterfaceMode {
    /// RNS 1.3.5 `Interface.DISCOVER_PATHS_FOR = [ACCESS_POINT, GATEWAY, ROAMING]`:
    /// the modes on which a transport node will recursively forward a path request
    /// for an unknown destination on the requester's behalf. Other modes answer
    /// only from what they already hold.
    pub fn recursively_forwards_unknown_paths(self) -> bool {
        matches!(
            self,
            InterfaceMode::AccessPoint | InterfaceMode::Gateway | InterfaceMode::Roaming
        )
    }
}
