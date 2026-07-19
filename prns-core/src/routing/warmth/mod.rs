mod core;
mod departed;

pub use core::{RouteWarmth, WarmestOf};
pub use departed::{
    DepartedInterface, DepartedInterfaceTable, DepartedInterfaces, Departure,
    FixedDepartedInterfaceTable, DEPARTED_INTERFACE_GRACE_MS,
};

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        pub use departed::HeapDepartedInterfaceTable;
    }
}

#[cfg(test)]
mod tests;
