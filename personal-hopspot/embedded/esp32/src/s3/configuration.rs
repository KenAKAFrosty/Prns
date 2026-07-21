use super::*;

#[cfg(feature = "wifi-auto")]
#[derive(Clone, Debug)]
pub(super) struct HopspotWifiConfig {
    pub(super) ssid: String,
    pub(super) password: String,
}

#[cfg(feature = "wifi-auto")]
impl HopspotWifiConfig {
    fn from_build_env() -> Self {
        Self {
            ssid: WIFI_SSID.to_string(),
            password: WIFI_PASSWORD.to_string(),
        }
    }

    pub(super) fn has_station(&self) -> bool {
        !self.ssid.is_empty()
    }
}

#[cfg(feature = "wifi-auto")]
pub(super) fn hopspot_wifi_config() -> HopspotWifiConfig {
    read_hopspot_config_slot().unwrap_or_else(HopspotWifiConfig::from_build_env)
}

#[cfg(feature = "wifi-auto")]
fn read_hopspot_config_slot() -> Option<HopspotWifiConfig> {
    let mut words = [0u32; HOPSPOT_CONFIG_READ_WORDS];
    // SAFETY: `words` is aligned writable storage of the exact byte length passed to the ROM, and
    // HOPSPOT_CONFIG_OFFSET names the reserved read-only provisioning slot in the partition layout.
    let read = unsafe {
        esp_rom_spiflash_read(
            HOPSPOT_CONFIG_OFFSET,
            words.as_mut_ptr() as *const u32,
            (words.len() * core::mem::size_of::<u32>()) as u32,
        )
    };
    if read != 0 {
        return None;
    }
    // SAFETY: u8 has alignment 1 and this slice covers exactly the initialized `words` allocation;
    // its lifetime is bounded by `words` and the parser does not retain it.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            words.as_ptr() as *const u8,
            words.len() * core::mem::size_of::<u32>(),
        )
    };
    parse_hopspot_config(bytes)
}

#[cfg(feature = "wifi-auto")]
fn parse_hopspot_config(bytes: &[u8]) -> Option<HopspotWifiConfig> {
    if bytes.get(0..8)? != HOPSPOT_CONFIG_MAGIC || *bytes.get(8)? != HOPSPOT_CONFIG_VERSION {
        return None;
    }
    let ssid_len = (*bytes.get(10)? as usize).min(HOPSPOT_CONFIG_SSID_MAX);
    let password_len = (*bytes.get(11)? as usize).min(HOPSPOT_CONFIG_PASSWORD_MAX);
    let ssid = core::str::from_utf8(bytes.get(16..16 + ssid_len)?)
        .ok()?
        .to_string();
    let password_start = 16 + HOPSPOT_CONFIG_SSID_MAX;
    let password = core::str::from_utf8(bytes.get(password_start..password_start + password_len)?)
        .ok()?
        .to_string();
    Some(HopspotWifiConfig { ssid, password })
}
