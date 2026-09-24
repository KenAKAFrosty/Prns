#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo"
))]
mod bluetooth_auto;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo"
))]
mod bluetooth_gatt_server;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo"
))]
mod bootloader_entry;
mod entropy;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
mod firmware;
#[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
pub(crate) mod gnss;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo"
))]
mod headless;
mod heartbeat;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
mod interface_cards;
mod learned_state;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
pub(crate) mod node;
#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
mod remote_control;
#[cfg(any(
    feature = "board-t-echo",
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-mesh-pocket",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo"
))]
pub(crate) mod software_vbus;
#[cfg(feature = "usb-debug-log")]
mod usb_debug;

#[cfg(any(feature = "board-t-echo", feature = "board-mesh-pocket"))]
pub use firmware::run;
#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2",
    feature = "board-muzi-base-duo"
))]
pub use headless::run;
