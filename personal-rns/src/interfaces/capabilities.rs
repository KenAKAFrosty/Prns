/// What an interface is willing to do for the engine, as declared by
/// the host (typically by parsing an `rnsd`-compatible config file).
/// Mirrors RNS's `IN / OUT / FWD / RPT` class-level flags on
/// [`RNS.Interfaces.Interface.Interface`](https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Interfaces/Interface.py#L38-L41)
/// in renamed, predicate-style form.
///
/// This is the **declaration shape**: a typed mirror of what a config
/// file said. The fields are nominally independent at this layer, but
/// some combinations are operationally incoherent — `forwards = true`
/// with `transmits = false` can't actually forward anything. A
/// normalized engine-side type that makes illegal combinations
/// unrepresentable can land once the engine consumes the full shape.
/// Today the engine only relies on `transmits` when registering a
/// concrete interface for fanout; the remaining fields are declaration
/// data until their owning routing decisions land.
///
/// The host states facts about its concrete interface; the engine
/// (after normalization) makes the routing/fanout calls. Operational
/// `mode` like `Full`, `AccessPoint`, etc. lands separately and
/// composes with these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub receives: bool,
    pub transmits: bool,
    pub forwards: bool,
    pub repeats: bool,
}
