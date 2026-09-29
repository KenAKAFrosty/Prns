pub mod core;
mod correlation;
#[cfg(test)]
mod correlation_tests;
mod impls;
#[cfg(test)]
mod stream_size_tests;
#[cfg(test)]
mod value_tests;
pub use correlation::AssemblyCorrelation;
pub use impls::*;

pub use self::core::*;
