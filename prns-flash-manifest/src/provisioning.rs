use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use thiserror::Error;
use url::{Host, Url};

/// Flash offset reserved for local Hopspot configuration.
pub const CONFIG_OFFSET: u32 = 0xD000;
/// Size of the reserved configuration slot.
pub const CONFIG_SIZE: usize = 0x1000;
/// Configuration magic bytes.
pub const CONFIG_MAGIC: &[u8; 8] = b"HSPCFG1\0";
/// Current configuration format version.
pub const CONFIG_VERSION: u8 = 1;
/// Maximum encoded SSID length.
pub const CONFIG_SSID_MAX_BYTES: usize = 32;
/// Maximum encoded password length.
pub const CONFIG_PASSWORD_MAX_BYTES: usize = 64;
pub const CONFIG_TCP_CLIENT_KIND_OFFSET: usize = 9;
pub const CONFIG_TCP_CLIENT_HOST_LENGTH_OFFSET: usize =
    16 + CONFIG_SSID_MAX_BYTES + CONFIG_PASSWORD_MAX_BYTES;
pub const CONFIG_TCP_CLIENT_PORT_OFFSET: usize = CONFIG_TCP_CLIENT_HOST_LENGTH_OFFSET + 1;
pub const CONFIG_TCP_CLIENT_TARGET_OFFSET: usize = CONFIG_TCP_CLIENT_PORT_OFFSET + 2;
pub const CONFIG_TCP_CLIENT_HOSTNAME_MAX_BYTES: usize = 253;
pub const DEFAULT_TCP_CLIENT_PORT: u16 = 4242;

/// User-selected provisioning behavior.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvisioningAction {
    /// Leave the flash slot untouched.
    #[default]
    Preserve,
    /// Write supplied credentials.
    Configure(WifiCredentials),
    ConfigureWithTcp {
        wifi: WifiCredentials,
        tcp_client: TcpClientEndpoint,
    },
    /// Explicitly write an empty configuration image.
    Clear,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpClientEndpoint {
    pub host: TcpClientHost,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TcpClientHost {
    Ipv4(Ipv4Addr),
    Hostname(String),
}

impl TcpClientEndpoint {
    pub fn parse(value: &str) -> Result<Self, ProvisioningError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ProvisioningError::InvalidTcpTarget);
        }
        if let Ok(address) = value.parse::<std::net::SocketAddrV4>() {
            return Self {
                host: TcpClientHost::Ipv4(*address.ip()),
                port: address.port(),
            }
            .validated();
        }
        if let Ok(address) = value.parse::<Ipv4Addr>() {
            return Self {
                host: TcpClientHost::Ipv4(address),
                port: DEFAULT_TCP_CLIENT_PORT,
            }
            .validated();
        }
        let candidate = if value.contains("://") {
            value.to_string()
        } else {
            format!("tcp://{value}")
        };
        let url = Url::parse(&candidate).map_err(|_| ProvisioningError::InvalidTcpTarget)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ProvisioningError::InvalidTcpTarget);
        }
        let port = url.port().unwrap_or(DEFAULT_TCP_CLIENT_PORT);
        let host = match url.host() {
            Some(Host::Ipv4(address)) => TcpClientHost::Ipv4(address),
            Some(Host::Domain(hostname)) => match hostname.parse::<Ipv4Addr>() {
                Ok(address) => TcpClientHost::Ipv4(address),
                Err(_) => TcpClientHost::Hostname(
                    hostname
                        .strip_suffix('.')
                        .unwrap_or(hostname)
                        .to_ascii_lowercase(),
                ),
            },
            Some(Host::Ipv6(_)) => return Err(ProvisioningError::UnsupportedTcpIpv6),
            None => return Err(ProvisioningError::InvalidTcpTarget),
        };
        Self { host, port }.validated()
    }

    fn validated(self) -> Result<Self, ProvisioningError> {
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), ProvisioningError> {
        match &self.host {
            TcpClientHost::Ipv4(address)
                if address.is_unspecified()
                    || address.is_multicast()
                    || *address == Ipv4Addr::BROADCAST =>
            {
                return Err(ProvisioningError::InvalidTcpAddress(*address));
            }
            TcpClientHost::Ipv4(_) => {}
            TcpClientHost::Hostname(hostname) => validate_hostname(hostname)?,
        }
        if self.port == 0 {
            return Err(ProvisioningError::InvalidTcpPort);
        }
        Ok(())
    }
}

fn validate_hostname(hostname: &str) -> Result<(), ProvisioningError> {
    if hostname.len() > CONFIG_TCP_CLIENT_HOSTNAME_MAX_BYTES {
        return Err(ProvisioningError::TcpHostnameTooLong {
            actual: hostname.len(),
            maximum: CONFIG_TCP_CLIENT_HOSTNAME_MAX_BYTES,
        });
    }
    let valid = !hostname.is_empty()
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
        });
    if valid {
        Ok(())
    } else {
        Err(ProvisioningError::InvalidTcpHostname)
    }
}

/// Wi-Fi credentials encoded locally into the Hopspot configuration slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WifiCredentials {
    /// Network SSID.
    pub ssid: String,
    /// Network password; empty is valid for an open network.
    pub password: String,
}

