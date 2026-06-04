#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediumKind {
    DirectPeer,

    SwitchedNetwork,

    SharedHalfDuplex,

    Multicast,

    Loopback,
}
