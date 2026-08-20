#[cfg(any(
    feature = "board-t096",
    feature = "board-t114",
    feature = "board-t1000e",
    feature = "board-mesh-tower-v2"
))]
mod status_led;

#[cfg(feature = "board-mesh-tower-v2")]
pub(crate) mod mesh_tower_v2;
#[cfg(feature = "board-t096")]
pub(crate) mod t096;
#[cfg(feature = "board-t1000e")]
pub(crate) mod t1000e;
#[cfg(feature = "board-t114")]
pub(crate) mod t114;
#[cfg(feature = "board-t-echo")]
pub(crate) mod t_echo;

#[cfg(all(
    feature = "board-mesh-tower-v2",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-t1000e")
))]
pub(crate) use mesh_tower_v2 as selected;
#[cfg(all(
    feature = "board-t096",
    not(feature = "board-t-echo"),
    not(feature = "board-t114"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2")
))]
#[allow(unused_imports)] // Reserved for the runtime once the bring-up boundary is cleared.
pub(crate) use t096 as selected;
#[cfg(all(
    feature = "board-t1000e",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-mesh-tower-v2")
))]
pub(crate) use t1000e as selected;
#[cfg(all(
    feature = "board-t114",
    not(feature = "board-t-echo"),
    not(feature = "board-t096"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2")
))]
pub(crate) use t114 as selected;
#[cfg(all(
    feature = "board-t-echo",
    not(feature = "board-t096"),
    not(feature = "board-t114"),
    not(feature = "board-t1000e"),
    not(feature = "board-mesh-tower-v2")
))]
pub(crate) use t_echo as selected;
