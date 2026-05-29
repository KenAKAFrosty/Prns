/// Lifecycle state of an interface, observable by the engine. Used
/// for routing decisions (the engine prefers `Connected` interfaces
/// over `Degraded` ones, won't route via `Failed` or `Disconnected`).
/// Used for diagnostics, and - critically - for hot-reload-without-restart
/// support: the host can move an interface through `Disconnected` and
/// remove it without taking down the engine, and add new interfaces
/// in `Initializing` at any time.
///
/// Compared to RNS's ad-hoc booleans
/// ([`online`, `detached`, `never_connected`, `reconnecting`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L96-L128))
/// this is an explicit enum with documented transitions: easier to
/// reason about, harder to land in an illegal combination.
///
/// # Documented transitions
///
/// - `Initializing → Connected` on successful transport setup.
/// - `Initializing → Failed` if setup fails terminally.
/// - `Connected ↔ Degraded` as health monitoring observes
///   retries / drops / latency.
/// - `Connected → Reconnecting` or `Degraded → Reconnecting` when the
///   connection drops.
/// - `Reconnecting → Connected` on successful recovery.
/// - `Reconnecting → Failed` after exhausting retry attempts.
/// - `Failed → Initializing` if the host triggers a fresh attempt
///   (e.g., a management-API retry).
/// - Any state → `Disconnected` when the host explicitly tears down
///   the interface. After `Disconnected` the interface is removed
///   from the engine entirely; the id may be reused for a freshly
///   constructed instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceState {
    /// Just constructed; transport setup in progress (TCP connecting,
    /// LoRa radio initializing, BLE bond establishing, etc.).
    Initializing,

    /// Up and exchanging packets normally.
    Connected,

    /// Connected but unhealthy (elevated retries, drops, latency).
    /// The engine may prefer alternative interfaces for new traffic
    /// but can still route via this one.
    Degraded,

    /// Lost connection; the host is actively trying to recover. The
    /// engine should treat this as temporarily unroutable.
    Reconnecting,

    /// Down. The engine should not route via. Recovery requires
    /// explicit host action (a management-API retry, a config reload,
    /// etc.).
    Failed,

    /// Host has explicitly torn down the interface; about to be
    /// removed from the engine entirely. Distinguished from `Failed`
    /// so the engine knows this is intentional (drop all state keyed
    /// on the id, don't await reconnection).
    Disconnected,
}
