use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::{Parser, Subcommand, ValueEnum};

mod splash;
mod ui;

type AppResult<T> = Result<T, String>;

const T_ECHO_BASE: &str = "0x27000";
const T_ECHO_FAMILY: &str = "0xADA52840";
const T_ECHO_PROFILE: &str = "hopspot-t-echo";
const ESP32S3_TARGET: &str = "xtensa-esp32s3-none-elf";
const ESP32C6_TARGET: &str = "riscv32imac-unknown-none-elf";
const HELTEC_V4_PROFILE: &str = "full";
const HELTEC_V4_ARTIFACT: &str = "hopspot-heltec-v4.bin";
const T_BEAM_SUPREME_PROFILE: &str = "full,board-tbeam-supreme";
const T_BEAM_SUPREME_ARTIFACT: &str = "hopspot-t-beam-supreme.bin";
const XIAO_ESP32_C6_PROFILE: &str = "hopspot-c6";
const XIAO_ESP32_C6_ARTIFACT: &str = "hopspot-xiao-esp32-c6.bin";
const ESP_PARTITIONS_8MB: &str = "partitions-hopspot-8mb.csv";
const ESP_PARTITIONS_4MB: &str = "partitions-hopspot-4mb.csv";
const HOPSPOT_CONFIG_OFFSET: u32 = 0xD000;
const HOPSPOT_CONFIG_SIZE: usize = 0x1000;
const HOPSPOT_CONFIG_MAGIC: &[u8; 8] = b"HSPCFG1\0";
const HOPSPOT_CONFIG_VERSION: u8 = 1;
const HOPSPOT_CONFIG_SSID_MAX: usize = 32;
const HOPSPOT_CONFIG_PASSWORD_MAX: usize = 64;

#[derive(Parser)]
#[command(
    name = "hopspot-flash",
    about = "Interactive firmware flasher for Personal Hopspot boards.",
    long_about = "Run without a subcommand for a guided board flashing flow."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CommandMode>,
}

#[derive(Subcommand)]
enum CommandMode {
    /// List boards known to the helper.
    List,
    /// Build a hosted docs firmware artifact.
    #[command(hide = true)]
    Build {
        #[arg(value_enum)]
        board: BoardId,
        #[arg(long, value_name = "DIR")]
        out_root: Option<PathBuf>,
    },
    /// Flash Hopspot firmware to the board.
    Flash {
        #[arg(value_enum)]
        board: BoardId,
        #[arg(long, value_name = "PORT", help = "Serial port for ESP boards")]
        port: Option<String>,
        #[arg(
            long,
            value_name = "SSID",
            help = "Explicit Wi-Fi SSID to write into Hopspot config"
        )]
        wifi_ssid: Option<String>,
        #[arg(
            long,
            value_name = "PASSWORD",
            help = "Explicit Wi-Fi password to write into Hopspot config"
        )]
        wifi_password: Option<String>,
        #[arg(
            long,
            help = "Load Wi-Fi Auto credentials from HOPSPOT_WIFI_* or .wifi-env"
        )]
        wifi_from_env: bool,
        #[arg(
            long,
            help = "Explicitly clear/omit Wi-Fi Auto credentials for this flash"
        )]
        no_wifi_creds: bool,
        #[arg(long, help = "Open espflash monitor after flashing ESP boards")]
        monitor: bool,
        #[arg(long, value_name = "DIR", help = "Mounted TECHOBOOT directory")]
        mount: Option<PathBuf>,
    },
    /// Show board-specific flashing steps.
    Steps {
        #[arg(value_enum)]
        board: BoardId,
    },
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum BoardId {
    HeltecV4,
    TBeamSupreme,
    XiaoEsp32C6,
    TEcho,
}

