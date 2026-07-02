#![forbid(unsafe_code)]

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "pipe",
    feature = "local",
    feature = "backbone"
))]
mod framed_stream;

#[cfg(feature = "tcp")]
pub mod tcp;

#[cfg(feature = "udp")]
pub mod udp;

#[cfg(feature = "serial")]
pub mod serial;

#[cfg(feature = "kiss")]
pub mod kiss;

#[cfg(feature = "rnode")]
pub mod rnode;

#[cfg(feature = "pipe")]
pub mod pipe;
