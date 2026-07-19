use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::boards::BoardTarget;
use crate::toolchain::parse_assignment_value;
use crate::{ui, AppResult};

const HOPSPOT_CONFIG_SIZE: usize = 0x1000;
pub(crate) const HOPSPOT_CONFIG_OFFSET: u32 = 0xD000;
const HOPSPOT_CONFIG_MAGIC: &[u8; 8] = b"HSPCFG1\0";
const HOPSPOT_CONFIG_VERSION: u8 = 1;
const HOPSPOT_CONFIG_SSID_MAX: usize = 32;
const HOPSPOT_CONFIG_PASSWORD_MAX: usize = 64;

#[derive(Clone, Debug)]
pub(crate) struct WifiFlashConfig {
    pub(crate) ssid: String,
    pub(crate) password: String,
}

impl WifiFlashConfig {
    fn validate(&self) -> AppResult<()> {
        let ssid_len = self.ssid.len();
        let password_len = self.password.len();
        if ssid_len == 0 {
            return Err("--wifi-ssid cannot be empty".to_string());
        }
        if ssid_len > HOPSPOT_CONFIG_SSID_MAX {
            return Err(format!(
                "Wi-Fi SSID is {ssid_len} bytes; max is {HOPSPOT_CONFIG_SSID_MAX}"
            ));
        }
        if password_len > HOPSPOT_CONFIG_PASSWORD_MAX {
            return Err(format!(
                "Wi-Fi password is {password_len} bytes; max is {HOPSPOT_CONFIG_PASSWORD_MAX}"
            ));
        }
        Ok(())
    }
}

pub(crate) fn wifi_config_from_args(
    board: &BoardTarget,
    repo: &Path,
    ssid: Option<String>,
    password: Option<String>,
    from_env: bool,
    no_wifi_creds: bool,
) -> AppResult<Option<WifiFlashConfig>> {
    if !board.supports_wifi_config() {
        if ssid.is_some() || password.is_some() || from_env || no_wifi_creds {
            return Err(format!("{} does not have Wi-Fi Auto", board.name));
        }
        return Ok(None);
    }
    let explicit = ssid.is_some() || password.is_some();
    let selected_modes = usize::from(explicit) + usize::from(from_env) + usize::from(no_wifi_creds);
    if selected_modes > 1 {
        return Err(
            "choose only one Wi-Fi credential source: --wifi-ssid/--wifi-password, --wifi-from-env, or --no-wifi-creds"
                .to_string(),
        );
    }
    if from_env {
        return local_wifi_config(repo)?
            .map(Some)
            .ok_or_else(|| local_wifi_config_missing_message(repo));
    }
    if no_wifi_creds {
        return Ok(None);
    }
    match (ssid, password) {
        (Some(ssid), password) => {
            let config = WifiFlashConfig {
                ssid,
                password: password.unwrap_or_default(),
            };
            config.validate()?;
            Ok(Some(config))
        }
        (None, Some(_)) => Err("--wifi-password requires --wifi-ssid".to_string()),
        (None, None) => Ok(None),
    }
}

pub(crate) fn prompt_wifi_config(
    board: &BoardTarget,
    repo: &Path,
) -> AppResult<Option<WifiFlashConfig>> {
    if !board.supports_wifi_config() || !ui::interactive_terminal() {
        return Ok(None);
    }
    println!();
    ui::print_section("Wi-Fi Auto");
    let choice = ui::select(
        "Configure Wi-Fi Auto network credentials for this flash?",
        &[
            "Do not include credentials (clear config slot)".to_string(),
            "Use HOPSPOT_WIFI_* / .wifi-env if present".to_string(),
            "Enter SSID and password".to_string(),
        ],
        0,
    )?;
    match choice {
        Some(1) => {
            let config =
                local_wifi_config(repo)?.ok_or_else(|| local_wifi_config_missing_message(repo))?;
            ui::print_note("Using local Wi-Fi Auto credentials for this flash.");
            return Ok(Some(config));
        }
        Some(2) => {}
        _ => return Ok(None),
    }
    let ssid = ui::input("SSID")?;
    let password = ui::password("Password")?;
    let config = WifiFlashConfig { ssid, password };
    config.validate()?;
    Ok(Some(config))
}

