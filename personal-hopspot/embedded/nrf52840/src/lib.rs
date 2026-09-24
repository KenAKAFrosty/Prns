#![no_std]

#[cfg(not(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-pocket",
    feature = "board-t1000e",
    feature = "board-sensecap-solar-node",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    feature = "board-rak4631",
    feature = "board-rak10724",
    feature = "board-wio-tracker-l1"
)))]
compile_error!(
    "select exactly one nRF52840 board feature; available: board-t-echo, board-t096, board-t114, board-mesh-pocket, board-t1000e, board-mesh-tower-v2, board-muzi-base-duo, board-rak4631, board-wio-tracker-l1"
);

#[cfg(any(
    all(feature = "board-t-echo", feature = "board-t096"),
    all(feature = "board-t-echo", feature = "board-t114"),
    all(feature = "board-t-echo", feature = "board-mesh-pocket"),
    all(feature = "board-t-echo", feature = "board-t1000e"),
    all(feature = "board-t-echo", feature = "board-mesh-tower-v2"),
    all(feature = "board-t096", feature = "board-t114"),
    all(feature = "board-t096", feature = "board-mesh-pocket"),
    all(feature = "board-t096", feature = "board-t1000e"),
    all(feature = "board-t096", feature = "board-mesh-tower-v2"),
    all(feature = "board-t114", feature = "board-t1000e"),
    all(feature = "board-t114", feature = "board-mesh-tower-v2"),
    all(feature = "board-t114", feature = "board-mesh-pocket"),
    all(feature = "board-mesh-pocket", feature = "board-t1000e"),
    all(feature = "board-mesh-pocket", feature = "board-mesh-tower-v2"),
    all(feature = "board-t1000e", feature = "board-mesh-tower-v2"),
    all(feature = "board-t-echo", feature = "board-muzi-base-duo"),
    all(feature = "board-t096", feature = "board-muzi-base-duo"),
    all(feature = "board-t114", feature = "board-muzi-base-duo"),
    all(feature = "board-mesh-pocket", feature = "board-muzi-base-duo"),
    all(feature = "board-t1000e", feature = "board-muzi-base-duo"),
    all(feature = "board-mesh-tower-v2", feature = "board-muzi-base-duo"),
    all(feature = "board-t-echo", feature = "board-rak4631"),
    all(feature = "board-t096", feature = "board-rak4631"),
    all(feature = "board-t114", feature = "board-rak4631"),
    all(feature = "board-mesh-pocket", feature = "board-rak4631"),
    all(feature = "board-t1000e", feature = "board-rak4631"),
    all(feature = "board-mesh-tower-v2", feature = "board-rak4631"),
    all(feature = "board-muzi-base-duo", feature = "board-rak4631"),
    all(feature = "board-t-echo", feature = "board-wio-tracker-l1"),
    all(feature = "board-t096", feature = "board-wio-tracker-l1"),
    all(feature = "board-t114", feature = "board-wio-tracker-l1"),
    all(feature = "board-mesh-pocket", feature = "board-wio-tracker-l1"),
    all(feature = "board-t1000e", feature = "board-wio-tracker-l1"),
    all(feature = "board-mesh-tower-v2", feature = "board-wio-tracker-l1"),
    all(feature = "board-muzi-base-duo", feature = "board-wio-tracker-l1"),
    all(feature = "board-rak4631", feature = "board-wio-tracker-l1"),
    all(feature = "board-sensecap-solar-node", feature = "board-t-echo"),
    all(feature = "board-sensecap-solar-node", feature = "board-t096"),
    all(feature = "board-sensecap-solar-node", feature = "board-t114"),
    all(feature = "board-sensecap-solar-node", feature = "board-mesh-pocket"),
    all(feature = "board-sensecap-solar-node", feature = "board-t1000e"),
    all(feature = "board-sensecap-solar-node", feature = "board-mesh-tower-v2"),
    all(feature = "board-sensecap-solar-node", feature = "board-muzi-base-duo"),
    all(feature = "board-sensecap-solar-node", feature = "board-rak4631"),
    all(
        feature = "board-sensecap-solar-node",
        feature = "board-wio-tracker-l1"
    ),
    all(feature = "board-rak10724", feature = "board-t-echo"),
    all(feature = "board-rak10724", feature = "board-t096"),
    all(feature = "board-rak10724", feature = "board-t114"),
    all(feature = "board-rak10724", feature = "board-mesh-pocket"),
    all(feature = "board-rak10724", feature = "board-t1000e"),
    all(feature = "board-rak10724", feature = "board-mesh-tower-v2"),
    all(feature = "board-rak10724", feature = "board-muzi-base-duo"),
    all(feature = "board-rak10724", feature = "board-rak4631"),
    all(feature = "board-rak10724", feature = "board-wio-tracker-l1"),
    all(feature = "board-rak10724", feature = "board-sensecap-solar-node")
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

