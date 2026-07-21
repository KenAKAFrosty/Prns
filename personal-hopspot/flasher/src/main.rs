mod build;
mod cli;
mod error;
mod esp;
mod events;
mod release;
mod splash;
mod techo;
mod toolchain;
mod ui;
mod wifi;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use prns_flash_manifest::{
    board_catalog, BoardCatalog, BoardCatalogEntry, ProvisioningAction, Transport,
};
use serde::Serialize;

use build::{assemble_manifest, build_board, default_artifact_root};
use cli::{ChannelArg, Cli, CommandMode, WifiMode};
use error::AppError;
use events::{Phase, Reporter};
use release::{prepare_candidate_target, prepare_published_target, PreparedTarget};
use wifi::WifiOptions;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let reporter = if cli.json_mode() {
        Reporter::json_lines()
    } else {
        Reporter::human()
    };
    match run(cli, reporter) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            reporter.error(&error);
            error.exit_code()
        }
    }
}

fn run(cli: Cli, reporter: Reporter) -> Result<(), AppError> {
    let catalog = board_catalog()
        .map_err(|error| AppError::trust(format!("embedded board catalog failed: {error}")))?;
    match cli.command {
        Some(CommandMode::List { json }) => list_boards(&catalog, json),
        Some(CommandMode::Doctor { board, port, json }) => {
            doctor(&catalog, board.as_deref(), port.as_deref(), json)
        }
        Some(CommandMode::Build { board, out_root }) => {
            let board = find_board(&catalog, &board)?;
            let repo = repo_root()?;
            let out_root = out_root.unwrap_or_else(|| default_artifact_root(&repo));
            let output = build_board(board, &repo, &out_root, reporter)?;
            println!("artifact directory: {}", output.output_dir.display());
            println!("target record: {}", output.target_record.display());
            Ok(())
        }
        Some(CommandMode::AssembleManifest {
            out_root,
            channel,
            commit,
            key_id,
        }) => {
            let path =
                assemble_manifest(&catalog, &repo_root()?, &out_root, channel, commit, key_id)?;
            println!("manifest: {}", path.display());
            Ok(())
        }
        Some(CommandMode::Flash {
            board,
            channel,
            version,
            allow_downgrade,
            port,
            wifi,
            wifi_ssid,
            wifi_password_stdin,
            wifi_from_env,
            offline,
            yes,
            monitor,
            json,
            local_build,
            candidate,
            mount,
        }) => {
            let board = find_board(&catalog, &board)?;
            let interactive = !json && ui::interactive_terminal();
            confirm_board(board, yes, interactive)?;
            if !local_build && candidate.is_none() {
                confirm_pinned_version(version.as_deref(), allow_downgrade, interactive)?;
            }
            let provisioning = wifi::resolve(
                board.supports_provisioning(),
                WifiOptions {
                    mode: wifi,
                    ssid: wifi_ssid,
                    password_stdin: wifi_password_stdin,
                    from_env: wifi_from_env,
                    interactive,
                },
            )?;
            execute_flash(
                &catalog,
                board,
                FlashRequest {
                    channel,
                    version: version.as_deref(),
                    port: port.as_deref(),
                    provisioning,
                    offline,
                    monitor,
                    local_build,
                    candidate: candidate.as_deref(),
                    mount: mount.as_deref(),
                },
                reporter,
            )
        }
        None => guided(&catalog, reporter),
    }
}

struct FlashRequest<'a> {
    channel: ChannelArg,
    version: Option<&'a str>,
    port: Option<&'a str>,
    provisioning: ProvisioningAction,
    offline: bool,
    monitor: bool,
    local_build: bool,
    candidate: Option<&'a Path>,
    mount: Option<&'a Path>,
}

