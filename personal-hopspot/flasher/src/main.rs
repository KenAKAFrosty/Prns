mod build;
mod cache;
mod cli;
mod error;
mod esp;
mod events;
mod nrf_serial_dfu;
mod release;
mod splash;
mod toolchain;
mod uf2;
mod ui;
mod wifi;

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{error::ErrorKind, Parser};
use prns_flash_manifest::{
    board_catalog, BoardCatalog, BoardCatalogEntry, ProvisioningAction, Transport,
};
use serde::Serialize;

use build::{
    assemble_manifest, build_board, build_board_for_flash, default_artifact_root, BuildVersion,
    ManifestTargetProfile,
};
use cli::{CacheCommand, ChannelArg, Cli, CommandMode, WifiMode};
use error::AppError;
use events::{Phase, Reporter};
use release::{verify_candidate_target, verify_published_target, PreparedTarget};
use wifi::WifiOptions;

fn main() -> ExitCode {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let json_requested = requests_json_output(&arguments);
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error) => return report_parse_error(error, json_requested),
    };
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

fn requests_json_output(arguments: &[OsString]) -> bool {
    arguments.iter().skip(1).any(|argument| {
        argument == OsStr::new("--json")
            || argument
                .to_str()
                .is_some_and(|argument| argument.starts_with("--json="))
    })
}

fn report_parse_error(error: clap::Error, json_requested: bool) -> ExitCode {
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        let code = error.exit_code();
        let _ = error.print();
        return ExitCode::from(u8::try_from(code).unwrap_or(2));
    }

    if json_requested {
        // Clap's rendered error can repeat arbitrary argv values. Never include
        // it in machine output: a misspelled credential option must not turn a
        // secret into a diagnostic. Emit one stable terminal schema-1 event.
        Reporter::json_lines().error(&AppError::arguments(
            "invalid command-line arguments; run `hopspot-flash --help` for valid options",
        ));
        ExitCode::from(2)
    } else {
        let code = error.exit_code();
        let _ = error.print();
        ExitCode::from(u8::try_from(code).unwrap_or(2))
    }
}

