#[cfg(all(feature = "ax25", feature = "tokio-host"))]
pub mod ax25_kiss;
#[cfg(all(feature = "backbone", feature = "tokio-host"))]
pub mod backbone;
#[cfg(all(
    feature = "bluetooth-auto",
    any(feature = "tokio-host", feature = "embassy-host")
))]
pub mod bluetooth_auto;
#[cfg(all(feature = "browser-rendezvous", feature = "tokio-host"))]
pub mod browser_rendezvous;
#[cfg(all(feature = "esp-now", feature = "embassy-host"))]
pub mod esp_now;
#[cfg(all(feature = "i2p", feature = "tokio-host"))]
pub mod i2p;
#[cfg(all(feature = "kiss", feature = "tokio-host"))]
pub mod kiss;
#[cfg(all(feature = "lora", feature = "embassy-host"))]
pub mod lora;
#[cfg(all(feature = "pipe", feature = "tokio-host"))]
pub mod pipe;
#[cfg(all(feature = "lora", feature = "embassy-host"))]
pub mod radios;
#[cfg(all(feature = "rnode", feature = "tokio-host"))]
pub mod rnode;
#[cfg(all(feature = "serial", feature = "tokio-host"))]
pub mod serial;
#[cfg(all(feature = "shared-instance", feature = "tokio-host"))]
pub mod shared_instance;
#[cfg(all(feature = "tcp", any(feature = "tokio-host", feature = "embassy-host")))]
pub mod tcp;
#[cfg(all(feature = "udp", feature = "tokio-host"))]
pub mod udp;
#[cfg(all(feature = "usb", any(feature = "tokio-host", feature = "embassy-host")))]
pub mod usb_auto;
#[cfg(all(feature = "weave", feature = "tokio-host"))]
pub mod weave;
#[cfg(all(feature = "websocket", feature = "tokio-host"))]
pub mod websocket;
#[cfg(all(
    feature = "wifi-auto",
    any(feature = "tokio-host", feature = "embassy-host")
))]
pub mod wifi_auto;
#[cfg(all(feature = "wifi-aware", feature = "tokio-host"))]
pub mod wifi_aware;
#[cfg(all(feature = "wifi-direct", feature = "tokio-host"))]
pub mod wifi_direct;