fn execute_flash(
    catalog: &BoardCatalog,
    board: &BoardCatalogEntry,
    request: FlashRequest<'_>,
    reporter: Reporter,
) -> Result<(), AppError> {
    esp::begin_cancellable_operation()?;
    let prepared = if request.local_build {
        let repo = repo_root()?;
        build_board(board, &repo, &default_artifact_root(&repo), reporter)?.prepared
    } else if let Some(candidate) = request.candidate {
        prepare_candidate_target(catalog, &board.slug, request.channel, candidate, reporter)?
    } else {
        prepare_published_target(
            catalog,
            &board.slug,
            request.channel,
            request.version,
            request.offline,
            reporter,
        )?
    };
    if esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    verify_prepared_identity(board, &prepared)?;
    reporter.phase(
        Phase::Ready,
        Some(&board.slug),
        &format!(
            "{} {} is verified and ready; no full-chip erase will be performed.",
            board.display_name, prepared.version
        ),
    );
    match board.transport {
        Transport::EspSerial => esp::flash(
            board,
            &prepared.parts,
            &request.provisioning,
            request.port,
            request.monitor,
            reporter,
        ),
        Transport::Uf2MassStorage => {
            if !matches!(request.provisioning, ProvisioningAction::Preserve) {
                return Err(AppError::usage(
                    "T-Echo does not support Wi-Fi provisioning",
                ));
            }
            techo::flash(board, &prepared.parts, request.mount, reporter)
        }
    }
}

fn guided(catalog: &BoardCatalog, reporter: Reporter) -> Result<(), AppError> {
    if !ui::interactive_terminal() {
        return Err(AppError::usage(
            "guided mode requires a terminal; use `hopspot-flash flash <BOARD> --yes`",
        ));
    }
    ui::print_header();
    let labels = catalog
        .boards
        .iter()
        .map(|board| {
            format!(
                "{}  [{}]",
                board.display_name,
                transport_label(board.transport)
            )
        })
        .collect::<Vec<_>>();
    let Some(index) =
        ui::select("Which exact board are you flashing?", &labels, 0).map_err(AppError::usage)?
    else {
        return Ok(());
    };
    let board = catalog
        .boards
        .get(index)
        .ok_or_else(|| AppError::usage("board selection is out of range"))?;
    println!();
    print_board(board);
    confirm_board(board, false, true)?;
    let wifi_mode = if board.supports_provisioning() {
        let choices = vec![
            "Preserve existing Wi-Fi configuration (recommended)".to_string(),
            "Configure Wi-Fi locally for this flash".to_string(),
            "Clear Wi-Fi configuration explicitly".to_string(),
        ];
        match ui::select("Wi-Fi configuration", &choices, 0).map_err(AppError::usage)? {
            Some(1) => WifiMode::Configure,
            Some(2) => WifiMode::Clear,
            Some(_) => WifiMode::Preserve,
            None => return Ok(()),
        }
    } else {
        WifiMode::Preserve
    };
    let provisioning = wifi::resolve(
        board.supports_provisioning(),
        WifiOptions {
            mode: wifi_mode,
            ssid: None,
            password_stdin: false,
            from_env: false,
            interactive: true,
        },
    )?;
    execute_flash(
        catalog,
        board,
        FlashRequest {
            channel: ChannelArg::Stable,
            version: None,
            port: None,
            provisioning,
            offline: false,
            monitor: false,
            local_build: false,
            candidate: None,
            mount: None,
        },
        reporter,
    )
}

fn confirm_board(board: &BoardCatalogEntry, yes: bool, interactive: bool) -> Result<(), AppError> {
    if yes {
        return Ok(());
    }
    if !interactive {
        return Err(AppError::usage(format!(
            "confirm {} with --yes after checking the board label and image",
            board.display_name
        )));
    }
    let confirmed = ui::confirm(
        &format!("I physically checked that this is {}", board.display_name),
        false,
    )
    .map_err(AppError::usage)?;
    if confirmed {
        Ok(())
    } else {
        Err(AppError::Cancelled)
    }
}

fn confirm_pinned_version(
    version: Option<&str>,
    allow_downgrade: bool,
    interactive: bool,
) -> Result<(), AppError> {
    let Some(version) = version else {
        return Ok(());
    };
    if allow_downgrade {
        return Ok(());
    }
    if !interactive {
        return Err(AppError::usage(format!(
            "pinned version {version} may be a downgrade; acknowledge it with --allow-downgrade"
        )));
    }
    let confirmed = ui::confirm(
        &format!("Flash pinned version {version}, acknowledging that it may downgrade the device"),
        false,
    )
    .map_err(AppError::usage)?;
    if confirmed {
        Ok(())
    } else {
        Err(AppError::Cancelled)
    }
}

