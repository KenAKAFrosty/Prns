#![no_std]

#[cfg(not(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2"
)))]
compile_error!(
    "select exactly one nRF52840 board feature; available: board-t-echo, board-t096, board-t114, board-t1000e, board-mesh-tower-v2"
);

#[cfg(any(
    all(feature = "board-t-echo", feature = "board-t096"),
    all(feature = "board-t-echo", feature = "board-t114"),
    all(feature = "board-t-echo", feature = "board-t1000e"),
    all(feature = "board-t-echo", feature = "board-mesh-tower-v2"),
    all(feature = "board-t096", feature = "board-t114"),
    all(feature = "board-t096", feature = "board-t1000e"),
    all(feature = "board-t096", feature = "board-mesh-tower-v2"),
    all(feature = "board-t114", feature = "board-t1000e"),
    all(feature = "board-t114", feature = "board-mesh-tower-v2"),
    all(feature = "board-t1000e", feature = "board-mesh-tower-v2")
))]
compile_error!("nRF52840 board features are mutually exclusive");

#[cfg(all(
    feature = "board-t-echo",
    not(any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))
))]
compile_error!("T-Echo requires exactly one S140 compatibility feature");

#[cfg(all(feature = "board-mesh-tower-v2", not(feature = "softdevice-s140-v6")))]
compile_error!("MeshTower V2 requires softdevice-s140-v6; HT-n5262 ships S140 6.1.1");

#[cfg(all(feature = "board-t096", not(feature = "softdevice-s140-v6")))]
compile_error!("T096 requires softdevice-s140-v6; HT-n5262G ships S140 6.1.1");

#[cfg(all(feature = "board-t114", not(feature = "softdevice-s140-v6")))]
compile_error!("T114 requires softdevice-s140-v6; HT-n5262 ships S140 6.1.1");

#[cfg(all(feature = "board-mesh-tower-v2", feature = "softdevice-s140-v7"))]
compile_error!("MeshTower V2 does not support S140 7.x");

#[cfg(all(feature = "board-t096", feature = "softdevice-s140-v7"))]
compile_error!("T096 does not support S140 7.x");

#[cfg(all(feature = "board-t114", feature = "softdevice-s140-v7"))]
compile_error!("T114 does not support S140 7.x");

#[cfg(all(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
compile_error!("S140 compatibility features are mutually exclusive");

#[cfg(all(
    feature = "board-t1000e",
    any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7")
))]
compile_error!("T1000-E does not support S140 compatibility features");

mod boards;
#[cfg(any(feature = "board-t096", feature = "board-t114"))]
mod immediate_display;
#[cfg(feature = "board-t-echo")]
mod retained_display;
#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-t096",
        not(feature = "board-t-echo"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-mesh-tower-v2",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e")
    )
))]
mod runtime;
mod storage;

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-t096",
        not(feature = "board-t-echo"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-t1000e",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-tower-v2")
    ),
    all(
        feature = "board-mesh-tower-v2",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e")
    )
))]
pub use runtime::run;
