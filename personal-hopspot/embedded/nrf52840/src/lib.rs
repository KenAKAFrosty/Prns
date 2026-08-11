#![no_std]

#[cfg(not(feature = "board-t-echo"))]
compile_error!("select exactly one nRF52840 board feature; available: board-t-echo");

#[cfg(feature = "board-t-echo")]
mod boards;
#[cfg(feature = "board-t-echo")]
mod runtime;

#[cfg(feature = "board-t-echo")]
pub use runtime::run;
