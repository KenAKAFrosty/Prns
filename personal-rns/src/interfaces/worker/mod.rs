mod core;
mod interface;

pub use core::{QueueFull, SendError};
pub use interface::{
    DriverMode, Interface, InterfaceHandle, NextScheduledInterfaceWake, RegisteredInterface,
    SelfDrivenInterface, StartedInterface,
};
