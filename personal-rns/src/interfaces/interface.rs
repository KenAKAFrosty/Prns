use crate::interfaces::{Capabilities, InterfaceId, InterfaceMode, InterfaceState, MediumKind};

/// Base contract every transport interface honors. Exposes the
/// declared shape (id, capabilities, mode, medium) and the observable
/// state (lifecycle); per-medium-kind read/write methods (and any
/// other capability-specific verbs) live on sub-traits, so the
/// compiler enforces "this method only exists where it makes sense."
///
/// Hosts implement this on concrete interface types (TCP, LoRa, BLE,
/// loopback, sim, …); the engine consumes the trait via dyn dispatch
/// to make per-interface routing and fanout decisions. Sims implement
/// the same trait and matching sub-traits, so the engine's behavior
/// under sim and under real hardware exercises one surface.
pub trait Interface {
    /// Stable identity for this interface.
    fn id(&self) -> InterfaceId;

    /// Declared capability set: what the host says this interface
    /// can and will do.
    fn capabilities(&self) -> Capabilities;

    /// Operational role the engine treats this interface in.
    fn mode(&self) -> InterfaceMode;

    /// Classification of the underlying physical or virtual medium.
    fn medium_kind(&self) -> MediumKind;

    /// Current lifecycle state.
    fn state(&self) -> InterfaceState;

    /// Identity of the parent interface, if this one was spawned by a
    /// server-style interface (e.g., a TCP-client interface spawned by
    /// a TCP-server on each accepted connection, or an auto-interface
    /// peer spawned by an auto-interface on discovery). `None` for
    /// top-level interfaces.
    ///
    /// Mirrors RNS's `parent_interface` / `spawned_interfaces`
    /// relationship: see
    /// [`Interface.received_announce`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L259-L267)
    /// for how RNS propagates per-interface stats up the chain.
    fn parent_interface(&self) -> Option<InterfaceId> {
        None
    }
}