impl WifiCredentials {
    /// Validate encoded byte lengths without truncation.
    pub fn validate(&self) -> Result<(), ProvisioningError> {
        let ssid_len = self.ssid.len();
        let password_len = self.password.len();
        if ssid_len == 0 {
            return Err(ProvisioningError::EmptySsid);
        }
        if ssid_len > CONFIG_SSID_MAX_BYTES {
            return Err(ProvisioningError::SsidTooLong {
                actual: ssid_len,
                maximum: CONFIG_SSID_MAX_BYTES,
            });
        }
        if password_len > CONFIG_PASSWORD_MAX_BYTES {
            return Err(ProvisioningError::PasswordTooLong {
                actual: password_len,
                maximum: CONFIG_PASSWORD_MAX_BYTES,
            });
        }
        Ok(())
    }
}

/// Provisioning image failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProvisioningError {
    /// Configure requires a non-empty SSID.
    #[error("Wi-Fi SSID cannot be empty")]
    EmptySsid,
    /// SSID exceeds the wire-format limit.
    #[error("Wi-Fi SSID is {actual} bytes; maximum is {maximum}")]
    SsidTooLong {
        /// Encoded length.
        actual: usize,
        /// Maximum encoded length.
        maximum: usize,
    },
    /// Password exceeds the wire-format limit.
    #[error("Wi-Fi password is {actual} bytes; maximum is {maximum}")]
    PasswordTooLong {
        /// Encoded length.
        actual: usize,
        /// Maximum encoded length.
        maximum: usize,
    },
    #[error("TCP client IPv4 address {0} is not a usable unicast target")]
    InvalidTcpAddress(Ipv4Addr),
    #[error("TCP client hostname is {actual} bytes; maximum is {maximum}")]
    TcpHostnameTooLong { actual: usize, maximum: usize },
    #[error("TCP client hostname must be a canonical lowercase DNS name")]
    InvalidTcpHostname,
    #[error("TCP client port must be between 1 and 65535")]
    InvalidTcpPort,
    #[error("TCP client target is not a valid IPv4 address, DNS hostname, or URL")]
    InvalidTcpTarget,
    #[error("embedded TCP client provisioning currently accepts IPv4 and DNS hostnames, not IPv6 literals")]
    UnsupportedTcpIpv6,
}

