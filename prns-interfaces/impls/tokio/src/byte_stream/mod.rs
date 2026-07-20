#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "rnode",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone",
    feature = "i2p"
))]
pub(crate) mod framing;

#[cfg(any(feature = "kiss", feature = "ax25", feature = "rnode"))]
pub(crate) mod deadline;
