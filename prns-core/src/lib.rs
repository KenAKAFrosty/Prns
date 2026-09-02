#![cfg_attr(not(any(feature = "std", test)), no_std)]
// Legacy helpers remain callable only to keep older protocol fixtures readable while they migrate.
// Their declarations stay deprecated and production consumers still receive the warning.
#![cfg_attr(test, allow(deprecated))]
#![deny(unsafe_code)]
#![doc = "Deterministic Reticulum engine & wire contract used by Prns"]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod capabilities;
pub mod crypto;
pub mod engine;
pub mod entropy;
pub mod identity;
pub mod interfaces;
pub mod lemire_index;
#[cfg(any(
    feature = "rnx",
    feature = "shared-instance-rpc",
    feature = "signed-artifact"
))]
pub mod message_pack;
pub mod persistence;
pub mod remote_control;
pub mod rncp;
#[cfg(feature = "rnx")]
pub mod rnx;
pub mod routing;
pub mod storage;
pub mod units;
pub mod wire;

#[cfg(feature = "interface-discovery")]
pub mod interface_discovery;
