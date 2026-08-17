#[cfg(feature = "board-t096")]
pub(crate) mod t096;
#[cfg(feature = "board-t114")]
pub(crate) mod t114;
#[cfg(feature = "board-t-echo")]
pub(crate) mod t_echo;

#[cfg(all(
    feature = "board-t096",
    not(feature = "board-t-echo"),
    not(feature = "board-t114")
))]
#[allow(unused_imports)] // Reserved for the runtime once the bring-up boundary is cleared.
pub(crate) use t096 as selected;
#[cfg(all(
    feature = "board-t114",
    not(feature = "board-t-echo"),
    not(feature = "board-t096")
))]
pub(crate) use t114 as selected;
#[cfg(all(
    feature = "board-t-echo",
    not(feature = "board-t096"),
    not(feature = "board-t114")
))]
pub(crate) use t_echo as selected;
