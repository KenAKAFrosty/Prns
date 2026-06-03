//! Per-substrate USB-auto workers. The std discoverer owns a host's whole USB
//! bus — many links behind one seam; the embassy device responder (later) answers
//! a single host on its one link.

#[cfg(feature = "std-host")]
mod std;