fn run(cli: Cli, reporter: Reporter) -> Result<(), AppError> {
    let catalog = board_catalog().map_err(|error| {
        AppError::trust_catalog(format!("embedded board catalog failed: {error}"))
    })?;
    match cli.command {
        Some(CommandMode::List { json }) => list_boards(&catalog, json),
        Some(CommandMode::Doctor { board, port, json }) => {
            doctor(&catalog, board.as_deref(), port.as_deref(), json)
        }
        Some(CommandMode::Cache {
            command: CacheCommand::Import { candidate, .. },
        }) => {
            esp::begin_cancellable_operation()?;
            let imported = cache::import_signed_candidate(&catalog, &candidate, reporter)?;
            reporter.operation_success(&format!(
                "Imported signed {} {} candidate ({} artifacts, {} bytes).",
                imported.channel,
                imported.version,
                imported.artifact_count,
                imported.artifact_bytes
            ));
            Ok(())
        }
        Some(CommandMode::Build {
            board,
            out_root,
            developer_version,
        }) => {
            let board = find_board(&catalog, &board)?;
            let repo = repo_root()?;
            let out_root = out_root.unwrap_or_else(|| default_artifact_root(&repo));
            let build_version = developer_version
                .as_deref()
                .map(BuildVersion::Developer)
                .unwrap_or(BuildVersion::Repository);
            let output = build_board(board, &repo, &out_root, build_version, reporter)?;
            println!("artifact directory: {}", output.output_dir.display());
            println!("target record: {}", output.target_record.display());
            Ok(())
        }
        Some(CommandMode::AssembleManifest {
            out_root,
            channel,
            commit,
            key_id,
            developer_version,
            boards,
        }) => {
            let target_profile = match developer_version.as_deref() {
                Some(version) => ManifestTargetProfile::LocalDevelopment {
                    version,
                    board_slugs: &boards,
                },
                None => ManifestTargetProfile::Production,
            };
            let path = assemble_manifest(
                &catalog,
                &repo_root()?,
                &out_root,
                channel,
                commit,
                key_id,
                target_profile,
            )?;
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
            tcp_client,
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
                board.supports_tcp_client_provisioning(),
                WifiOptions {
                    mode: wifi,
                    ssid: wifi_ssid,
                    password_stdin: wifi_password_stdin,
                    from_env: wifi_from_env,
                    tcp_client,
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
    let (prepared, detected_uf2) = if request.local_build {
        let detected_uf2 = match board.transport {
            Transport::EspSerial => None,
            Transport::Uf2MassStorage => Some(uf2::detect_device(board, request.mount)?),
            Transport::NrfSerialDfu => None,
        };
        let repo = repo_root()?;
        let prepared = match &detected_uf2 {
            Some(device) => build_board_for_flash(
                board,
                &repo,
                &default_artifact_root(&repo),
                BuildVersion::Repository,
                device.softdevice(),
                reporter,
            )?
            .into_prepared()?,
            None => build_board(
                board,
                &repo,
                &default_artifact_root(&repo),
                BuildVersion::Repository,
                reporter,
            )?
            .into_prepared()?,
        };
        (prepared, detected_uf2)
    } else {
        let verified = if let Some(candidate) = request.candidate {
            verify_candidate_target(catalog, &board.slug, request.channel, candidate, reporter)?
        } else {
            verify_published_target(
                catalog,
                &board.slug,
                request.channel,
                request.version,
                request.offline,
                reporter,
            )?
        };
        let detected_uf2 = match board.transport {
            Transport::EspSerial => None,
            Transport::Uf2MassStorage => Some(uf2::detect_device(board, request.mount)?),
            Transport::NrfSerialDfu => None,
        };
        let prepared = verified.prepare(
            detected_uf2.as_ref().map(|device| device.softdevice()),
            reporter,
        )?;
        (prepared, detected_uf2)
    };
    if esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    if prepared.board_id().as_str() != board.slug {
        return Err(AppError::trust_identity(
            "prepared artifact does not match the selected board",
        ));
    }
    reporter.phase(
        Phase::Ready,
        Some(&board.slug),
        &format!(
            "{} {} is verified and ready; no full-chip erase will be performed.",
            board.display_name,
            prepared.version()
        ),
    );
    match (board.transport, prepared) {
        (Transport::EspSerial, PreparedTarget::EspSerial(prepared)) => esp::flash(
            board,
            &prepared,
            &request.provisioning,
            request.port,
            request.monitor,
            reporter,
        ),
        (Transport::Uf2MassStorage, PreparedTarget::Uf2(prepared)) => {
            if !matches!(request.provisioning, ProvisioningAction::Preserve) {
                return Err(AppError::unsupported_operation(format!(
                    "{} does not support Wi-Fi provisioning",
                    board.display_name
                )));
            }
            let device = detected_uf2.ok_or_else(|| {
                AppError::device_identity("UF2 device selection disappeared before delivery")
            })?;
            uf2::flash(board, &prepared, device, reporter)
        }
        (Transport::NrfSerialDfu, PreparedTarget::NrfSerialDfu(prepared)) => {
            if !matches!(request.provisioning, ProvisioningAction::Preserve) {
                return Err(AppError::unsupported_operation(format!(
                    "{} does not support Wi-Fi provisioning",
                    board.display_name
                )));
            }
            if request.monitor {
                return Err(AppError::unsupported_operation(
                    "Nordic serial DFU does not provide a post-flash serial monitor",
                ));
            }
            nrf_serial_dfu::flash(board, &prepared, request.port, reporter)
        }
        _ => Err(AppError::trust_identity(
            "prepared artifact transport does not match the selected board",
        )),
    }
}

fn guided(catalog: &BoardCatalog, reporter: Reporter) -> Result<(), AppError> {
    if !ui::interactive_terminal() {
        return Err(AppError::arguments(
            "guided mode requires a terminal; use `hopspot-flash flash <BOARD> --yes`",
        ));
    }
    ui::print_header();
    let boards = catalog.shipping_boards().collect::<Vec<_>>();
    let labels = boards
        .iter()
        .map(|board| {
            format!(
                "{}  [{}]",
                board.display_name,
                transport_label(board.transport)
            )
        })
        .collect::<Vec<_>>();
    let Some(index) = ui::select("Which exact board are you flashing?", &labels, 0)
        .map_err(AppError::configuration)?
    else {
        return Ok(());
    };
    let board = boards
        .get(index)
        .ok_or_else(|| AppError::configuration("board selection is out of range"))?;
    println!();
    print_board(catalog, board);
    confirm_board(board, false, true)?;
    let wifi_mode = if board.supports_provisioning() {
        let choices = vec![
            "Preserve existing Wi-Fi configuration (recommended)".to_string(),
            "Configure Wi-Fi locally for this flash".to_string(),
            "Clear Wi-Fi configuration explicitly".to_string(),
        ];
        match ui::select("Wi-Fi configuration", &choices, 0).map_err(AppError::configuration)? {
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
        board.supports_tcp_client_provisioning(),
        WifiOptions {
            mode: wifi_mode,
            ssid: None,
            password_stdin: false,
            from_env: false,
            tcp_client: None,
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
        return Err(AppError::confirmation(format!(
            "confirm {} with --yes after checking the board label and image",
            board.display_name
        )));
    }
    let confirmed = ui::confirm(
        &format!("I physically checked that this is {}", board.display_name),
        false,
    )
    .map_err(AppError::confirmation)?;
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
        return Err(AppError::confirmation(format!(
            "pinned version {version} may be a downgrade; acknowledge it with --allow-downgrade"
        )));
    }
    let confirmed = ui::confirm(
        &format!("Flash pinned version {version}, acknowledging that it may downgrade the device"),
        false,
    )
    .map_err(AppError::confirmation)?;
    if confirmed {
        Ok(())
    } else {
        Err(AppError::Cancelled)
    }
}

fn list_boards(catalog: &BoardCatalog, json: bool) -> Result<(), AppError> {
    let boards = catalog.shipping_boards().collect::<Vec<_>>();
    if json {
        #[derive(Serialize)]
        struct BoardListEvent<'a> {
            schema: u8,
            event: &'static str,
            phase: &'static str,
            boards: &'a [&'a BoardCatalogEntry],
        }
        println!(
            "{}",
            json_line(&BoardListEvent {
                schema: 1,
                event: "board_list",
                phase: "complete",
                boards: &boards,
            })?
        );
    } else {
        for board in boards {
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
struct EspIdentityPeer {
    slug: String,
    display_name: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    check: Option<DoctorCheck>,
}

#[derive(Serialize)]
#[serde(tag = "transport")]
enum DoctorCheck {
    #[serde(rename = "esp-serial")]
    EspSerial {
        port: String,
        detected_chip: String,
        flash_size: u32,
        indistinguishable_boards: Vec<EspIdentityPeer>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "uf2-mass-storage")]
    Uf2MassStorage {
        mount: String,
        board_id: String,
        bootloader_version: String,
        softdevice_family: String,
        softdevice_version: String,
        compatibility_variant: String,
    },
    #[serde(rename = "nrf-serial-dfu")]
    NrfSerialDfu {
        port: String,
        mode: nrf_serial_dfu::DeviceMode,
        vendor_id: u16,
        product_id: u16,
    },
}

fn doctor(
    catalog: &BoardCatalog,
    board_slug: Option<&str>,
    requested_port: Option<&str>,
    json: bool,
) -> Result<(), AppError> {
    let board = board_slug
        .map(|slug| find_board(catalog, slug))
        .transpose()?;
    if board.is_some_and(|board| board.transport == Transport::Uf2MassStorage)
        && requested_port.is_some()
    {
        return Err(AppError::unsupported_operation(
            "--port applies only to serial boards; UF2 boards use a bootloader drive",
        ));
    }
    if board.is_some() {
        esp::begin_cancellable_operation()?;
    }
    let detected_ports = if board.is_some_and(|board| board.transport == Transport::Uf2MassStorage)
    {
        Vec::new()
    } else {
        esp::diagnostic_ports()?
    };
    let detected_mounts = uf2::detect_any_uf2_mounts(catalog);
    let check = match board {
        Some(board) if board.transport == Transport::EspSerial => {
            if !json {
                println!(
                    "Running a non-writing identity preflight for {}…",
                    board.display_name
                );
            }
            let report = esp::doctor(board, detected_ports.clone(), requested_port)?;
            let indistinguishable = indistinguishable_esp_boards(catalog, board);
            let note = (!indistinguishable.is_empty()).then(|| {
                let names = indistinguishable
                    .iter()
                    .map(|candidate| candidate.display_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "This identity check cannot distinguish {} from these board models because they share the detected ESP chip and flash capacity: {names}. Physically confirm the selected board; ROM detection does not identify pinout, PSRAM mode, radio, or display.",
                    board.display_name
                )
            });
            let indistinguishable_boards = indistinguishable
                .into_iter()
                .map(|candidate| EspIdentityPeer {
                    slug: candidate.slug.clone(),
                    display_name: candidate.display_name.clone(),
                })
                .collect();
            Some(DoctorCheck::EspSerial {
                port: report.port_name,
                detected_chip: report.detected_chip,
                flash_size: report.flash_size,
                indistinguishable_boards,
                note,
            })
        }
        Some(board) if board.transport == Transport::Uf2MassStorage => {
            let device = uf2::detect_device(board, None)?;
            Some(DoctorCheck::Uf2MassStorage {
                mount: device.mount().display().to_string(),
                board_id: device.identity().board_id().to_string(),
                bootloader_version: device.identity().bootloader_version().to_string(),
                softdevice_family: device.softdevice().family().as_str().to_string(),
                softdevice_version: device.softdevice().version().as_str().to_string(),
                compatibility_variant: device.compatibility_variant().to_string(),
            })
        }
        Some(board) => {
            let report = nrf_serial_dfu::doctor(board, detected_ports.clone(), requested_port)?;
            Some(DoctorCheck::NrfSerialDfu {
                port: report.port_name,
                mode: report.mode,
                vendor_id: report.vendor_id,
                product_id: report.product_id,
            })
        }
        None => None,
    };
    let ports = detected_ports
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
    let mounts = detected_mounts
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let output = DoctorOutput {
        schema: 3,
        event: "doctor",
        phase: "complete",
        board: board_slug,
        requested_port,
        serial_ports: ports,
        techo_mounts: mounts,
        check,
    };
    if json {
        println!("{}", json_line(&output)?);
    } else {
        print!("{}", human_doctor_output(&output, board, requested_port));
    }
    Ok(())
}

fn human_doctor_output(
    output: &DoctorOutput<'_>,
    board: Option<&BoardCatalogEntry>,
    requested_port: Option<&str>,
) -> String {
    let mut rendered = String::new();
    if let Some(board) = output.board {
        rendered.push_str(&format!("board: {board}\n"));
    }
    if board.is_none_or(|board| board.transport != Transport::Uf2MassStorage) {
        rendered.push_str("serial ports:\n");
        let ports = human_serial_ports(&output.serial_ports, requested_port);
        if ports.is_empty() {
            rendered.push_str("  none\n");
        }
        for port in ports {
            let requested = if Some(port.name.as_str()) == requested_port {
                " (requested)"
            } else {
                ""
            };
            rendered.push_str(&format!("  {} [{}]{requested}\n", port.name, port.kind));
        }
    }
    if board.is_none_or(|board| board.transport == Transport::Uf2MassStorage) {
        rendered.push_str("UF2 bootloader mounts:\n");
        if output.techo_mounts.is_empty() {
            rendered.push_str("  none\n");
        }
        for mount in &output.techo_mounts {
            rendered.push_str(&format!("  {mount}\n"));
        }
    }
    match &output.check {
        Some(DoctorCheck::EspSerial {
            port,
            detected_chip,
            flash_size,
            note,
            ..
        }) => {
            rendered.push_str("non-writing ESP preflight: passed\n");
            rendered.push_str(&format!("  port: {port}\n"));
            rendered.push_str(&format!("  detected chip: {detected_chip}\n"));
            rendered.push_str(&format!("  detected flash: {flash_size} bytes\n"));
            if let Some(note) = note {
                rendered.push_str(&format!("  board confirmation: {note}\n"));
            }
        }
        Some(DoctorCheck::Uf2MassStorage {
            mount,
            board_id,
            bootloader_version,
            softdevice_family,
            softdevice_version,
            compatibility_variant,
        }) => {
            rendered.push_str("non-writing UF2 preflight: passed\n");
            rendered.push_str(&format!("  identifiable UF2 bootloader mount: {mount}\n"));
            rendered.push_str(&format!("  Board-ID: {board_id}\n"));
            rendered.push_str(&format!("  bootloader: {bootloader_version}\n"));
            rendered.push_str(&format!(
                "  SoftDevice: {softdevice_family} {softdevice_version}\n"
            ));
            rendered.push_str(&format!(
                "  compatibility variant: {compatibility_variant}\n"
            ));
        }
        Some(DoctorCheck::NrfSerialDfu {
            port,
            mode,
            vendor_id,
            product_id,
        }) => {
            rendered.push_str("non-writing Nordic serial DFU preflight: passed\n");
            rendered.push_str(&format!("  port: {port}\n"));
            rendered.push_str(&format!("  device mode: {mode}\n"));
            rendered.push_str(&format!(
                "  USB identity: {vendor_id:04x}:{product_id:04x}\n"
            ));
        }
        None => {}
    }
    rendered
}

fn human_serial_ports<'a>(
    ports: &'a [PortDiagnostic],
    requested_port: Option<&str>,
) -> Vec<&'a PortDiagnostic> {
    let has_usb = ports.iter().any(|port| port.kind == "usb");
    ports
        .iter()
        .filter(|port| {
            !has_usb
                || port.kind != "unknown"
                || !port.name.starts_with("/dev/ttyS")
                || Some(port.name.as_str()) == requested_port
        })
        .collect()
}

fn indistinguishable_esp_boards<'a>(
    catalog: &'a BoardCatalog,
    board: &BoardCatalogEntry,
) -> Vec<&'a BoardCatalogEntry> {
    catalog
        .boards
        .iter()
        .filter(|candidate| {
            candidate.slug != board.slug
                && candidate.transport == Transport::EspSerial
                && candidate.expected_chip == board.expected_chip
                && candidate.flash_size == board.flash_size
        })
        .collect()
}

