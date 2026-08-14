#![no_std]

#[cfg(not(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t096",
        not(feature = "board-t-echo"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096")
    ),
)))]
compile_error!(
    "select exactly one nRF52840 board feature; available: board-t-echo, board-t096, board-t1000e"
);

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t096",
        not(feature = "board-t-echo"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096")
    ),
))]
mod boards;

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096")
    ),
))]
mod runtime;

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096")
    ),
))]
pub use runtime::run;
