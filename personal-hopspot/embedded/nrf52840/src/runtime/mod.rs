#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-tower-v2"
))]
mod bluetooth_auto;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-tower-v2"
))]
mod bluetooth_gatt_server;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2",
    feature = "board-sensecap-solar-node"
))]
mod bootloader_entry;
mod entropy;
#[cfg(feature = "board-t-echo")]
mod firmware;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t1000e",
    feature = "board-sensecap-solar-node"
))]
pub(crate) mod gnss;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2",
    feature = "board-sensecap-solar-node"
))]
mod headless;
mod heartbeat;
#[cfg(feature = "board-t-echo")]
mod interface_cards;
mod learned_state;
#[cfg(feature = "board-t-echo")]
pub(crate) mod node;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-tower-v2"
))]
pub(crate) mod software_vbus;

#[cfg(feature = "board-t-echo")]
pub use firmware::run;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2",
    feature = "board-sensecap-solar-node"
))]
pub use headless::run;
