//! Optional, platform-neutral observations that can inform PRNS behavior.
//!
//! Hosts own acquisition: an embedded board may read a UART or ADC while a phone uses operating
//! system services. This module owns the portable values and deterministic interpretation shared
//! by those hosts. Merely supplying an observation never publishes it or changes network policy.

pub mod positioning;
pub mod power;
