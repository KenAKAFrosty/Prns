#![cfg_attr(not(feature = "std"), no_std)]
#![doc = "LXMF application-layer scaffold above the Personal Reticulum engine."]

/// Marker for the LXMF layer while daemon-required Reticulum behavior is rebuilt.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LxmfLayer;
