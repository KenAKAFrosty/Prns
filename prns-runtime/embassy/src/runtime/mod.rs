mod embassy_bind;
mod embassy_interface_store;

pub use prns_runtime::runtime::*;

pub use embassy_bind::Fleet as EmbassyFleet;
pub use embassy_bind::{
    CompletionPool, EmbassyPrnsHandle, Fleet, MemberWire, Prns, ReactorPlumbing,
};
pub use embassy_interface_store::EmbassyInterfaceStore;
pub(crate) use embassy_interface_store::{InterfaceInspectionStore, NoInterfaceInspectionStore};
