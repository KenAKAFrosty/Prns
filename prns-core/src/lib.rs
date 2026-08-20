#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![deny(unsafe_code)]
#![doc = "Deterministic Reticulum engine & wire contract used by Prns"]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod capabilities;
pub mod crypto;
pub mod engine;
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
pub mod rncp;
#[cfg(feature = "rnx")]
pub mod rnx;
pub mod routing;
pub mod storage;
pub mod units;
pub mod wire;

#[cfg(feature = "interface-discovery")]
pub mod interface_discovery;