fn list_boards(catalog: &BoardCatalog, json: bool) -> Result<(), AppError> {
    if json {
        #[derive(Serialize)]
        struct BoardListEvent<'a> {
            schema: u8,
            event: &'static str,
            phase: &'static str,
            boards: &'a [BoardCatalogEntry],
        }
        println!(
            "{}",
            json_line(&BoardListEvent {
                schema: 1,
                event: "board_list",
                phase: "complete",
                boards: &catalog.boards,
            })?
        );
    } else {
        for board in &catalog.boards {
            println!(
                "{:<20} {:<12} {}",
                board.slug,
                transport_label(board.transport),
                board.display_name
            );
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct PortDiagnostic {
    name: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct DoctorOutput<'a> {
    schema: u8,
    event: &'static str,
    phase: &'static str,
    board: Option<&'a str>,
    requested_port: Option<&'a str>,
    serial_ports: Vec<PortDiagnostic>,
    techo_mounts: Vec<String>,
}

fn doctor(
    catalog: &BoardCatalog,
    board_slug: Option<&str>,
    requested_port: Option<&str>,
    json: bool,
) -> Result<(), AppError> {
    if let Some(slug) = board_slug {
        let _ = find_board(catalog, slug)?;
    }
    let ports = esp::diagnostic_ports()?
        .into_iter()
        .map(|port| PortDiagnostic {
            name: port.port_name,
            kind: match port.port_type {
                serialport::SerialPortType::UsbPort(_) => "usb",
                serialport::SerialPortType::BluetoothPort => "bluetooth",
                serialport::SerialPortType::PciPort => "pci",
                serialport::SerialPortType::Unknown => "unknown",
            },
        })
        .collect::<Vec<_>>();
    let mounts = techo::detect_mounts()
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let output = DoctorOutput {
        schema: 1,
        event: "doctor",
        phase: "complete",
        board: board_slug,
        requested_port,
        serial_ports: ports,
        techo_mounts: mounts,
    };
    if json {
        println!("{}", json_line(&output)?);
    } else {
        if let Some(board) = board_slug {
            println!("board: {board}");
        }
        println!("serial ports:");
        if output.serial_ports.is_empty() {
            println!("  none");
        }
        for port in &output.serial_ports {
            let requested = if Some(port.name.as_str()) == requested_port {
                " (requested)"
            } else {
                ""
            };
            println!("  {} [{}]{}", port.name, port.kind, requested);
        }
        println!("TECHOBOOT mounts:");
        if output.techo_mounts.is_empty() {
            println!("  none");
        }
        for mount in &output.techo_mounts {
            println!("  {mount}");
        }
    }
    Ok(())
}

fn json_line<T: Serialize>(value: &T) -> Result<String, AppError> {
    serde_json::to_string(value)
        .map_err(|error| AppError::usage(format!("could not encode JSON event: {error}")))
}

fn verify_prepared_identity(
    board: &BoardCatalogEntry,
    prepared: &PreparedTarget,
) -> Result<(), AppError> {
    if prepared.target.board_slug == board.slug && prepared.target.transport == board.transport {
        Ok(())
    } else {
        Err(AppError::trust(
            "prepared artifact does not match the selected board",
        ))
    }
}

fn find_board<'a>(
    catalog: &'a BoardCatalog,
    slug: &str,
) -> Result<&'a BoardCatalogEntry, AppError> {
    catalog.board(slug).ok_or_else(|| {
        AppError::usage(format!(
            "unknown board {slug:?}; supported: {}",
            catalog
                .boards
                .iter()
                .map(|board| board.slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

fn print_board(board: &BoardCatalogEntry) {
    ui::print_section(&board.display_name);
    ui::print_key_value("silicon", &board.silicon);
    ui::print_key_value("transport", transport_label(board.transport));
    ui::print_key_value("interfaces", &board.interfaces.join(", "));
    if board.slug == "heltec-v4" || board.slug == "t-beam-supreme" {
        ui::print_note(
            "This board shares ESP32-S3 silicon with another target; its exact model cannot be detected automatically.",
        );
    }
}

const fn transport_label(transport: Transport) -> &'static str {
    match transport {
        Transport::EspSerial => "ESP serial",
        Transport::Uf2MassStorage => "UF2",
    }
}

fn repo_root() -> Result<PathBuf, AppError> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            AppError::developer(format!(
                "cannot determine repository root from {}",
                manifest_dir.display()
            ))
        })
}