fn json_line<T: Serialize>(value: &T) -> Result<String, AppError> {
    serde_json::to_string(value)
        .map_err(|error| AppError::output(format!("could not encode JSON event: {error}")))
}

fn find_board<'a>(
    catalog: &'a BoardCatalog,
    slug: &str,
) -> Result<&'a BoardCatalogEntry, AppError> {
    catalog.board(slug).ok_or_else(|| {
        AppError::arguments(format!(
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

fn print_board(catalog: &BoardCatalog, board: &BoardCatalogEntry) {
    ui::print_section(&board.display_name);
    ui::print_key_value("silicon", &board.silicon);
    ui::print_key_value("transport", transport_label(board.transport));
    ui::print_key_value("interfaces", &board.interfaces.join(", "));
    if !indistinguishable_esp_boards(catalog, board).is_empty() {
        ui::print_note(
            "Multiple board models share this detectable ESP chip and flash capacity. Run doctor to list every match, then physically confirm the selected pinout, PSRAM mode, radio, and display.",
        );
    }
}

const fn transport_label(transport: Transport) -> &'static str {
    match transport {
        Transport::EspSerial => "ESP serial",
        Transport::Uf2MassStorage => "UF2",
        Transport::NrfSerialDfu => "Nordic serial DFU",
    }
}

fn repo_root() -> Result<PathBuf, AppError> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            AppError::developer_repository(format!(
                "cannot determine repository root from {}",
                manifest_dir.display()
            ))
        })
}

