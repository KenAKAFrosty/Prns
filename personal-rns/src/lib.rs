#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![forbid(unsafe_code)]
#![doc = "Reticulum"]
#![deny(rustdoc::broken_intra_doc_links)]

pub use prns_runtime::{
    crypto, engine, identity, interfaces, persistence, routes, routing, storage, units, wire,
};

#[cfg(feature = "interface-discovery")]
pub use prns_runtime::interface_discovery;
#[cfg(feature = "std")]
pub use prns_runtime::node_introspection;

pub mod reactor {
    pub use prns_runtime::reactor::{
        airtime, announce_pacer, decline_all, duty_gate, grant, interface_seam, kernel, throughput,
        timers, AppDeciders, Host,
    };

    cfg_if::cfg_if! {
        if #[cfg(any(feature = "tokio-host", feature = "embassy-host"))] {
            pub mod impls {
                #[cfg(feature = "tokio-host")]
                pub use prns_runtime_tokio::reactor::impls::{compression, tokio_reactor};
                #[cfg(feature = "embassy-host")]
                pub use prns_runtime_embassy::reactor::impls::embassy_reactor;
            }
        }
    }

    #[cfg(feature = "embassy-host")]
    pub use prns_runtime_embassy::reactor::timebase;
}

pub mod runtime {
    cfg_if::cfg_if! {
        if #[cfg(feature = "tokio-host")] {
            pub use prns_runtime_tokio::runtime::*;
        } else if #[cfg(feature = "embassy-host")] {
            pub use prns_runtime_embassy::runtime::*;
        } else {
            pub use prns_runtime::runtime::*;
        }
    }

    #[cfg(all(feature = "tokio-host", feature = "embassy-host"))]
    pub use prns_runtime_embassy::runtime::{
        CompletionPool, EmbassyFleet, EmbassyInterfaceStore, MemberWire,
        PrnsNode as EmbassyPrnsNode, PrnsNodeHandle as EmbassyPrnsNodeHandle, ReactorPlumbing,
    };
}

mod lane_guards;
pub mod prelude;
pub use prelude::*;