#[cfg(all(feature = "board-mesh-pocket", not(feature = "softdevice-s140-v6")))]
compile_error!("MeshPocket requires softdevice-s140-v6; HT-n5262 ships S140 6.1.1");

#[cfg(all(feature = "board-mesh-tower-v2", feature = "softdevice-s140-v7"))]
compile_error!("MeshTower V2 does not support S140 7.x");

#[cfg(all(feature = "board-muzi-base-duo", not(feature = "softdevice-s140-v6")))]
compile_error!("muzi Base Duo requires softdevice-s140-v6; the muzi bootloader ships S140 6.1.1");

#[cfg(all(feature = "board-muzi-base-duo", feature = "softdevice-s140-v7"))]
compile_error!("muzi Base Duo does not support S140 7.x");

#[cfg(all(feature = "board-rak4631", not(feature = "softdevice-s140-v6")))]
compile_error!("RAK4631 requires softdevice-s140-v6; its UF2 bootloader ships S140 6.1.1");

#[cfg(all(feature = "board-rak4631", feature = "softdevice-s140-v7"))]
compile_error!("RAK4631 does not support S140 7.x");

#[cfg(all(feature = "board-t096", feature = "softdevice-s140-v7"))]
compile_error!("T096 does not support S140 7.x");

#[cfg(all(feature = "board-t114", feature = "softdevice-s140-v7"))]
compile_error!("T114 does not support S140 7.x");

#[cfg(all(feature = "board-mesh-pocket", feature = "softdevice-s140-v7"))]
compile_error!("MeshPocket does not support S140 7.x");

#[cfg(all(feature = "board-wio-tracker-l1", not(feature = "softdevice-s140-v7")))]
compile_error!("Wio Tracker L1 requires softdevice-s140-v7; its UF2 bootloader ships S140 7.3.0");

#[cfg(all(feature = "board-wio-tracker-l1", feature = "softdevice-s140-v6"))]
compile_error!("Wio Tracker L1 does not support S140 6.x");

#[cfg(all(
    feature = "wio-tracker-l1-pro-1w",
    not(feature = "board-wio-tracker-l1")
))]
compile_error!("wio-tracker-l1-pro-1w requires board-wio-tracker-l1");

#[cfg(all(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7"))]
compile_error!("S140 compatibility features are mutually exclusive");

#[cfg(all(
    any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
    any(feature = "softdevice-s140-v6", feature = "softdevice-s140-v7")
))]
compile_error!("T1000-E does not support S140 compatibility features");

#[cfg(all(
    feature = "board-mesh-pocket",
    not(any(
        feature = "mesh-pocket-battery-5000",
        feature = "mesh-pocket-battery-10000"
    ))
))]
compile_error!("MeshPocket requires exactly one battery-capacity feature");

#[cfg(all(
    feature = "mesh-pocket-battery-5000",
    feature = "mesh-pocket-battery-10000"
))]
compile_error!("MeshPocket battery-capacity features are mutually exclusive");

#[cfg(all(
    not(feature = "board-mesh-pocket"),
    any(
        feature = "mesh-pocket-battery-5000",
        feature = "mesh-pocket-battery-10000"
    )
))]
compile_error!("MeshPocket battery-capacity features require board-mesh-pocket");

mod boards;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-wio-tracker-l1"
))]
mod immediate_display;
mod memory;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
mod retained_display;
#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-t096",
        not(feature = "board-t-echo"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-mesh-pocket",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-mesh-tower-v2",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-muzi-base-duo",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        any(feature = "board-rak4631", feature = "board-rak10724"),
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-wio-tracker-l1",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631")
    )
))]
mod runtime;
mod storage;

#[cfg(any(
    all(
        feature = "board-t-echo",
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-t096",
        not(feature = "board-t-echo"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-t114",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-mesh-pocket",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-mesh-tower-v2",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-muzi-base-duo",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-rak4631"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        any(feature = "board-rak4631", feature = "board-rak10724"),
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-wio-tracker-l1")
    ),
    all(
        feature = "board-wio-tracker-l1",
        not(feature = "board-t-echo"),
        not(feature = "board-t096"),
        not(feature = "board-t114"),
        not(feature = "board-mesh-pocket"),
        not(feature = "board-t1000e"),
        not(feature = "board-mesh-tower-v2"),
        not(feature = "board-muzi-base-duo"),
        not(feature = "board-rak4631")
    )
))]
pub use runtime::run;

#[cfg(all(feature = "board-rak10724", not(feature = "softdevice-s140-v6")))]
compile_error!("RAK10724 requires softdevice-s140-v6");
#[cfg(all(feature = "board-rak10724", feature = "softdevice-s140-v7"))]
compile_error!("RAK10724 does not support S140 7.x");
