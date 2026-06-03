#![cfg_attr(not(feature = "std"), no_std)]
// 100% safe Rust, compiler-enforced (rationale in personal-rns/src/lib.rs). LXMF is
// pure logic layered over the engine; it never needs `unsafe`.
#![forbid(unsafe_code)]
#![doc = "LXMF application-layer scaffold above the Personal Reticulum engine."]

/// Marker for the LXMF layer while daemon-required Reticulum behavior is rebuilt.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LxmfLayer;
