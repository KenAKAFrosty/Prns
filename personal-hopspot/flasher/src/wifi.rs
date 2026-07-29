use std::env;
use std::io::{self, IsTerminal, Read};

use prns_flash_manifest::{ProvisioningAction, TcpClientEndpoint, WifiCredentials};

use crate::cli::WifiMode;
use crate::error::AppError;
use crate::ui;

pub(crate) struct WifiOptions {
    pub(crate) mode: WifiMode,
    pub(crate) ssid: Option<String>,
    pub(crate) password_stdin: bool,
    pub(crate) from_env: bool,
    pub(crate) tcp_client: Option<String>,
    pub(crate) interactive: bool,
}

pub(crate) fn resolve(
    supports_provisioning: bool,
    supports_tcp_client: bool,
    options: WifiOptions,
) -> Result<ProvisioningAction, AppError> {
    if !supports_provisioning {
        if options.mode != WifiMode::Preserve
            || options.ssid.is_some()
            || options.password_stdin
            || options.from_env
            || options.tcp_client.is_some()
        {
            return Err(AppError::configuration(
                "this board does not support Wi-Fi provisioning",
            ));
        }
        return Ok(ProvisioningAction::Preserve);
    }
    if options.tcp_client.is_some() && !supports_tcp_client {
        return Err(AppError::configuration(
            "this board does not have room for TCP client provisioning",
        ));
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
            let credentials = if options.from_env {
                if options.ssid.is_some() || options.password_stdin {
                    return Err(AppError::configuration(
                        "--wifi-from-env cannot be combined with SSID/password input options",
                    ));
                }
                credentials_from_env()?
            } else {
                let ssid = match options.ssid {
                    Some(ssid) => ssid,
                    None if options.interactive => {
                        ui::input("Wi-Fi SSID").map_err(AppError::configuration)?
                    }
                    None => {
                        return Err(AppError::configuration(
                            "--wifi configure requires --wifi-ssid outside guided mode",
                        ));
                    }
                };
                let password = if options.password_stdin {
                    read_password_stdin(options.interactive)?
                } else if options.interactive {
                    ui::password("Wi-Fi password (empty for open network)")
                        .map_err(AppError::configuration)?
                } else {
                    return Err(AppError::configuration(
                        "--wifi configure requires --wifi-password-stdin or --wifi-from-env outside guided mode",
                    ));
                };
                let credentials = WifiCredentials { ssid, password };
                credentials
                    .validate()
                    .map_err(|error| AppError::configuration(error.to_string()))?;
                credentials
            };
            let mut tcp_input = options.tcp_client.or_else(|| {
                options
                    .from_env
                    .then(|| env::var("HOPSPOT_TCP_TARGET").ok())
                    .flatten()
                    .filter(|value| !value.is_empty())
            });
            if supports_tcp_client && options.interactive && tcp_input.is_none() {
                let choices = vec![
                    "No outbound TCP client".to_string(),
                    "Configure one outbound TCP client".to_string(),
                ];
                if ui::select("TCP client", &choices, 0).map_err(AppError::configuration)?
                    == Some(1)
                {
                    tcp_input = Some(
                        ui::input("TCP target (IPv4[:port], hostname, or URL)")
                            .map_err(AppError::configuration)?,
                    );
                }
            }
            if tcp_input.is_some() && !supports_tcp_client {
                return Err(AppError::configuration(
                    "this board does not have room for TCP client provisioning",
                ));
            }
            let tcp_client = tcp_input
                .as_deref()
                .map(|value| {
                    TcpClientEndpoint::parse(value)
                        .map_err(|error| AppError::configuration(error.to_string()))
                })
                .transpose()?;
            if let Some(tcp_client) = tcp_client {
                Ok(ProvisioningAction::ConfigureWithTcp {
                    wifi: credentials,
                    tcp_client,
                })
            } else {
                Ok(ProvisioningAction::Configure(credentials))
            }
        }
    }
}

fn reject_unused_inputs(options: &WifiOptions) -> Result<(), AppError> {
    if options.ssid.is_some()
        || options.password_stdin
        || options.from_env
        || options.tcp_client.is_some()
    {
        return Err(AppError::configuration(
            "Wi-Fi and TCP client inputs require `--wifi configure`",
        ));
    }
    Ok(())
}

fn read_password_stdin(allow_masked_prompt: bool) -> Result<String, AppError> {
    if io::stdin().is_terminal() {
        return if allow_masked_prompt {
            ui::password("Wi-Fi password (empty for open network)").map_err(AppError::configuration)
        } else {
            Err(AppError::configuration(
                "--wifi-password-stdin requires piped standard input in noninteractive/JSON mode",
            ))
        };
    }
    let mut value = String::new();
    io::stdin().read_to_string(&mut value).map_err(|error| {
        AppError::configuration(format!("could not read password from stdin: {error}"))
    })?;
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    Ok(value)
}

fn credentials_from_env() -> Result<WifiCredentials, AppError> {
    let ssid = env::var("HOPSPOT_WIFI_SSID")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::configuration("HOPSPOT_WIFI_SSID is missing from the environment")
        })?;
    let password = env::var("HOPSPOT_WIFI_PASSWORD").unwrap_or_default();
    let credentials = WifiCredentials { ssid, password };
    credentials
        .validate()
        .map_err(|error| AppError::configuration(error.to_string()))?;
    Ok(credentials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_flash_manifest::TcpClientHost;
    use std::net::Ipv4Addr;

    #[test]
    fn preserve_rejects_credential_flags() {
        let result = resolve(
            true,
            true,
            WifiOptions {
                mode: WifiMode::Preserve,
                ssid: Some("network".to_string()),
                password_stdin: false,
                from_env: false,
                tcp_client: None,
                interactive: false,
            },
        );
        assert!(matches!(result, Err(AppError::Usage(_))));
    }

    #[test]
    fn numeric_target_accepts_default_and_explicit_ports() -> Result<(), AppError> {
        assert_eq!(
            TcpClientEndpoint::parse("192.0.2.10")
                .map_err(|error| AppError::configuration(error.to_string()))?,
            TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::new(192, 0, 2, 10)),
                port: 4242,
            }
        );
        assert_eq!(
            TcpClientEndpoint::parse("tcp://192.0.2.10:5252/path")
                .map_err(|error| AppError::configuration(error.to_string()))?,
            TcpClientEndpoint {
                host: TcpClientHost::Ipv4(Ipv4Addr::new(192, 0, 2, 10)),
                port: 5252,
            }
        );
        Ok(())
    }

    #[test]
    fn hostname_and_url_are_canonicalized_for_device_resolution() -> Result<(), AppError> {
        assert_eq!(
            TcpClientEndpoint::parse("https://Node.Example.:5252/path")
                .map_err(|error| AppError::configuration(error.to_string()))?,
            TcpClientEndpoint {
                host: TcpClientHost::Hostname("node.example".to_string()),
                port: 5252,
            }
        );
        Ok(())
    }

    #[test]
    fn tcp_target_is_rejected_on_non_capable_board() {
        let result = resolve(
            true,
            false,
            WifiOptions {
                mode: WifiMode::Configure,
                ssid: Some("network".to_string()),
                password_stdin: false,
                from_env: false,
                tcp_client: Some("192.0.2.10:4242".to_string()),
                interactive: false,
            },
        );
        assert!(matches!(result, Err(AppError::Usage(_))));
    }
}