#[cfg(test)]
mod doctor_tests {
    use super::*;

    #[test]
    fn t_echo_doctor_rejects_serial_port_without_touching_devices() {
        let catalog = board_catalog().expect("catalog");
        assert!(matches!(
            doctor(&catalog, Some("t-echo"), Some("unused-port"), true),
            Err(AppError::Usage(message)) if message.to_string().contains("bootloader drive")
        ));
    }

    #[test]
    fn esp_doctor_json_exposes_every_indistinguishable_board() {
        let encoded = json_line(&DoctorOutput {
            schema: 3,
            event: "doctor",
            phase: "complete",
            board: Some("heltec-v4"),
            requested_port: Some("fake-port"),
            serial_ports: vec![PortDiagnostic {
                name: "fake-port".to_string(),
                kind: "usb",
            }],
            techo_mounts: Vec::new(),
            check: Some(DoctorCheck::EspSerial {
                port: "fake-port".to_string(),
                detected_chip: "esp32s3".to_string(),
                flash_size: 16 * 1024 * 1024,
                indistinguishable_boards: vec![
                    EspIdentityPeer {
                        slug: "heltec-v4-r8".to_string(),
                        display_name: "Heltec LoRa 32 V4 (S3R8)".to_string(),
                    },
                    EspIdentityPeer {
                        slug: "heltec-e290".to_string(),
                        display_name: "Heltec Vision Master E290-HF".to_string(),
                    },
                ],
                note: Some("cannot distinguish these two board models".to_string()),
            }),
        })
        .expect("doctor output serializes");
        assert_eq!(
            encoded,
            r#"{"schema":3,"event":"doctor","phase":"complete","board":"heltec-v4","requested_port":"fake-port","serial_ports":[{"name":"fake-port","kind":"usb"}],"techo_mounts":[],"check":{"transport":"esp-serial","port":"fake-port","detected_chip":"esp32s3","flash_size":16777216,"indistinguishable_boards":[{"slug":"heltec-v4-r8","display_name":"Heltec LoRa 32 V4 (S3R8)"},{"slug":"heltec-e290","display_name":"Heltec Vision Master E290-HF"}],"note":"cannot distinguish these two board models"}}"#
        );
    }

