#![no_std]

#[cfg(not(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114"
)))]
compile_error!(
    "select exactly one nRF52840 board feature; available: board-t-echo, board-t096, board-t114"
);

#[cfg(any(
    all(feature = "board-t-echo", feature = "board-t096"),
    all(feature = "board-t-echo", feature = "board-t114"),
    all(feature = "board-t096", feature = "board-t114")
))]
compile_error!("nRF52840 board features are mutually exclusive");

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114")
    ),
    all(
        feature = "board-t096",
        not(feature = "board-t-echo"),
        not(feature = "board-t114")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096")
    )
))]
mod boards;

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096")
    )
))]
mod runtime;

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096")
    )
))]
pub use runtime::run;
