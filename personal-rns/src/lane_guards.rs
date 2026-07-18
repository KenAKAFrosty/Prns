//! Every interface family's weak feature edges do nothing until a runtime lane enables
//! the crate that implements it — so a family named without its lane would otherwise be
//! a silent no-op. These guards turn each of those states into a compile error naming
//! the lane to add.

#[cfg(all(
    feature = "tcp",
    not(any(feature = "tokio-host", feature = "embassy-host"))
))]
compile_error!("the `tcp` family needs a runtime lane: enable `tokio-host` or `embassy-host`");

#[cfg(all(
    feature = "wifi",
    not(any(feature = "tokio-host", feature = "embassy-host"))
))]
compile_error!("the `wifi` family needs a runtime lane: enable `tokio-host` or `embassy-host`");

#[cfg(all(
    feature = "usb",
    not(any(feature = "tokio-host", feature = "embassy-host"))
))]
compile_error!("the `usb` family needs a runtime lane: enable `tokio-host` or `embassy-host`");

#[cfg(all(
    feature = "ble",
    not(any(feature = "tokio-host", feature = "embassy-host"))
))]
compile_error!("the `ble` family needs a runtime lane: enable `tokio-host` or `embassy-host`");

#[cfg(all(feature = "wifi-direct", not(feature = "tokio-host")))]
compile_error!("the `wifi-direct` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "udp", not(feature = "tokio-host")))]
compile_error!("the `udp` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "serial", not(feature = "tokio-host")))]
compile_error!("the `serial` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "kiss", not(feature = "tokio-host")))]
compile_error!("the `kiss` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "ax25", not(feature = "tokio-host")))]
compile_error!("the `ax25` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "rnode", not(feature = "tokio-host")))]
compile_error!("the `rnode` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "pipe", not(feature = "tokio-host")))]
compile_error!("the `pipe` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "backbone", not(feature = "tokio-host")))]
compile_error!("the `backbone` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "websocket", not(feature = "tokio-host")))]
compile_error!("the `websocket` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "i2p", not(feature = "tokio-host")))]
compile_error!("the `i2p` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "shared-instance", not(feature = "tokio-host")))]
compile_error!("the `shared-instance` family is tokio-only: enable `tokio-host`");

#[cfg(all(feature = "lora", not(feature = "embassy-host")))]
compile_error!("the `lora` family is embassy-only: enable `embassy-host`");

#[cfg(all(feature = "ble-trouble", not(feature = "embassy-host")))]
compile_error!("the `ble-trouble` backend is embassy-only: enable `embassy-host`");

#[cfg(all(feature = "esp-now", not(feature = "embassy-host")))]
compile_error!("the `esp-now` family is embassy-only: enable `embassy-host`");

#[cfg(all(feature = "usb-device", not(feature = "embassy-host")))]
compile_error!("the `usb-device` family is embassy-only: enable `embassy-host`");
