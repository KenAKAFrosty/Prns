use super::*;

#[derive(Clone, Debug)]
pub(super) struct HopspotWifiConfig {
    pub(super) ssid: String,
    pub(super) password: String,
    pub(super) tcp_client: Option<HopspotTcpClientConfig>,
}

#[derive(Clone, Debug)]
pub(super) struct HopspotTcpClientConfig {
    pub(super) host: HopspotTcpClientHost,
    pub(super) port: u16,
}

#[derive(Clone, Debug)]
pub(super) enum HopspotTcpClientHost {
    Ipv4(core::net::Ipv4Addr),
    Hostname(String),
}

impl HopspotWifiConfig {
    fn from_build_env() -> Self {
        Self {
            ssid: WIFI_SSID.to_string(),
            password: WIFI_PASSWORD.to_string(),
            tcp_client: parse_tcp_client_target(HOPSPOT_TCP_TARGET),
        }
    }

    pub(super) fn has_station(&self) -> bool {
        !self.ssid.is_empty()
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum HopspotWifiConfigSource {
    Provisioning,
    BuildEnvironment,
}

pub(super) fn hopspot_wifi_config() -> (HopspotWifiConfig, HopspotWifiConfigSource) {
    #[cfg(feature = "wifi-security-probe")]
    if option_env!("HOPSPOT_WIFI_SECURITY_STATION_PROBE").is_some() {
        return (
            HopspotWifiConfig::from_build_env(),
            HopspotWifiConfigSource::BuildEnvironment,
        );
    }

    match read_hopspot_config_slot() {
        Some(config) => (config, HopspotWifiConfigSource::Provisioning),
        None => (
            HopspotWifiConfig::from_build_env(),
            HopspotWifiConfigSource::BuildEnvironment,
        ),
    }
}

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
    let tcp_client = parse_tcp_client(bytes);
    Some(HopspotWifiConfig {
        ssid,
        password,
        tcp_client,
    })
}

fn parse_tcp_client(bytes: &[u8]) -> Option<HopspotTcpClientConfig> {
    match *bytes.get(HOPSPOT_CONFIG_TCP_KIND_OFFSET)? {
        1 => {
            if *bytes.get(HOPSPOT_CONFIG_TCP_HOST_LENGTH_OFFSET)? != 4 {
                None
            } else {
                let address = core::net::Ipv4Addr::new(
                    *bytes.get(HOPSPOT_CONFIG_TCP_TARGET_OFFSET)?,
                    *bytes.get(HOPSPOT_CONFIG_TCP_TARGET_OFFSET + 1)?,
                    *bytes.get(HOPSPOT_CONFIG_TCP_TARGET_OFFSET + 2)?,
                    *bytes.get(HOPSPOT_CONFIG_TCP_TARGET_OFFSET + 3)?,
                );
                let port = read_tcp_port(bytes)?;
                valid_tcp_ipv4(address, port).then_some(HopspotTcpClientConfig {
                    host: HopspotTcpClientHost::Ipv4(address),
                    port,
                })
            }
        }
        2 => {
            let hostname_len = *bytes.get(HOPSPOT_CONFIG_TCP_HOST_LENGTH_OFFSET)? as usize;
            let hostname = core::str::from_utf8(bytes.get(
                HOPSPOT_CONFIG_TCP_TARGET_OFFSET..HOPSPOT_CONFIG_TCP_TARGET_OFFSET + hostname_len,
            )?)
            .ok()?;
            let port = read_tcp_port(bytes)?;
            valid_tcp_hostname(hostname, port).then(|| HopspotTcpClientConfig {
                host: HopspotTcpClientHost::Hostname(hostname.to_string()),
                port,
            })
        }
        _ => None,
    }
}

fn parse_tcp_client_target(value: &str) -> Option<HopspotTcpClientConfig> {
    if let Ok(target) = value.parse::<core::net::SocketAddrV4>() {
        return valid_tcp_ipv4(*target.ip(), target.port()).then_some(HopspotTcpClientConfig {
            host: HopspotTcpClientHost::Ipv4(*target.ip()),
            port: target.port(),
        });
    }
    if let Ok(address) = value.parse::<core::net::Ipv4Addr>() {
        return valid_tcp_ipv4(address, HOPSPOT_TCP_DEFAULT_PORT).then_some(
            HopspotTcpClientConfig {
                host: HopspotTcpClientHost::Ipv4(address),
                port: HOPSPOT_TCP_DEFAULT_PORT,
            },
        );
    }
    let (hostname, port) = match value.rsplit_once(':') {
        Some((hostname, port)) => (hostname, port.parse().ok()?),
        None => (value, HOPSPOT_TCP_DEFAULT_PORT),
    };
    valid_tcp_hostname(hostname, port).then(|| HopspotTcpClientConfig {
        host: HopspotTcpClientHost::Hostname(hostname.to_string()),
        port,
    })
}

fn read_tcp_port(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes([
        *bytes.get(HOPSPOT_CONFIG_TCP_PORT_OFFSET)?,
        *bytes.get(HOPSPOT_CONFIG_TCP_PORT_OFFSET + 1)?,
    ]))
}

fn valid_tcp_ipv4(address: core::net::Ipv4Addr, port: u16) -> bool {
    !address.is_unspecified()
        && !address.is_multicast()
        && address != core::net::Ipv4Addr::BROADCAST
        && port != 0
}

fn valid_tcp_hostname(hostname: &str, port: u16) -> bool {
    port != 0
        && !hostname.is_empty()
        && hostname.len() <= HOPSPOT_CONFIG_TCP_HOSTNAME_MAX
        && hostname.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && hostname.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && label
                    .bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
        })
}