    #[test]
    fn indistinguishable_board_sets_cover_zero_one_and_multiple_peers() {
        let catalog = board_catalog().expect("catalog");
        assert_eq!(
            indistinguishable_esp_boards(&catalog, catalog.board("heltec-v4").expect("Heltec"))
                .into_iter()
                .map(|board| board.slug.as_str())
                .collect::<Vec<_>>(),
            ["heltec-v4-r8", "heltec-e290"]
        );
        assert_eq!(
            indistinguishable_esp_boards(&catalog, catalog.board("heltec-e290").expect("E290"))
                .into_iter()
                .map(|board| board.slug.as_str())
                .collect::<Vec<_>>(),
            ["heltec-v4", "heltec-v4-r8"]
        );
        assert!(indistinguishable_esp_boards(
            &catalog,
            catalog.board("xiao-esp32-c6").expect("XIAO")
        )
        .is_empty());

        let mut two_board_catalog = catalog.clone();
        two_board_catalog
            .boards
            .retain(|board| board.slug != "heltec-e290");
        assert_eq!(
            indistinguishable_esp_boards(
                &two_board_catalog,
                two_board_catalog.board("heltec-v4").expect("Heltec")
            )
            .into_iter()
            .map(|board| board.slug.as_str())
            .collect::<Vec<_>>(),
            ["heltec-v4-r8"]
        );
    }

