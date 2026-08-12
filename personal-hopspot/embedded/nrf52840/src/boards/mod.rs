#[cfg(feature = "board-t096")]
pub(crate) mod t096;
#[cfg(feature = "board-t-echo")]
pub(crate) mod t_echo;

#[cfg(all(feature = "board-t096", not(feature = "board-t-echo")))]
#[allow(unused_imports)] // Reserved for the runtime once the bring-up boundary is cleared.
pub(crate) use t096 as selected;
#[cfg(all(feature = "board-t-echo", not(feature = "board-t096")))]
pub(crate) use t_echo as selected;
