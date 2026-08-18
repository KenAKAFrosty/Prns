#![no_std]

#[cfg(not(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e"
)))]
compile_error!(
    "select exactly one nRF52840 board feature; available: board-t-echo, board-t096, board-t114, board-t1000e"
);

#[cfg(any(
    all(feature = "board-t-echo", feature = "board-t096"),
    all(feature = "board-t-echo", feature = "board-t114"),
    all(feature = "board-t-echo", feature = "board-t1000e"),
    all(feature = "board-t096", feature = "board-t114"),
    all(feature = "board-t096", feature = "board-t1000e"),
    all(feature = "board-t114", feature = "board-t1000e")
))]
compile_error!("nRF52840 board features are mutually exclusive");

#[cfg(all(
    feature = "board-t-echo",
    not(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))
))]
compile_error!("T-Echo requires exactly one S140 compatibility feature");

#[cfg(all(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
compile_error!("S140 compatibility features are mutually exclusive");

#[cfg(all(
    any(
        feature = "board-t096",
        feature = "board-t114",
        feature = "board-t1000e"
    ),
    any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7")
))]
compile_error!("only T-Echo supports S140 compatibility features");

mod boards;
#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114")
    )
))]
mod runtime;
mod storage;

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t1000e")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114")
    )
))]
pub use runtime::run;
