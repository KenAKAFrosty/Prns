cfg_if::cfg_if! {
    if #[cfg(feature = "board-tbeam-supreme")] {
        pub mod t_beam_supreme;
    } else {
        pub mod heltec_v4;
    }
}
