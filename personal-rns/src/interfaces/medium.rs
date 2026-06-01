/// EXPERIMENTAL / ITERATIVE
///
/// Classification of the underlying physical or virtual medium an
/// interface carries traffic over. Complements [`Capabilities`](crate::interfaces::Capabilities) (what
/// the interface can do) and [`InterfaceMode`](crate::interfaces::InterfaceMode) (the engine's policy
/// for it) by describing the medium's nature.
///
/// This is **not** a direct port of an RNS concept: RNS distinguishes
/// media implicitly through interface subclasses (`RNodeInterface`
/// is LoRa, `AutoInterface` uses IP multicast, etc.). We surface it
/// explicitly so:
///
/// - Sims can faithfully model "a LoRa-like interface" by declaring
///   `SharedHalfDuplex` and exhibiting the matching behaviors, so the
///   engine's behavior under sim and real hardware exercises the same
///   surface.
/// - Diagnostics and host tooling get a typed handle on the medium.
/// - The engine MAY use this later for medium-aware decisions (e.g.,
///   not expecting packet ordering on a contended shared medium).
///
/// Like [`Capabilities`](crate::interfaces::Capabilities) and [`InterfaceMode`](crate::interfaces::InterfaceMode), this is the
/// declaration shape, i.e., what a host or config parser fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediumKind {
    /// One identified peer, reliable ordering, no fanout in the
    /// medium itself. TCP, BLE GATT, USB CDC, serial.
    DirectPeer,

    /// One logical peer, but the underlying fabric routes through
    /// unknown intermediaries (switches, ISPs, …) that may reorder,
    /// drop, or duplicate. Distinct from `DirectPeer` for diagnostics
    /// and sim modeling (e.g., simulating realistic packet loss).
    SwitchedNetwork,

    /// Shared broadcast medium where every neighbor hears every
    /// transmission, including the sender hearing its own echo back.
    /// LoRa, plain ESP-NOW broadcast, classic packet radio. Pairs
    /// naturally with [`Capabilities::repeats`](crate::interfaces::Capabilities::repeats) = true.
    SharedHalfDuplex,

    /// Group-addressed shared medium reaching a declared subset of
    /// neighbors. IP multicast, Bluetooth Mesh. Used by RNS's
    /// `AutoInterface` for peer discovery.
    Multicast,

    /// In-process / virtual medium. Sim and test loopback. No physical
    /// wire involved.
    Loopback,
}
