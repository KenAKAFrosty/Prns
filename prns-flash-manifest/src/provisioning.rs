use serde::{Deserialize, Serialize};
use thiserror::Error;

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

/// User-selected provisioning behavior.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvisioningAction {
    /// Leave the flash slot untouched.
    #[default]
    Preserve,
    /// Write supplied credentials.
    Configure(WifiCredentials),
    /// Explicitly write an empty configuration image.
    Clear,
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
}

/// Build a configuration-slot image, or `None` when preserving the slot.
pub fn provisioning_image(
    action: &ProvisioningAction,
) -> Result<Option<Vec<u8>>, ProvisioningError> {
    if matches!(action, ProvisioningAction::Preserve) {
        return Ok(None);
    }

    let credentials = match action {
        ProvisioningAction::Configure(credentials) => {
            credentials.validate()?;
            Some(credentials)
        }
        ProvisioningAction::Clear => None,
        ProvisioningAction::Preserve => return Ok(None),
    };

    let mut bytes = vec![0xff; CONFIG_SIZE];
    bytes[..CONFIG_MAGIC.len()].copy_from_slice(CONFIG_MAGIC);
    bytes[8] = CONFIG_VERSION;
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
}
