mod core;
mod impls;

pub use core::{
    DepartedInterface, DepartedInterfaceTable, DepartedInterfaces, Departure,
    DEPARTED_INTERFACE_GRACE_MS,
};
pub use impls::FixedDepartedInterfaceTable;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        pub use impls::HeapDepartedInterfaceTable;
    }
}

#[cfg(test)]
mod tests;