    #[test]
    fn esp_human_doctor_output_prioritizes_usb_and_omits_techo_mounts() {
        let catalog = board_catalog().expect("catalog");
        let board = catalog.board("t-beam-supreme").expect("T-Beam");
        let output = DoctorOutput {
            schema: 3,
            event: "doctor",
            phase: "complete",
            board: Some("t-beam-supreme"),
            requested_port: None,
            serial_ports: vec![
                PortDiagnostic {
                    name: "/dev/ttyS0".to_string(),
                    kind: "unknown",
                },
                PortDiagnostic {
                    name: "/dev/ttyACM0".to_string(),
                    kind: "usb",
                },
            ],
            techo_mounts: vec!["/media/operator/TECHOBOOT".to_string()],
            check: Some(DoctorCheck::EspSerial {
                port: "/dev/ttyACM0".to_string(),
                detected_chip: "esp32s3".to_string(),
                flash_size: 8 * 1024 * 1024,
                indistinguishable_boards: Vec::new(),
                note: None,
            }),
        };

        assert_eq!(
            human_doctor_output(&output, Some(board), None),
            "board: t-beam-supreme\nserial ports:\n  /dev/ttyACM0 [usb]\nnon-writing ESP preflight: passed\n  port: /dev/ttyACM0\n  detected chip: esp32s3\n  detected flash: 8388608 bytes\n"
        );
    }

    #[test]
    fn t_echo_human_doctor_output_only_reports_uf2_mounts() {
        let catalog = board_catalog().expect("catalog");
        let board = catalog.board("t-echo").expect("T-Echo");
        let output = DoctorOutput {
            schema: 3,
            event: "doctor",
            phase: "complete",
            board: Some("t-echo"),
            requested_port: None,
            serial_ports: vec![PortDiagnostic {
                name: "/dev/ttyACM0".to_string(),
                kind: "usb",
            }],
            techo_mounts: vec!["/media/operator/TECHOBOOT".to_string()],
            check: Some(DoctorCheck::Uf2MassStorage {
                mount: "/media/operator/TECHOBOOT".to_string(),
                board_id: "nrf52840-techo-v1".to_string(),
                bootloader_version: "0.6.1".to_string(),
                softdevice_family: "s140".to_string(),
                softdevice_version: "7.3.0".to_string(),
                compatibility_variant: "s140-7.3.0-fwid-0x0123".to_string(),
            }),
        };

        assert_eq!(
            human_doctor_output(&output, Some(board), None),
            "board: t-echo\nUF2 bootloader mounts:\n  /media/operator/TECHOBOOT\nnon-writing UF2 preflight: passed\n  identifiable UF2 bootloader mount: /media/operator/TECHOBOOT\n  Board-ID: nrf52840-techo-v1\n  bootloader: 0.6.1\n  SoftDevice: s140 7.3.0\n  compatibility variant: s140-7.3.0-fwid-0x0123\n"
        );
    }
}