/// Build a configuration-slot image, or `None` when preserving the slot.
pub fn provisioning_image(
    action: &ProvisioningAction,
) -> Result<Option<Vec<u8>>, ProvisioningError> {
    if matches!(action, ProvisioningAction::Preserve) {
        return Ok(None);
    }

    let (credentials, tcp_client) = match action {
        ProvisioningAction::Configure(credentials) => {
            credentials.validate()?;
            (Some(credentials), None)
        }
        ProvisioningAction::ConfigureWithTcp { wifi, tcp_client } => {
            wifi.validate()?;
            tcp_client.validate()?;
            (Some(wifi), Some(tcp_client))
        }
        ProvisioningAction::Clear => (None, None),
        ProvisioningAction::Preserve => return Ok(None),
    };

    let mut bytes = vec![0xff; CONFIG_SIZE];
    bytes[..CONFIG_MAGIC.len()].copy_from_slice(CONFIG_MAGIC);
    bytes[8] = CONFIG_VERSION;
    bytes[CONFIG_TCP_CLIENT_KIND_OFFSET] = match tcp_client.map(|endpoint| &endpoint.host) {
        None => 0,
        Some(TcpClientHost::Ipv4(_)) => 1,
        Some(TcpClientHost::Hostname(_)) => 2,
    };
    if let Some(credentials) = credentials {
        let ssid = credentials.ssid.as_bytes();
        let password = credentials.password.as_bytes();
        bytes[10] = ssid.len() as u8;
        bytes[11] = password.len() as u8;
        bytes[16..16 + ssid.len()].copy_from_slice(ssid);
        let password_start = 16 + CONFIG_SSID_MAX_BYTES;
        bytes[password_start..password_start + password.len()].copy_from_slice(password);
    } else {
        bytes[10] = 0;
        bytes[11] = 0;
    }
    if let Some(tcp_client) = tcp_client {
        bytes[CONFIG_TCP_CLIENT_PORT_OFFSET..CONFIG_TCP_CLIENT_PORT_OFFSET + 2]
            .copy_from_slice(&tcp_client.port.to_be_bytes());
        match &tcp_client.host {
            TcpClientHost::Ipv4(address) => {
                bytes[CONFIG_TCP_CLIENT_HOST_LENGTH_OFFSET] = 4;
                bytes[CONFIG_TCP_CLIENT_TARGET_OFFSET..CONFIG_TCP_CLIENT_TARGET_OFFSET + 4]
                    .copy_from_slice(&address.octets());
            }
            TcpClientHost::Hostname(hostname) => {
                bytes[CONFIG_TCP_CLIENT_HOST_LENGTH_OFFSET] = hostname.len() as u8;
                bytes[CONFIG_TCP_CLIENT_TARGET_OFFSET
                    ..CONFIG_TCP_CLIENT_TARGET_OFFSET + hostname.len()]
                    .copy_from_slice(hostname.as_bytes());
            }
        }
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserve_emits_no_part() -> Result<(), ProvisioningError> {
        assert_eq!(provisioning_image(&ProvisioningAction::Preserve)?, None);
        Ok(())
    }

    #[test]
    fn clear_is_a_versioned_empty_image() -> Result<(), ProvisioningError> {
        let image = provisioning_image(&ProvisioningAction::Clear)?;
        let image = image.ok_or(ProvisioningError::EmptySsid)?;
        assert_eq!(image.len(), CONFIG_SIZE);
        assert_eq!(&image[..CONFIG_MAGIC.len()], CONFIG_MAGIC);
        assert_eq!(image[8], CONFIG_VERSION);
        assert_eq!((image[10], image[11]), (0, 0));
        assert_eq!(image[CONFIG_TCP_CLIENT_KIND_OFFSET], 0);
        Ok(())
    }

    #[test]
    fn utf8_limits_are_measured_in_bytes() {
        let credentials = WifiCredentials {
            ssid: "é".repeat(17),
            password: String::new(),
        };
        assert!(matches!(
            credentials.validate(),
            Err(ProvisioningError::SsidTooLong { actual: 34, .. })
        ));
    }

    #[test]
    fn tcp_client_is_encoded_after_legacy_wifi_fields() -> Result<(), ProvisioningError> {
        let image = provisioning_image(&ProvisioningAction::ConfigureWithTcp {
            wifi: WifiCredentials {
                ssid: "mesh".to_string(),
                password: "private".to_string(),
            },
            tcp_client: TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::new(192, 0, 2, 10)),
                port: 4242,
            },
        })?
        .ok_or(ProvisioningError::EmptySsid)?;
        assert_eq!(image[CONFIG_TCP_CLIENT_KIND_OFFSET], 1);
        assert_eq!(
            &image[CONFIG_TCP_CLIENT_TARGET_OFFSET..CONFIG_TCP_CLIENT_TARGET_OFFSET + 4],
            &[192, 0, 2, 10]
        );
        assert_eq!(
            &image[CONFIG_TCP_CLIENT_PORT_OFFSET..CONFIG_TCP_CLIENT_PORT_OFFSET + 2],
            &4242_u16.to_be_bytes()
        );
        Ok(())
    }

    #[test]
    fn tcp_client_rejects_non_unicast_and_zero_port() {
        for endpoint in [
            TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::UNSPECIFIED),
                port: 4242,
            },
            TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::BROADCAST),
                port: 4242,
            },
            TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::new(224, 0, 0, 1)),
                port: 4242,
            },
            TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::new(192, 0, 2, 10)),
                port: 0,
            },
        ] {
            assert!(endpoint.validate().is_err());
        }
    }

    #[test]
    fn hostname_target_is_canonical_and_bounded() -> Result<(), ProvisioningError> {
        let endpoint = TcpClientEndpoint {
            host: TcpClientHost::Hostname("node.example".to_string()),
            port: 4242,
        };
        endpoint.validate()?;
        let image = provisioning_image(&ProvisioningAction::ConfigureWithTcp {
            wifi: WifiCredentials {
                ssid: "mesh".to_string(),
                password: String::new(),
            },
            tcp_client: endpoint,
        })?
        .ok_or(ProvisioningError::EmptySsid)?;
        assert_eq!(image[CONFIG_TCP_CLIENT_KIND_OFFSET], 2);
        assert_eq!(image[CONFIG_TCP_CLIENT_HOST_LENGTH_OFFSET], 12);
        assert_eq!(
            &image[CONFIG_TCP_CLIENT_TARGET_OFFSET..CONFIG_TCP_CLIENT_TARGET_OFFSET + 12],
            b"node.example"
        );
        for hostname in ["", "Node.example", "-node.example", "node..example"] {
            assert_eq!(
                TcpClientEndpoint {
                    host: TcpClientHost::Hostname(hostname.to_string()),
                    port: 4242,
                }
                .validate(),
                Err(ProvisioningError::InvalidTcpHostname)
            );
        }
        Ok(())
    }

    #[test]
    fn user_targets_parse_to_one_canonical_endpoint() -> Result<(), ProvisioningError> {
        assert_eq!(
            TcpClientEndpoint::parse("192.0.2.10")?,
            TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::new(192, 0, 2, 10)),
                port: 4242,
            }
        );
        assert_eq!(
            TcpClientEndpoint::parse("https://Node.Example.:5252/path")?,
            TcpClientEndpoint {
                host: TcpClientHost::Hostname("node.example".to_string()),
                port: 5252,
            }
        );
        assert_eq!(
            TcpClientEndpoint::parse("node.example")?,
            TcpClientEndpoint {
                host: TcpClientHost::Hostname("node.example".to_string()),
                port: 4242,
            }
        );
        assert_eq!(
            TcpClientEndpoint::parse("[2001:db8::1]:4242"),
            Err(ProvisioningError::UnsupportedTcpIpv6)
        );
        Ok(())
    }
}