fn local_wifi_config(repo: &Path) -> AppResult<Option<WifiFlashConfig>> {
    if let Some(config) = wifi_config_from_process_env()? {
        return Ok(Some(config));
    }
    wifi_config_from_env_file(&repo.join(".wifi-env"))
}

fn wifi_config_from_process_env() -> AppResult<Option<WifiFlashConfig>> {
    let Some(ssid) = env::var("HOPSPOT_WIFI_SSID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let password = env::var("HOPSPOT_WIFI_PASSWORD").unwrap_or_default();
    let config = WifiFlashConfig { ssid, password };
    config.validate()?;
    Ok(Some(config))
}

fn wifi_config_from_env_file(path: &Path) -> AppResult<Option<WifiFlashConfig>> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Ok(None);
    };
    let Some(ssid) = parse_env_file_value(&contents, "HOPSPOT_WIFI_SSID")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let password = parse_env_file_value(&contents, "HOPSPOT_WIFI_PASSWORD").unwrap_or_default();
    let config = WifiFlashConfig { ssid, password };
    config.validate()?;
    Ok(Some(config))
}

fn parse_env_file_value(contents: &str, key: &str) -> Option<String> {
    contents
        .lines()
        .find_map(|line| parse_assignment_value(line, key))
}

fn local_wifi_config_missing_message(repo: &Path) -> String {
    format!(
        "no local Wi-Fi credentials found; set HOPSPOT_WIFI_SSID/HOPSPOT_WIFI_PASSWORD or create {}",
        repo.join(".wifi-env").display()
    )
}

pub(crate) fn write_hopspot_config_image(
    repo: &Path,
    board: &BoardTarget,
    wifi_config: Option<&WifiFlashConfig>,
) -> AppResult<PathBuf> {
    let work_dir = repo
        .join("target")
        .join("flash-artifacts")
        .join("work")
        .join(board.slug);
    fs::create_dir_all(&work_dir)
        .map_err(|err| format!("failed to create {}: {err}", work_dir.display()))?;
    let path = work_dir.join("hopspot-config.bin");
    let bytes = hopspot_config_image_bytes(wifi_config);
    fs::write(&path, bytes).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(path)
}

pub(crate) fn hopspot_config_image_bytes(wifi_config: Option<&WifiFlashConfig>) -> Vec<u8> {
    let mut bytes = vec![0xff; HOPSPOT_CONFIG_SIZE];
    bytes[..HOPSPOT_CONFIG_MAGIC.len()].copy_from_slice(HOPSPOT_CONFIG_MAGIC);
    bytes[8] = HOPSPOT_CONFIG_VERSION;
    if let Some(config) = wifi_config {
        let ssid = config.ssid.as_bytes();
        let password = config.password.as_bytes();
        bytes[10] = ssid.len() as u8;
        bytes[11] = password.len() as u8;
        bytes[16..16 + ssid.len()].copy_from_slice(ssid);
        let password_start = 16 + HOPSPOT_CONFIG_SSID_MAX;
        bytes[password_start..password_start + password.len()].copy_from_slice(password);
    } else {
        bytes[10] = 0;
        bytes[11] = 0;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_parser_accepts_plain_and_exported_assignments() {
        let contents = r#"
# local-only credentials
HOPSPOT_WIFI_SSID="Lab Network"
export HOPSPOT_WIFI_PASSWORD='secret phrase'
"#;

        assert_eq!(
            parse_env_file_value(contents, "HOPSPOT_WIFI_SSID").as_deref(),
            Some("Lab Network")
        );
        assert_eq!(
            parse_env_file_value(contents, "HOPSPOT_WIFI_PASSWORD").as_deref(),
            Some("secret phrase")
        );
    }

    #[test]
    fn empty_config_image_clears_lengths() {
        let bytes = hopspot_config_image_bytes(None);

        assert_eq!(&bytes[..HOPSPOT_CONFIG_MAGIC.len()], HOPSPOT_CONFIG_MAGIC);
        assert_eq!(bytes[8], HOPSPOT_CONFIG_VERSION);
        assert_eq!(bytes[10], 0);
        assert_eq!(bytes[11], 0);
    }
}
