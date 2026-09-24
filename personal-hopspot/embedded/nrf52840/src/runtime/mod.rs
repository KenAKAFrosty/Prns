#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-wio-tracker-l1",
    feature = "board-t114",
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    any(feature = "board-rak4631", feature = "board-rak10724")
))]
mod bluetooth_auto;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-wio-tracker-l1",
    feature = "board-t114",
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    any(feature = "board-rak4631", feature = "board-rak10724")
))]
mod bluetooth_gatt_server;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-wio-tracker-l1",
    feature = "board-t114",
    any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    any(feature = "board-rak4631", feature = "board-rak10724")
))]
pub(crate) mod bootloader_entry;
mod controller_enrollment;
mod entropy;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
mod firmware;
#[cfg(any(
    feature = "board-t096",
    any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
    feature = "board-wio-tracker-l1"
))]
pub(crate) mod gnss;
#[cfg(any(
    feature = "board-t096",
    feature = "board-wio-tracker-l1",
    feature = "board-t114",
    any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    any(feature = "board-rak4631", feature = "board-rak10724")
))]
mod headless;
mod heartbeat;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
mod interface_cards;
mod learned_state;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
pub(crate) mod node;
mod node_name;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
mod remote_control;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-wio-tracker-l1",
    feature = "board-t114",
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    any(feature = "board-rak4631", feature = "board-rak10724")
))]
pub(crate) mod software_vbus;
#[cfg(not(feature = "board-muzi-base-duo"))]
mod subg_configuration;

#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
pub use firmware::run;
#[cfg(any(
    feature = "board-t096",
    feature = "board-wio-tracker-l1",
    feature = "board-t114",
    any(feature = "board-t1000e", feature = "board-sensecap-solar-node"),
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo",
    any(feature = "board-rak4631", feature = "board-rak10724")
))]
pub use headless::run;
