use std::env;
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

#[derive(Parser)]
#[command(
    name = "hopspot-flash",
    about = "Interactive firmware builder and flasher for Personal Hopspot boards.",
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
    /// Build a local firmware artifact.
    Build {
        #[arg(value_enum)]
        board: BoardId,
        #[arg(long, value_name = "DIR")]
        out_root: Option<PathBuf>,
    },
    /// Build and flash/copy firmware to the board.
    Flash {
        #[arg(value_enum)]
        board: BoardId,
        #[arg(long, value_name = "DIR")]
        out_root: Option<PathBuf>,
        #[arg(long, value_name = "DIR")]
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
    Planned,
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
        backend: BoardBackend::Planned,
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
    uf2: PathBuf,
    metadata: PathBuf,
    profile: &'static str,
    sha256: String,
    size: u64,
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
            out_root,
            mount,
        }) => {
            let out_root = out_root.unwrap_or_else(|| default_artifact_root(&repo));
            flash_board(board.target(), &repo, &out_root, mount.as_deref())
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
                let ready = board.backend == BoardBackend::TEchoUf2;
                let state = match board.backend {
                    BoardBackend::TEchoUf2 => "ready",
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

    if board.backend == BoardBackend::Planned {
        println!(
            "{} is in the flasher catalog, but this local CLI backend is not wired yet.",
            board.name
        );
        println!("For now, use the hosted web flasher when its artifact is published.");
        return Ok(());
    }

    loop {
        let action = ui::select(
            "What do you want to do?",
            &[
                "Build firmware artifact".to_string(),
                "Build and copy to TECHOBOOT".to_string(),
                "Show flashing steps".to_string(),
                "Exit".to_string(),
            ],
            0,
        )?
        .unwrap_or(3);

        match action {
            0 => {
                let output = build_board(board, repo, &default_artifact_root(repo))?;
                print_build_output(&output);
                return Ok(());
            }
            1 => return flash_board(board, repo, &default_artifact_root(repo), None),
            2 => {
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
        BoardBackend::Planned => {
            println!("Local CLI flashing is not wired for this board yet.");
        }
    }
}

fn flash_board(
    board: &BoardTarget,
    repo: &Path,
    out_root: &Path,
    mount_override: Option<&Path>,
) -> AppResult<()> {
    ensure_supported(board)?;
    let output = build_board(board, repo, out_root)?;
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
    ui::print_key_value("from", &output.uf2.display().to_string());
    ui::print_key_value("to", &destination.display().to_string());
    fs::copy(&output.uf2, &destination).map_err(|err| {
        format!(
            "failed to copy {} to {}: {err}",
            output.uf2.display(),
            destination.display()
        )
    })?;
    let _ = Command::new("sync").status();

    ui::print_note("Copy complete. The T-Echo should reboot into the new firmware.");
    Ok(())
}

fn build_board(board: &BoardTarget, repo: &Path, out_root: &Path) -> AppResult<BuildOutput> {
    ensure_supported(board)?;
    build_t_echo(repo, out_root)
}

fn ensure_supported(board: &BoardTarget) -> AppResult<()> {
    if board.backend == BoardBackend::TEchoUf2 {
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
    run_status(
        Command::new("cargo")
            .arg("build")
            .arg("--release")
            .arg("--no-default-features")
            .arg("--features")
            .arg(T_ECHO_PROFILE)
            .current_dir(&crate_dir),
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
        uf2,
        metadata,
        profile: T_ECHO_PROFILE,
        sha256,
        size,
    })
}

fn print_build_output(output: &BuildOutput) {
    println!();
    ui::print_section("Artifact ready");
    ui::print_key_value("artifact", &output.uf2.display().to_string());
    ui::print_key_value("metadata", &output.metadata.display().to_string());
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
