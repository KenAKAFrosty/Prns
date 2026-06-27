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
const HELTEC_V4_PROFILE: &str = "full";
const HELTEC_V4_ARTIFACT: &str = "hopspot-heltec-v4.bin";
const HELTEC_V4_PARTITIONS: &str = "partitions-hopspot-8mb.csv";

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
    HeltecV4EspFlash,
    Planned,
}

impl BoardBackend {
    fn ready(self) -> bool {
        !matches!(self, BoardBackend::Planned)
    }
}

struct BoardTarget {
    slug: &'static str,
    name: &'static str,
    silicon: &'static str,
    interfaces: &'static [&'static str],
    backend: BoardBackend,
}

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
        backend: BoardBackend::HeltecV4EspFlash,
    },
    BoardTarget {
        slug: "t-beam-supreme",
        name: "LilyGO T-Beam Supreme",
        silicon: "ESP32-S3 + SX1262",
        interfaces: &["Wi-Fi Auto", "BLE Auto", "LoRa", "ESP-NOW", "USB Auto"],
        backend: BoardBackend::Planned,
    },
    BoardTarget {
        slug: "xiao-esp32-c6",
        name: "Seeed Studio XIAO ESP32-C6",
        silicon: "ESP32-C6 + SX1262",
        interfaces: &["ESP-NOW", "BLE Auto", "USB Auto"],
        backend: BoardBackend::Planned,
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
            monitor,
            mount,
        }) => flash_board(
            board.target(),
            &repo,
            port.as_deref(),
            monitor,
            mount.as_deref(),
        ),
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
                    BoardBackend::HeltecV4EspFlash => "ready",
                    BoardBackend::Planned => "coming soon",
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
            0 => return flash_board(board, repo, None, false, None),
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
            BoardBackend::HeltecV4EspFlash => "ready",
            BoardBackend::Planned => "planned",
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
            BoardBackend::HeltecV4EspFlash => "ready",
            BoardBackend::Planned => "coming soon",
        },
    );
}

fn print_steps(board: &BoardTarget) {
    print_board_summary(board);
    println!();

    match board.backend {
        BoardBackend::TEchoUf2 => {
            println!("1. Double-tap RESET so the TECHOBOOT drive mounts.");
            println!("2. Build the UF2 artifact.");
            println!("3. Copy t-echo.uf2 to TECHOBOOT.");
            println!("4. The T-Echo reboots into the new firmware after the copy completes.");
        }
        BoardBackend::HeltecV4EspFlash => {
            println!("1. Connect a USB-C data cable.");
            println!("2. Run `cargo run -p hopspot-flash -- flash heltec-v4`.");
            println!("3. Choose the port labeled USB JTAG/serial debug if prompted.");
            println!("4. If your device is not detected, hold BOOT, tap RESET, then release BOOT.");
            println!("5. Wait for flash verification, then reset once.");
        }
        BoardBackend::Planned => {
            println!("Local CLI flashing is not wired for this board yet.");
        }
    }
}

