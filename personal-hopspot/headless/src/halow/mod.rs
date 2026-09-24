//! Attachment only: OpenWrt retains ownership of radio settings.
use personal_rns::interfaces::wifi_halow::InstanceTag;
use std::num::{NonZeroU32, NonZeroU8};

#[derive(Debug, clap::Args)]
pub struct HaLowOptions {
    /// Already configured Linux HaLoW network device (requires CAP_NET_RAW).
    #[arg(long, requires = "halow_scope")]
    pub halow_device: Option<String>,
    /// Stable local radio identity, 1–64 bytes; preserve across device renames.
    #[arg(long, requires = "halow_device", value_parser = scope)]
    pub halow_scope: Option<String>,
    /// Maximum admitted MAC peers; each owns bounded runtime and receive queues.
    #[arg(long, default_value = "16", requires = "halow_device")]
    pub halow_peers: NonZeroU8,
    /// Drop an admitted peer after this many idle seconds; no announces are scheduled.
    #[arg(long, default_value = "900", requires = "halow_device", value_parser = idle_seconds)]
    pub halow_idle_seconds: NonZeroU32,
}
fn scope(value: &str) -> Result<String, &'static str> {
    InstanceTag::new(value.as_bytes()).map_err(|_| "scope must contain 1–64 bytes")?;
    Ok(value.to_owned())
}
fn idle_seconds(value: &str) -> Result<NonZeroU32, &'static str> {
    let value: NonZeroU32 = value.parse().map_err(|_| "expected positive seconds")?;
    if value.get() > 86_400 {
        return Err("peer idle timeout must be at most one day");
    }
    Ok(value)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(not(target_os = "linux"))]
    #[error("HaLoW attachment is supported only on Linux")]
    Unsupported,
    #[error("HaLoW requires both device and stable scope")]
    Incomplete,
    #[cfg(target_os = "linux")]
    #[error("invalid HaLoW scope: {0:?}")]
    Scope(personal_rns::interfaces::wifi_halow::InstanceTagError),
    #[cfg(target_os = "linux")]
    #[error("cannot bind HaLoW device: {0}")]
    Bind(std::io::Error),
    #[cfg(target_os = "linux")]
    #[error("invalid experimental EtherType")]
    Protocol,
}

pub struct Prepared {
    #[cfg(target_os = "linux")]
    radio: personal_rns::wifi_halow::HaLowDevice<personal_rns::wifi_halow::LinuxHaLowRadio>,
    broadcast: personal_rns::interfaces::InterfaceId,
}
impl HaLowOptions {
    pub fn prepare(&self) -> Result<Option<Prepared>, Error> {
        let (device, scope) = match (&self.halow_device, &self.halow_scope) {
            (None, None) => return Ok(None),
            (Some(device), Some(scope)) => (device, scope),
            _ => return Err(Error::Incomplete),
        };
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (device, scope);
            Err(Error::Unsupported)
        }
        #[cfg(target_os = "linux")]
        {
            use personal_rns::interfaces::{BitrateBps, InterfaceId, InterfaceKind};
            use personal_rns::wifi_halow::{EtherType, HaLowDevice, HaLowLimits, LinuxHaLowRadio};
            let scope = InstanceTag::new(scope.as_bytes()).map_err(Error::Scope)?;
            let protocol = EtherType::new(0x88b6).map_err(|_| Error::Protocol)?;
            let source = LinuxHaLowRadio::new(device.clone(), protocol).map_err(Error::Bind)?;
            let broadcast = InterfaceId::from_channel_tag(
                InterfaceKind::WifiHaLowBroadcast,
                &scope.channel_tag(),
            );
            // Conservative estimates from the 8 MHz/MCS2 lab profile, not PHY rates.
            let peer_policy = personal_rns::interfaces::wifi_halow::policy_for_bitrate(
                BitrateBps::guess(7_300_000),
            );
            let broadcast_policy = personal_rns::interfaces::wifi_halow::policy_for_bitrate(
                BitrateBps::guess(4_000_000),
            );
            let idle_seconds = self.halow_idle_seconds;
            Ok(Some(Prepared {
                radio: HaLowDevice::new(
                    source,
                    scope,
                    peer_policy,
                    broadcast_policy,
                    HaLowLimits {
                        peers: self.halow_peers,
                        idle_seconds,
                    },
                    personal_rns::prelude::ReconnectPolicy::STANDARD,
                ),
                broadcast,
            }))
        }
    }
}
impl Prepared {
    pub fn attach(
        self,
        handle: &personal_rns::runtime::PrnsNodeHandle,
    ) -> personal_rns::interfaces::InterfaceId {
        #[cfg(target_os = "linux")]
        handle.supervise(self.radio);
        #[cfg(not(target_os = "linux"))]
        let _ = handle;
        self.broadcast
    }
}

#[cfg(test)]
mod tests;
