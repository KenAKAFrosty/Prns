#![no_std]

#[cfg(not(any(feature = "board-t-echo", feature = "board-t096")))]
compile_error!("select exactly one nRF52840 board feature; available: board-t-echo, board-t096");

#[cfg(all(feature = "board-t-echo", feature = "board-t096"))]
compile_error!("nRF52840 board features are mutually exclusive");

#[cfg(all(
    feature = "board-t-echo",
    not(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))
))]
compile_error!("T-Echo requires exactly one S140 compatibility feature");

#[cfg(all(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
compile_error!("S140 compatibility features are mutually exclusive");

#[cfg(any(
    all(feature = "board-t-echo", not(feature = "board-t096")),
    all(feature = "board-t096", not(feature = "board-t-echo"))
))]
mod boards;
#[cfg(all(feature = "board-t-echo", not(feature = "board-t096")))]
mod runtime;

#[cfg(all(feature = "board-t-echo", not(feature = "board-t096")))]
pub use runtime::run;
