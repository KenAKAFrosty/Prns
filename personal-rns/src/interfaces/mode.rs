#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceMode {
    Full,

    PointToPoint,

    AccessPoint,

    Roaming,

    Boundary,

    Gateway,
}
