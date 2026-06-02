/// Policy role for an interface in routing and rebroadcast decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceMode {
    /// Unrestricted participant.
    Full,

    /// Endpoint-oriented link that is not expected to carry transit traffic.
    PointToPoint,

    /// Accepts clients, suppresses outbound announce broadcast, and discovers paths.
    AccessPoint,

    /// Mobile interface with restricted announce fanout and active path discovery.
    Roaming,

    /// Boundary between routing or trust domains, with restricted fanout.
    Boundary,

    /// General gateway that actively discovers paths.
    Gateway,
}
