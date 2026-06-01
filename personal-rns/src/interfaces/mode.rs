/// The operational role an interface plays in the engine's routing
/// model. Mirrors RNS's `MODE_*` constants on
/// [`RNS.Interfaces.Interface.Interface`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L44-L50)
/// in Rust-flavored form.
///
/// Mode is policy: it tells the engine WHAT KINDS OF DECISIONS to
/// make about announce fanout, transit forwarding, and path
/// discovery on this interface. It composes with [`Capabilities`](crate::interfaces::Capabilities):
/// capabilities say what the interface can technically do (receive
/// bytes, transmit bytes, …); mode says what the engine should do
/// with that capability.
///
/// Like [`Capabilities`](crate::interfaces::Capabilities), this is the declaration shape, i.e., what a
/// parsed `rnsd`-compatible config would surface. A normalized
/// engine-side type may replace it later as we learn what the engine
/// actually consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceMode {
    /// Unrestricted participant. Announces broadcast freely, transit
    /// traffic forwards, paths are learned and advertised. The most
    /// common mode; the default for `TCPClient`, `UDP`, `Auto`, etc.
    Full,

    /// Pure endpoint terminating at a specific peer. The engine does
    /// not expect to use it for transit traffic. Mostly informational
    /// in RNS — semantics largely overlap with `Full` in the current
    /// [`Transport.outbound`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1090)
    /// code.
    PointToPoint,

    /// Server-side interface that accepts clients but does **not**
    /// broadcast announces outbound on it
    /// ([Transport.py:1193](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1193)).
    /// The engine actively discovers paths for destinations reached
    /// via this mode
    /// ([`DISCOVER_PATHS_FOR`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L52-L54)).
    /// Typical for an `rnsd` hosting client connections.
    AccessPoint,

    /// Mobile / roaming interface. The engine restricts which
    /// announces broadcast outbound on it: only locally-originated
    /// announces, or announces whose next-hop interface is neither
    /// roaming nor boundary
    /// ([Transport.py:1197–1220](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1197-L1220)).
    /// Also triggers active path discovery. Used for nodes that may
    /// move between networks.
    Roaming,

    /// Gateway interface bridging trust or routing domains. Similar
    /// restricted-fanout rules to `Roaming`
    /// ([Transport.py:1222](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1222)),
    /// plus extra link-routing exceptions
    /// ([Transport.py:737, 750](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L737-L750)).
    /// Used to controllably hand traffic between distinct mesh
    /// segments.
    Boundary,

    /// General gateway. Triggers active path discovery like
    /// `AccessPoint` / `Roaming`; otherwise behaves similarly to
    /// `Full` in the current `Transport.outbound` code.
    Gateway,
}
