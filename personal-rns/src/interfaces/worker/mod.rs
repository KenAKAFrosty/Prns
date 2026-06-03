//! The interface contract — how an interface meets the runtime.
//!
//! An interface owns its byte I/O, peers, fan-out, and discovery opaquely and runs on
//! its own (a thread, an embassy task). It meets the runtime through a narrow seam
//! ([`InterfaceWorkerContext`](crate::interfaces::InterfaceWorkerContext) /
//! [`InterfaceHandle`]): the runtime hands it packets to
//! send and drains the inbound it submits; nothing below the interface (peer
//! addresses, sockets, the socket's platform) ever surfaces. An [`Interface`] starts
//! itself into a [`StartedInterface`] the runtime pools; [`SelfDrivenInterface`] is the
//! ready-made adapter for the common self-driven case.

mod core;
mod interface;

pub use core::{QueueFull, SendError};
pub use interface::{
    DriverMode, Interface, InterfaceHandle, NextScheduledInterfaceWake, RegisteredInterface,
    SelfDrivenInterface, StartedInterface,
};
