use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::boards::{BoardBackend, BoardTarget, EspImageSpec};
#[cfg(test)]
use crate::boards::{HELTEC_V4_ESP, T_BEAM_SUPREME_ESP, XIAO_ESP32_C6_ESP};
use crate::build::{
    build_esp_firmware, build_t_echo, default_artifact_root, ensure_supported, print_build_output,
};
use crate::toolchain::run_status;
use crate::wifi::{write_hopspot_config_image, WifiFlashConfig, HOPSPOT_CONFIG_OFFSET};
use crate::{ui, AppResult};

pub(crate) fn flash_board(
    board: &BoardTarget,
    repo: &Path,
    port: Option<&str>,
    monitor: bool,
    mount_override: Option<&Path>,
    wifi_config: Option<&WifiFlashConfig>,
) -> AppResult<()> {
    ensure_supported(board)?;
    match board.backend {
        BoardBackend::TEchoUf2 => flash_t_echo(repo, mount_override),
        BoardBackend::EspFlash(spec) => {
            flash_esp_board(board, spec, repo, port, monitor, wifi_config)
        }
    }
}

fn flash_t_echo(repo: &Path, mount_override: Option<&Path>) -> AppResult<()> {
    let output = build_t_echo(repo, &default_artifact_root(repo))?;
    print_build_output(&output);
    let mount = match mount_override {
        Some(path) => path.to_path_buf(),
        None => match detect_techo_mount() {
            Some(path) => path,
            None if ui::interactive_terminal() => {
                println!();
                ui::print_section("Prepare T-Echo");
                println!("1. Double-tap RESET so the TECHOBOOT drive mounts.");
                println!("2. Wait for the drive to appear.");
                ui::pause("Press Enter once TECHOBOOT is mounted... ")?;
                detect_techo_mount().ok_or_else(|| {
                    "TECHOBOOT is still not mounted. Check the USB cable and bootloader mode."
                        .to_string()
                })?
            }
            None => {
                return Err(
                    "TECHOBOOT is not mounted. Double-tap RESET on the T-Echo, then run again."
                        .to_string(),
                );
            }
        },
    };
    if !mount.is_dir() {
        return Err(format!("{} is not a directory", mount.display()));
    }

    let destination = mount.join("t-echo.uf2");
    println!();
    ui::print_section("Copying UF2");
    ui::print_key_value("from", &output.artifact.display().to_string());
    ui::print_key_value("to", &destination.display().to_string());
    fs::copy(&output.artifact, &destination).map_err(|err| {
        format!(
            "failed to copy {} to {}: {err}",
            output.artifact.display(),
            destination.display()
        )
    })?;
    let _ = Command::new("sync").status();

    ui::print_note("Copy complete. The T-Echo should reboot into the new firmware.");
    Ok(())
}

fn flash_esp_board(
    board: &BoardTarget,
    spec: &EspImageSpec,
    repo: &Path,
    port: Option<&str>,
    monitor: bool,
    wifi_config: Option<&WifiFlashConfig>,
) -> AppResult<()> {
    let firmware = build_esp_firmware(board, spec, repo)?;

    println!();
    let section = format!("Flashing {}", board.name);
    ui::print_section(&section);
    ui::print_key_value("image", &firmware.elf.display().to_string());
    ui::print_key_value(
        "partition table",
        &firmware.partition_table.display().to_string(),
    );
    if let Some(port) = port {
        ui::print_key_value("port", port);
    } else {
        ui::print_key_value("port", "auto-detect");
    }

    let mut espflash = Command::new("espflash");
    espflash
        .arg("flash")
        .arg("--chip")
        .arg(spec.chip)
        .arg("--flash-size")
        .arg(spec.flash_size)
        .arg("--partition-table")
        .arg(&firmware.partition_table)
        .arg("--target-app-partition")
        .arg("factory")
        .arg("--after")
        .arg(spec.after_reset)
        .arg("--skip-update-check");
    if !ui::interactive_terminal() {
        espflash.arg("--non-interactive");
    }
    if monitor {
        espflash.arg("--monitor");
    }
    if let Some(port) = port {
        espflash.arg("--port").arg(port);
    }
    espflash.arg(&firmware.elf);
    run_status(&mut espflash, "espflash flash")?;

    if board.supports_wifi_config() {
        let config_image = write_hopspot_config_image(repo, board, wifi_config)?;
        println!();
        ui::print_section("Writing Hopspot config");
        ui::print_key_value("offset", &format!("0x{HOPSPOT_CONFIG_OFFSET:x}"));
        ui::print_key_value(
            "wifi",
            wifi_config
                .map(|config| config.ssid.as_str())
                .filter(|ssid| !ssid.is_empty())
                .unwrap_or("not configured"),
        );
        let mut write_bin = Command::new("espflash");
        write_bin
            .arg("write-bin")
            .arg("--chip")
            .arg(spec.chip)
            .arg("--before")
            .arg(esp_before_reset(spec))
            .arg("--after")
            .arg(spec.after_reset)
            .arg("--skip-update-check");
        if !ui::interactive_terminal() {
            write_bin.arg("--non-interactive");
        }
        if let Some(port) = port {
            write_bin.arg("--port").arg(port);
        }
        write_bin
            .arg(format!("0x{HOPSPOT_CONFIG_OFFSET:x}"))
            .arg(&config_image);
        run_status(&mut write_bin, "espflash write-bin hopcfg")?;
    }

    ui::print_note("Flash complete. Reset once if needed.");
    Ok(())
}

fn esp_before_reset(spec: &EspImageSpec) -> &'static str {
    match spec.chip {
        "esp32s3" => "usb-reset",
        _ => "default-reset",
    }
}

fn detect_techo_mount() -> Option<PathBuf> {
    if let Some(path) = env::var_os("HOPSPOT_TECHOBOOT") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Some(path);
        }
    }

    let mut candidates = vec![
        PathBuf::from("/Volumes/TECHOBOOT"),
        PathBuf::from("/mnt/TECHOBOOT"),
    ];
    if let Some(user) = env::var_os("USER").and_then(|value| value.into_string().ok()) {
        candidates.push(PathBuf::from("/media").join(&user).join("TECHOBOOT"));
        candidates.push(PathBuf::from("/run/media").join(user).join("TECHOBOOT"));
    }
    candidates.into_iter().find(|path| path.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esp32s3_config_write_uses_usb_reset() {
        assert_eq!(esp_before_reset(&T_BEAM_SUPREME_ESP), "usb-reset");
        assert_eq!(esp_before_reset(&HELTEC_V4_ESP), "usb-reset");
        assert_eq!(esp_before_reset(&XIAO_ESP32_C6_ESP), "default-reset");
    }
}