fn flash_board(
    board: &BoardTarget,
    repo: &Path,
    port: Option<&str>,
    monitor: bool,
    mount_override: Option<&Path>,
) -> AppResult<()> {
    ensure_supported(board)?;
    match board.backend {
        BoardBackend::TEchoUf2 => flash_t_echo(repo, mount_override),
        BoardBackend::HeltecV4EspFlash => flash_heltec_v4(repo, port, monitor),
        BoardBackend::Planned => unreachable!("planned boards are rejected by ensure_supported"),
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

fn flash_heltec_v4(repo: &Path, port: Option<&str>, monitor: bool) -> AppResult<()> {
    let firmware = build_heltec_v4_firmware(repo)?;

    println!();
    ui::print_section("Flashing Heltec V4");
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
        .arg("esp32s3")
        .arg("--flash-size")
        .arg("8mb")
        .arg("--partition-table")
        .arg(&firmware.partition_table)
        .arg("--target-app-partition")
        .arg("factory")
        .arg("--after")
        .arg("watchdog-reset")
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

    ui::print_note("Flash complete. Reset once if needed.");
    Ok(())
}

fn build_board(board: &BoardTarget, repo: &Path, out_root: &Path) -> AppResult<BuildOutput> {
    ensure_supported(board)?;
    match board.backend {
        BoardBackend::TEchoUf2 => build_t_echo(repo, out_root),
        BoardBackend::HeltecV4EspFlash => build_heltec_v4(repo, out_root),
        BoardBackend::Planned => unreachable!("planned boards are rejected by ensure_supported"),
    }
}

fn ensure_supported(board: &BoardTarget) -> AppResult<()> {
    if board.backend.ready() {
        Ok(())
    } else {
        Err(format!(
            "{} is not wired in hopspot-flash yet. Run without arguments for current options.",
            board.name
        ))
    }
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

fn build_heltec_v4(repo: &Path, out_root: &Path) -> AppResult<BuildOutput> {
    let firmware = build_heltec_v4_firmware(repo)?;

    let board_out = out_root
        .join("firmware")
        .join("hopspot")
        .join("heltec-v4")
        .join("latest");
    fs::create_dir_all(&board_out)
        .map_err(|err| format!("failed to create {}: {err}", board_out.display()))?;

    let artifact = board_out.join(HELTEC_V4_ARTIFACT);
    let metadata = board_out.join(format!("{HELTEC_V4_ARTIFACT}.json"));
    let web_manifest = board_out.join("manifest.json");

    run_status(
        Command::new("espflash")
            .arg("save-image")
            .arg("--chip")
            .arg("esp32s3")
            .arg("--flash-size")
            .arg("8mb")
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
    write_esp_metadata(
        &metadata,
        "heltec-v4",
        HELTEC_V4_PROFILE,
        HELTEC_V4_ARTIFACT,
        &sha256,
        size,
    )?;
    write_esp_web_manifest(&web_manifest, "Hopspot Heltec V4", HELTEC_V4_ARTIFACT)?;

    Ok(BuildOutput {
        artifact,
        metadata,
        web_manifest: Some(web_manifest),
        profile: HELTEC_V4_PROFILE,
        sha256,
        size,
    })
}

fn build_heltec_v4_firmware(repo: &Path) -> AppResult<EspFirmware> {
    ui::print_section("Building Heltec V4");
    let crate_dir = repo.join("personal-hopspot").join("app");
    let elf = crate_dir
        .join("target")
        .join(ESP32S3_TARGET)
        .join("release")
        .join("personal-hopspot-app");
    let partition_table = crate_dir.join(HELTEC_V4_PARTITIONS);

    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg("personal-hopspot-app")
        .arg("--target")
        .arg(ESP32S3_TARGET)
        .arg("-Zbuild-std=core,alloc")
        .arg("--features")
        .arg(HELTEC_V4_PROFILE)
        .current_dir(&crate_dir);
    let linker = configure_esp_toolchain(&mut cargo)?;
    ui::print_key_value("xtensa gcc", &linker.display().to_string());
    run_status(
        &mut cargo,
        "cargo build --release --bin personal-hopspot-app --target xtensa-esp32s3-none-elf -Zbuild-std=core,alloc --features full",
    )?;

    Ok(EspFirmware {
        elf,
        partition_table,
    })
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
    profile: &str,
    artifact: &str,
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
            "  \"chip\": \"esp32s3\",\n",
            "  \"flash_size\": \"8mb\",\n",
            "  \"partition_table\": \"personal-hopspot/app/{partition_table}\",\n",
            "  \"source\": \"personal-hopspot/app\"\n",
            "}}\n"
        ),
        board_slug = board_slug,
        profile = profile,
        artifact = artifact,
        sha256 = sha256,
        size = size,
        partition_table = HELTEC_V4_PARTITIONS,
    );
    fs::write(path, json).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn write_esp_web_manifest(path: &Path, name: &str, artifact: &str) -> AppResult<()> {
    let json = format!(
        concat!(
            "{{\n",
            "  \"name\": \"{name}\",\n",
            "  \"version\": \"preview\",\n",
            "  \"new_install_prompt_erase\": true,\n",
            "  \"new_install_improv_wait_time\": 0,\n",
            "  \"builds\": [\n",
            "    {{\n",
            "      \"chipFamily\": \"ESP32-S3\",\n",
            "      \"improv\": false,\n",
            "      \"parts\": [\n",
            "        {{ \"path\": \"{artifact}\", \"offset\": 0 }}\n",
            "      ]\n",
            "    }}\n",
            "  ]\n",
            "}}\n"
        ),
        name = name,
        artifact = artifact,
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
    let prefix = format!("export {key}=");
    let value = line.trim().strip_prefix(&prefix)?.trim();
    Some(
        value
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string(),
    )
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
