/// Classification of the medium an interface runs over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediumKind {
    /// One identified peer with no medium-level fanout.
    DirectPeer,

    /// One logical peer reached through an intervening network.
    SwitchedNetwork,

    /// Shared broadcast medium where all neighbors hear each transmission.
    SharedHalfDuplex,

    /// Group-addressed shared medium reaching a declared subset of neighbors.
    Multicast,

    /// In-process or virtual medium.
    Loopback,
}