impl BoardId {
    fn target(self) -> &'static BoardTarget {
        match self {
            BoardId::TEcho => &BOARDS[0],
            BoardId::HeltecV4 => &BOARDS[1],
            BoardId::TBeamSupreme => &BOARDS[2],
            BoardId::XiaoEsp32C6 => &BOARDS[3],
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum BoardBackend {
    TEchoUf2,
    EspFlash(&'static EspImageSpec),
}

impl BoardBackend {
    fn ready(self) -> bool {
        true
    }
}

struct BoardTarget {
    slug: &'static str,
    name: &'static str,
    silicon: &'static str,
    interfaces: &'static [&'static str],
    backend: BoardBackend,
}

impl BoardTarget {
    fn supports_wifi_config(&self) -> bool {
        self.interfaces
            .iter()
            .any(|interface| *interface == "Wi-Fi Auto")
    }
}

#[derive(Clone, Copy, PartialEq)]
struct EspImageSpec {
    chip: &'static str,
    chip_family: &'static str,
    flash_size: &'static str,
    target: &'static str,
    partition_table: &'static str,
    profile: &'static str,
    artifact: &'static str,
    web_name: &'static str,
    no_default_features: bool,
    wifi_configurable: bool,
    after_reset: &'static str,
}

const HELTEC_V4_ESP: EspImageSpec = EspImageSpec {
    chip: "esp32s3",
    chip_family: "ESP32-S3",
    flash_size: "8mb",
    target: ESP32S3_TARGET,
    partition_table: ESP_PARTITIONS_8MB,
    profile: HELTEC_V4_PROFILE,
    artifact: HELTEC_V4_ARTIFACT,
    web_name: "Hopspot Heltec V4",
    no_default_features: false,
    wifi_configurable: true,
    after_reset: "watchdog-reset",
};

const T_BEAM_SUPREME_ESP: EspImageSpec = EspImageSpec {
    chip: "esp32s3",
    chip_family: "ESP32-S3",
    flash_size: "8mb",
    target: ESP32S3_TARGET,
    partition_table: ESP_PARTITIONS_8MB,
    profile: T_BEAM_SUPREME_PROFILE,
    artifact: T_BEAM_SUPREME_ARTIFACT,
    web_name: "Hopspot T-Beam Supreme",
    no_default_features: false,
    wifi_configurable: true,
    after_reset: "watchdog-reset",
};

const XIAO_ESP32_C6_ESP: EspImageSpec = EspImageSpec {
    chip: "esp32c6",
    chip_family: "ESP32-C6",
    flash_size: "4mb",
    target: ESP32C6_TARGET,
    partition_table: ESP_PARTITIONS_4MB,
    profile: XIAO_ESP32_C6_PROFILE,
    artifact: XIAO_ESP32_C6_ARTIFACT,
    web_name: "Hopspot XIAO ESP32-C6",
    no_default_features: true,
    wifi_configurable: false,
    after_reset: "hard-reset",
};

const T_ECHO: BoardTarget = BoardTarget {
    slug: "t-echo",
    name: "LilyGO T-Echo",
    silicon: "nRF52840 + SX1262",
    interfaces: &["BLE Auto", "LoRa", "USB Auto"],
    backend: BoardBackend::TEchoUf2,
};

const BOARDS: &[BoardTarget] = &[
    T_ECHO,
    BoardTarget {
        slug: "heltec-v4",
        name: "Heltec V4",
        silicon: "ESP32-S3 + SX1262",
        interfaces: &["Wi-Fi Auto", "BLE Auto", "LoRa", "ESP-NOW", "USB Auto"],
        backend: BoardBackend::EspFlash(&HELTEC_V4_ESP),
    },
    BoardTarget {
        slug: "t-beam-supreme",
        name: "LilyGO T-Beam Supreme",
        silicon: "ESP32-S3 + SX1262",
        interfaces: &["Wi-Fi Auto", "BLE Auto", "LoRa", "ESP-NOW", "USB Auto"],
        backend: BoardBackend::EspFlash(&T_BEAM_SUPREME_ESP),
    },
    BoardTarget {
        slug: "xiao-esp32-c6",
        name: "Seeed Studio XIAO ESP32-C6",
        silicon: "ESP32-C6 + SX1262",
        interfaces: &["ESP-NOW", "BLE Auto", "USB Auto"],
        backend: BoardBackend::EspFlash(&XIAO_ESP32_C6_ESP),
    },
];

struct BuildOutput {
    artifact: PathBuf,
    metadata: PathBuf,
    web_manifest: Option<PathBuf>,
    profile: &'static str,
    sha256: String,
    size: u64,
}

struct EspFirmware {
    elf: PathBuf,
    partition_table: PathBuf,
}

#[derive(Clone, Debug)]
struct WifiFlashConfig {
    ssid: String,
    password: String,
}

impl WifiFlashConfig {
    fn validate(&self) -> AppResult<()> {
        let ssid_len = self.ssid.as_bytes().len();
        let password_len = self.password.as_bytes().len();
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

struct EspToolchainEnv {
    path: OsString,
    libclang_path: Option<OsString>,
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::ExitCode::from(1)
        }
    }
}

fn run() -> AppResult<()> {
    let cli = Cli::parse();
    let repo = repo_root()?;

    match cli.command {
        Some(CommandMode::List) => list_boards(),
        Some(CommandMode::Build { board, out_root }) => {
            let out_root = out_root.unwrap_or_else(|| default_artifact_root(&repo));
            let output = build_board(board.target(), &repo, &out_root)?;
            print_build_output(&output);
            Ok(())
        }
        Some(CommandMode::Flash {
            board,
            port,
            wifi_ssid,
            wifi_password,
            wifi_from_env,
            no_wifi_creds,
            monitor,
            mount,
        }) => {
            let target = board.target();
            let wifi_config = wifi_config_from_args(
                target,
                &repo,
                wifi_ssid,
                wifi_password,
                wifi_from_env,
                no_wifi_creds,
            )?;
            flash_board(
                target,
                &repo,
                port.as_deref(),
                monitor,
                mount.as_deref(),
                wifi_config.as_ref(),
            )
        }
        Some(CommandMode::Steps { board }) => {
            print_steps(board.target());
            Ok(())
        }
        None => interactive(&repo),
    }
}

fn interactive(repo: &Path) -> AppResult<()> {
    ui::print_header();

    let Some(board_index) = ui::select(
        "Which board are you flashing?",
        &BOARDS
            .iter()
            .map(|board| {
                let ready = board.backend.ready();
                let state = match board.backend {
                    BoardBackend::TEchoUf2 => "ready",
                    BoardBackend::EspFlash(_) => "ready",
                };
                format!(
                    "{}  {}",
                    board.name,
                    ui::status_chip(&format!("[{state}]"), ready)
                )
            })
            .collect::<Vec<_>>(),
        0,
    )?
    else {
        return Ok(());
    };
    let board = &BOARDS[board_index];

    println!();
    print_board_summary(board);
    println!();

    if !board.backend.ready() {
        println!(
            "{} is in the flasher catalog, but this local CLI backend is not wired yet.",
            board.name
        );
        println!("For now, use a board-specific workflow until this backend lands.");
        return Ok(());
    }

    loop {
        let action = ui::select(
            "What do you want to do?",
            &[
                "Flash Hopspot firmware".to_string(),
                "Show flashing steps".to_string(),
                "Exit".to_string(),
            ],
            0,
        )?
        .unwrap_or(3);

        match action {
            0 => {
                let wifi_config = prompt_wifi_config(board, repo)?;
                return flash_board(board, repo, None, false, None, wifi_config.as_ref());
            }
            1 => {
                println!();
                print_steps(board);
                println!();
                ui::pause("Press Enter to return to the flasher menu... ")?;
                println!();
            }
            _ => return Ok(()),
        }
    }
}

fn list_boards() -> AppResult<()> {
    for board in BOARDS {
        let state = match board.backend {
            BoardBackend::TEchoUf2 => "ready",
            BoardBackend::EspFlash(_) => "ready",
        };
        println!("{:<18} {:<8} {}", board.slug, state, board.name);
    }
    Ok(())
}

fn print_board_summary(board: &BoardTarget) {
    ui::print_section(board.name);
    ui::print_key_value("silicon", board.silicon);
    ui::print_key_value("interfaces", &board.interfaces.join(", "));
    ui::print_key_value(
        "status",
        match board.backend {
            BoardBackend::TEchoUf2 => "ready",
            BoardBackend::EspFlash(_) => "ready",
        },
    );
}

fn print_steps(board: &BoardTarget) {
    print_board_summary(board);
    println!();

    match board.backend {
        BoardBackend::TEchoUf2 => {
            println!("1. Connect a USB-C data cable to your T-Echo and plug it into this device.");
            println!("2. Double-tap RESET so the TECHOBOOT drive mounts.");
            println!("3. Run `cargo run -p hopspot-flash -- flash t-echo`.");
            println!("4. The CLI copies the UF2 file to TECHOBOOT.");
            println!("5. The T-Echo reboots into the new firmware after the copy completes.");
        }
        BoardBackend::EspFlash(_) => {
            println!(
                "1. Connect a USB-C data cable to your {} and plug it into this device.",
                board.name
            );
            println!(
                "2. Run `cargo run -p hopspot-flash -- flash {}`.",
                board.slug
            );
            println!("3. Choose the port labeled USB JTAG/serial debug if prompted.");
            println!("4. If your device is not detected, hold BOOT, tap RESET, then release BOOT.");
            println!("5. Wait for flash verification, then reset once.");
        }
    }
}

fn wifi_config_from_args(
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

fn prompt_wifi_config(board: &BoardTarget, repo: &Path) -> AppResult<Option<WifiFlashConfig>> {
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

fn flash_board(
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

fn write_hopspot_config_image(
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

fn hopspot_config_image_bytes(wifi_config: Option<&WifiFlashConfig>) -> Vec<u8> {
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

fn build_board(board: &BoardTarget, repo: &Path, out_root: &Path) -> AppResult<BuildOutput> {
    ensure_supported(board)?;
    match board.backend {
        BoardBackend::TEchoUf2 => build_t_echo(repo, out_root),
        BoardBackend::EspFlash(spec) => build_esp_board(board, spec, repo, out_root),
    }
}

fn ensure_supported(board: &BoardTarget) -> AppResult<()> {
    let _ = board;
    Ok(())
}

fn build_t_echo(repo: &Path, out_root: &Path) -> AppResult<BuildOutput> {
    ui::print_section("Building LilyGO T-Echo");
    let crate_dir = repo.join("personal-hopspot").join("t-echo");
    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("build")
        .arg("--release")
        .arg("--no-default-features")
        .arg("--features")
        .arg(T_ECHO_PROFILE)
        .current_dir(&crate_dir);
    run_status(
        &mut cargo,
        "cargo build --release --no-default-features --features hopspot-t-echo",
    )?;

    let host_triple = rust_host_triple()?;
    let sysroot = capture_stdout(Command::new("rustc").arg("--print").arg("sysroot"), "rustc")?;
    let objcopy = Path::new(sysroot.trim())
        .join("lib")
        .join("rustlib")
        .join(host_triple.trim())
        .join("bin")
        .join("llvm-objcopy");

    let work_dir = repo
        .join("target")
        .join("flash-artifacts")
        .join("work")
        .join("t-echo");
    let board_out = out_root
        .join("firmware")
        .join("hopspot")
        .join("t-echo")
        .join("latest");
    fs::create_dir_all(&work_dir)
        .map_err(|err| format!("failed to create {}: {err}", work_dir.display()))?;
    fs::create_dir_all(&board_out)
        .map_err(|err| format!("failed to create {}: {err}", board_out.display()))?;

    let elf = crate_dir
        .join("target")
        .join("thumbv7em-none-eabihf")
        .join("release")
        .join("t-echo");
    let bin = work_dir.join("t-echo.bin");
    let uf2 = board_out.join("t-echo.uf2");
    let metadata = board_out.join("t-echo.uf2.json");

    run_status(
        Command::new(&objcopy)
            .arg("-O")
            .arg("binary")
            .arg(&elf)
            .arg(&bin),
        "llvm-objcopy",
    )?;
    run_status(
        Command::new("python3")
            .arg(repo.join("scripts").join("bin2uf2.py"))
            .arg(&bin)
            .arg(&uf2)
            .arg(T_ECHO_BASE)
            .arg(T_ECHO_FAMILY),
        "bin2uf2.py",
    )?;

    let sha256 = sha256_file(&uf2)?;
    let size = fs::metadata(&uf2)
        .map_err(|err| format!("failed to inspect {}: {err}", uf2.display()))?
        .len();
    write_metadata(&metadata, &sha256, size, T_ECHO_PROFILE)?;

    Ok(BuildOutput {
        artifact: uf2,
        metadata,
        web_manifest: None,
        profile: T_ECHO_PROFILE,
        sha256,
        size,
    })
}

fn build_esp_board(
    board: &BoardTarget,
    spec: &EspImageSpec,
    repo: &Path,
    out_root: &Path,
) -> AppResult<BuildOutput> {
    let firmware = build_esp_firmware(board, spec, repo)?;

    let board_out = out_root
        .join("firmware")
        .join("hopspot")
        .join(board.slug)
        .join("latest");
    fs::create_dir_all(&board_out)
        .map_err(|err| format!("failed to create {}: {err}", board_out.display()))?;

    let artifact = board_out.join(spec.artifact);
    let metadata = board_out.join(format!("{}.json", spec.artifact));
    let web_manifest = board_out.join("manifest.json");
    if spec.wifi_configurable {
        fs::write(
            board_out.join("hopspot-config-empty.bin"),
            hopspot_config_image_bytes(None),
        )
        .map_err(|err| {
            format!(
                "failed to write {}: {err}",
                board_out.join("hopspot-config-empty.bin").display()
            )
        })?;
    }

    run_status(
        Command::new("espflash")
            .arg("save-image")
            .arg("--chip")
            .arg(spec.chip)
            .arg("--flash-size")
            .arg(spec.flash_size)
            .arg("--partition-table")
            .arg(&firmware.partition_table)
            .arg("--target-app-partition")
            .arg("factory")
            .arg("--merge")
            .arg("--skip-padding")
            .arg(&firmware.elf)
            .arg(&artifact),
        "espflash save-image",
    )?;

    let sha256 = sha256_file(&artifact)?;
    let size = fs::metadata(&artifact)
        .map_err(|err| format!("failed to inspect {}: {err}", artifact.display()))?
        .len();
    write_esp_metadata(&metadata, board.slug, spec, &sha256, size)?;
    write_esp_web_manifest(&web_manifest, spec)?;

    Ok(BuildOutput {
        artifact,
        metadata,
        web_manifest: Some(web_manifest),
        profile: spec.profile,
        sha256,
        size,
    })
}

fn build_esp_firmware(
    board: &BoardTarget,
    spec: &EspImageSpec,
    repo: &Path,
) -> AppResult<EspFirmware> {
    prepare_embedded_site_bundle(spec, repo)?;

    let section = format!("Building {}", board.name);
    ui::print_section(&section);
    let crate_dir = repo.join("personal-hopspot").join("app");
    let elf = crate_dir
        .join("target")
        .join(spec.target)
        .join("release")
        .join("personal-hopspot-app");
    let partition_table = crate_dir.join(spec.partition_table);

    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg("personal-hopspot-app")
        .arg("--target")
        .arg(spec.target)
        .arg("-Zbuild-std=core,alloc");
    if spec.no_default_features {
        cargo.arg("--no-default-features");
    }
    cargo
        .arg("--features")
        .arg(spec.profile)
        .current_dir(&crate_dir);
    if spec.target == ESP32S3_TARGET {
        let linker = configure_esp_toolchain(&mut cargo)?;
        ui::print_key_value("xtensa gcc", &linker.display().to_string());
    }
    let build_label = format!(
        "cargo build --release --bin personal-hopspot-app --target {} -Zbuild-std=core,alloc {}--features {}",
        spec.target,
        if spec.no_default_features {
            "--no-default-features "
        } else {
            ""
        },
        spec.profile
    );
    run_status(&mut cargo, &build_label)?;

    Ok(EspFirmware {
        elf,
        partition_table,
    })
}

fn prepare_embedded_site_bundle(spec: &EspImageSpec, repo: &Path) -> AppResult<()> {
    if spec.target != ESP32S3_TARGET {
        return Ok(());
    }

    let site_dir = repo.join("docs").join("website");
    let output_dir = site_dir
        .join("target")
        .join("dx")
        .join("reticulum-site")
        .join("release")
        .join("web")
        .join("public");

    ui::print_section("Building embedded docs bundle");
    ui::print_key_value("mode", "Hopspot SoftAP");
    ui::print_key_value("output", &output_dir.display().to_string());

    let mut dx = Command::new("dx");
    dx.env("PRNS_EMBEDDED_SITE", "1")
        .env_remove("PRNS_FLASH_ARTIFACT_ROOT")
        .arg("build")
        .arg("--platform")
        .arg("web")
        .arg("--debug-symbols")
        .arg("false")
        .arg("--release")
        .current_dir(&site_dir);
    run_status(
        &mut dx,
        "dx build --platform web --debug-symbols false --release",
    )?;

    if !output_dir.join("index.html").is_file() {
        return Err(format!(
            "embedded docs bundle did not produce {}",
            output_dir.join("index.html").display()
        ));
    }
    if !output_dir.join("source.zip").is_file() {
        return Err(format!(
            "embedded docs bundle did not include {}",
            output_dir.join("source.zip").display()
        ));
    }

    Ok(())
}

fn print_build_output(output: &BuildOutput) {
    println!();
    ui::print_section("Artifact ready");
    ui::print_key_value("artifact", &output.artifact.display().to_string());
    ui::print_key_value("metadata", &output.metadata.display().to_string());
    if let Some(web_manifest) = &output.web_manifest {
        ui::print_key_value("web manifest", &web_manifest.display().to_string());
    }
    ui::print_key_value("profile", output.profile);
    ui::print_key_value("sha256", &output.sha256);
    ui::print_key_value("size", &format!("{} bytes", output.size));
}

fn write_metadata(path: &Path, sha256: &str, size: u64, profile: &str) -> AppResult<()> {
    let json = format!(
        concat!(
            "{{\n",
            "  \"board_slug\": \"t-echo\",\n",
            "  \"profile\": \"{profile}\",\n",
            "  \"format\": \"uf2\",\n",
            "  \"transport\": \"uf2-mass-storage\",\n",
            "  \"artifact\": \"t-echo.uf2\",\n",
            "  \"artifact_sha256\": \"{sha256}\",\n",
            "  \"artifact_size\": {size},\n",
            "  \"flash_base\": \"{base}\",\n",
            "  \"family\": \"{family}\",\n",
            "  \"source\": \"personal-hopspot/t-echo\"\n",
            "}}\n"
        ),
        profile = profile,
        sha256 = sha256,
        size = size,
        base = T_ECHO_BASE,
        family = T_ECHO_FAMILY,
    );
    fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn write_esp_metadata(
    path: &Path,
    board_slug: &str,
    spec: &EspImageSpec,
    sha256: &str,
    size: u64,
) -> AppResult<()> {
    let json = format!(
        concat!(
            "{{\n",
            "  \"board_slug\": \"{board_slug}\",\n",
            "  \"profile\": \"{profile}\",\n",
            "  \"format\": \"esp-bin\",\n",
            "  \"transport\": \"esp-web-serial\",\n",
            "  \"artifact\": \"{artifact}\",\n",
            "  \"artifact_sha256\": \"{sha256}\",\n",
            "  \"artifact_size\": {size},\n",
            "  \"chip\": \"{chip}\",\n",
            "  \"flash_size\": \"{flash_size}\",\n",
            "  \"partition_table\": \"personal-hopspot/app/{partition_table}\",\n",
            "  \"config_offset\": {config_offset},\n",
            "  \"config_artifact\": {config_artifact},\n",
            "  \"source\": \"personal-hopspot/app\"\n",
            "}}\n"
        ),
        board_slug = board_slug,
        profile = spec.profile,
        artifact = spec.artifact,
        sha256 = sha256,
        size = size,
        chip = spec.chip,
        flash_size = spec.flash_size,
        partition_table = spec.partition_table,
        config_offset = if spec.wifi_configurable {
            format!("\"0x{HOPSPOT_CONFIG_OFFSET:x}\"")
        } else {
            "null".to_string()
        },
        config_artifact = if spec.wifi_configurable {
            "\"hopspot-config-empty.bin\"".to_string()
        } else {
            "null".to_string()
        },
    );
    fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn write_esp_web_manifest(path: &Path, spec: &EspImageSpec) -> AppResult<()> {
    let config_part = if spec.wifi_configurable {
        format!(
            ",\n        {{ \"path\": \"hopspot-config-empty.bin\", \"offset\": {} }}",
            HOPSPOT_CONFIG_OFFSET
        )
    } else {
        String::new()
    };
    let json = format!(
        concat!(
            "{{\n",
            "  \"name\": \"{name}\",\n",
            "  \"version\": \"preview\",\n",
            "  \"new_install_prompt_erase\": true,\n",
            "  \"new_install_improv_wait_time\": 0,\n",
            "  \"builds\": [\n",
            "    {{\n",
            "      \"chipFamily\": \"{chip_family}\",\n",
            "      \"improv\": false,\n",
            "      \"parts\": [\n",
            "        {{ \"path\": \"{artifact}\", \"offset\": 0 }}{config_part}\n",
            "      ]\n",
            "    }}\n",
            "  ]\n",
            "}}\n"
        ),
        name = spec.web_name,
        chip_family = spec.chip_family,
        artifact = spec.artifact,
        config_part = config_part,
    );
    fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn sha256_file(path: &Path) -> AppResult<String> {
    if let Ok(hash) = capture_stdout(
        Command::new("shasum").arg("-a").arg("256").arg(path),
        "shasum",
    ) {
        return first_word(&hash).ok_or_else(|| "shasum produced no hash".to_string());
    }
    if let Ok(hash) = capture_stdout(Command::new("sha256sum").arg(path), "sha256sum") {
        return first_word(&hash).ok_or_else(|| "sha256sum produced no hash".to_string());
    }
    Err("missing shasum or sha256sum".to_string())
}

fn first_word(value: &str) -> Option<String> {
    value.split_whitespace().next().map(str::to_string)
}

fn configure_esp_toolchain(command: &mut Command) -> AppResult<PathBuf> {
    let env = esp_toolchain_env()?;
    let linker = find_on_path("xtensa-esp32s3-elf-gcc", &env.path).ok_or_else(|| {
        "xtensa-esp32s3-elf-gcc was not found. Install the ESP Rust toolchain or update ~/export-esp.sh so it exports the Xtensa GCC bin directory.".to_string()
    })?;

    command.env("PATH", &env.path);
    if let Some(libclang_path) = env.libclang_path {
        command.env("LIBCLANG_PATH", libclang_path);
    }
    Ok(linker)
}

fn esp_toolchain_env() -> AppResult<EspToolchainEnv> {
    let mut path_entries = Vec::new();
    let mut libclang_path = env::var_os("LIBCLANG_PATH");

    if let Some(home) = home_dir() {
        let export_path = home.join("export-esp.sh");
        if let Ok(contents) = fs::read_to_string(&export_path) {
            for line in contents.lines() {
                if let Some(value) = parse_export_value(line, "PATH") {
                    for part in value.split(':') {
                        if part == "$PATH" || part == "${PATH}" || part.is_empty() {
                            continue;
                        }
                        path_entries.push(expand_export_path(part, &home));
                    }
                } else if let Some(value) = parse_export_value(line, "LIBCLANG_PATH") {
                    libclang_path = Some(expand_export_path(&value, &home).into_os_string());
                }
            }
        }

        collect_xtensa_toolchain_bins(
            &home
                .join(".rustup")
                .join("toolchains")
                .join("esp")
                .join("xtensa-esp-elf"),
            &mut path_entries,
        );
    }

    if let Some(current_path) = env::var_os("PATH") {
        path_entries.extend(env::split_paths(&current_path));
    }

    let mut seen = HashSet::new();
    path_entries.retain(|path| seen.insert(path.to_string_lossy().into_owned()));
    let path = env::join_paths(path_entries)
        .map_err(|err| format!("failed to build ESP toolchain PATH: {err}"))?;

    Ok(EspToolchainEnv {
        path,
        libclang_path,
    })
}

fn parse_export_value(line: &str, key: &str) -> Option<String> {
    parse_assignment_value(line, key)
}

fn parse_assignment_value(line: &str, key: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let (name, value) = line.split_once('=')?;
    if name.trim() != key {
        return None;
    }
    Some(unquote_assignment_value(value.trim()))
}

fn unquote_assignment_value(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn expand_export_path(value: &str, home: &Path) -> PathBuf {
    let home_string = home.to_string_lossy();
    let expanded = value
        .replace("${HOME}", &home_string)
        .replace("$HOME", &home_string);
    if let Some(rest) = expanded.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(expanded)
    }
}

fn collect_xtensa_toolchain_bins(root: &Path, path_entries: &mut Vec<PathBuf>) {
    let Ok(releases) = fs::read_dir(root) else {
        return;
    };

    for release in releases.flatten() {
        let bin = release.path().join("xtensa-esp-elf").join("bin");
        if bin.is_dir() {
            path_entries.push(bin);
        }
    }
}

fn find_on_path(binary: &str, path: &OsString) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn rust_host_triple() -> AppResult<String> {
    let version = capture_stdout(Command::new("rustc").arg("-vV"), "rustc -vV")?;
    version
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        .ok_or_else(|| "rustc -vV did not report a host triple".to_string())
}

fn run_status(command: &mut Command, label: &str) -> AppResult<()> {
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    let status = command
        .status()
        .map_err(|err| format!("failed to run {label}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} exited with {status}"))
    }
}

fn capture_stdout(command: &mut Command, label: &str) -> AppResult<String> {
    let output = command
        .output()
        .map_err(|err| format!("failed to run {label}: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{label} exited with {}: {stderr}", output.status));
    }
    String::from_utf8(output.stdout)
        .map_err(|err| format!("{label} produced invalid UTF-8 output: {err}"))
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

fn default_artifact_root(repo: &Path) -> PathBuf {
    repo.join("target").join("flash-artifacts")
}

fn repo_root() -> AppResult<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("cannot determine repo root from {}", manifest_dir.display()))
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

    #[test]
    fn esp32s3_config_write_uses_usb_reset() {
        assert_eq!(esp_before_reset(&T_BEAM_SUPREME_ESP), "usb-reset");
        assert_eq!(esp_before_reset(&HELTEC_V4_ESP), "usb-reset");
        assert_eq!(esp_before_reset(&XIAO_ESP32_C6_ESP), "default-reset");
    }
}
