use std::env;
use std::io::{self, IsTerminal, Read};

use prns_flash_manifest::{ProvisioningAction, WifiCredentials};

use crate::cli::WifiMode;
use crate::error::AppError;
use crate::ui;

pub(crate) struct WifiOptions {
    pub(crate) mode: WifiMode,
    pub(crate) ssid: Option<String>,
    pub(crate) password_stdin: bool,
    pub(crate) from_env: bool,
    pub(crate) interactive: bool,
}

pub(crate) fn resolve(
    supports_provisioning: bool,
    options: WifiOptions,
) -> Result<ProvisioningAction, AppError> {
    if !supports_provisioning {
        if options.mode != WifiMode::Preserve
            || options.ssid.is_some()
            || options.password_stdin
            || options.from_env
        {
            return Err(AppError::usage(
                "this board does not support Wi-Fi provisioning",
            ));
        }
        return Ok(ProvisioningAction::Preserve);
    }

    match options.mode {
        WifiMode::Preserve => {
            reject_unused_inputs(&options)?;
            Ok(ProvisioningAction::Preserve)
        }
        WifiMode::Clear => {
            reject_unused_inputs(&options)?;
            Ok(ProvisioningAction::Clear)
        }
        WifiMode::Configure => {
            if options.from_env {
                if options.ssid.is_some() || options.password_stdin {
                    return Err(AppError::usage(
                        "--wifi-from-env cannot be combined with SSID/password input options",
                    ));
                }
                return credentials_from_env().map(ProvisioningAction::Configure);
            }

            let ssid = match options.ssid {
                Some(ssid) => ssid,
                None if options.interactive => ui::input("Wi-Fi SSID").map_err(AppError::usage)?,
                None => {
                    return Err(AppError::usage(
                        "--wifi configure requires --wifi-ssid outside guided mode",
                    ));
                }
            };
            let password = if options.password_stdin {
                read_password_stdin(options.interactive)?
            } else if options.interactive {
                ui::password("Wi-Fi password (empty for open network)").map_err(AppError::usage)?
            } else {
                return Err(AppError::usage(
                    "--wifi configure requires --wifi-password-stdin or --wifi-from-env outside guided mode",
                ));
            };
            let credentials = WifiCredentials { ssid, password };
            credentials
                .validate()
                .map_err(|error| AppError::usage(error.to_string()))?;
            Ok(ProvisioningAction::Configure(credentials))
        }
    }
}

fn reject_unused_inputs(options: &WifiOptions) -> Result<(), AppError> {
    if options.ssid.is_some() || options.password_stdin || options.from_env {
        return Err(AppError::usage(
            "Wi-Fi credential inputs require `--wifi configure`",
        ));
    }
    Ok(())
}

fn read_password_stdin(allow_masked_prompt: bool) -> Result<String, AppError> {
    if io::stdin().is_terminal() {
        return if allow_masked_prompt {
            ui::password("Wi-Fi password (empty for open network)").map_err(AppError::usage)
        } else {
            Err(AppError::usage(
                "--wifi-password-stdin requires piped standard input in noninteractive/JSON mode",
            ))
        };
    }
    let mut value = String::new();
    io::stdin()
        .read_to_string(&mut value)
        .map_err(|error| AppError::usage(format!("could not read password from stdin: {error}")))?;
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    Ok(value)
}

fn credentials_from_env() -> Result<WifiCredentials, AppError> {
    let ssid = env::var("HOPSPOT_WIFI_SSID")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::usage("HOPSPOT_WIFI_SSID is missing from the environment"))?;
    let password = env::var("HOPSPOT_WIFI_PASSWORD").unwrap_or_default();
    let credentials = WifiCredentials { ssid, password };
    credentials
        .validate()
        .map_err(|error| AppError::usage(error.to_string()))?;
    Ok(credentials)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserve_rejects_credential_flags() {
        let result = resolve(
            true,
            WifiOptions {
                mode: WifiMode::Preserve,
                ssid: Some("network".to_string()),
                password_stdin: false,
                from_env: false,
                interactive: false,
            },
        );
        assert!(matches!(result, Err(AppError::Usage(_))));
    }
}
