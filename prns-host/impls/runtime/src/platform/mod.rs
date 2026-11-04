//! Optional native platform mechanisms. Product startup policy stays in composition.
#[cfg(feature = "android-platform")]
pub mod android;

mod lan;
pub use lan::{
    configure_apple_multicast, mobile_auto_lan_factory, mobile_auto_lan_network_changed,
    mobile_auto_lan_status, MobileAutoLanStatus,
};
