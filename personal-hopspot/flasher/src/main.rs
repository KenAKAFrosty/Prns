mod build;
mod cache;
mod cli;
mod error;
mod esp;
mod events;
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
    assemble_manifest, build_board, default_artifact_root, BuildVersion, ManifestTargetProfile,
};
use cli::{CacheCommand, ChannelArg, Cli, CommandMode, WifiMode};
use error::AppError;
use events::{Phase, Reporter};
use release::{prepare_candidate_target, prepare_published_target, PreparedTarget};
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
    let prepared = if request.local_build {
        let repo = repo_root()?;
        build_board(
            board,
            &repo,
            &default_artifact_root(&repo),
            BuildVersion::Repository,
            reporter,
        )?
        .prepared
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
            uf2::flash(board, &prepared, request.mount, reporter)
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
    let Some(index) = ui::select("Which exact board are you flashing?", &labels, 0)
        .map_err(AppError::configuration)?
    else {
        return Ok(());
    };
    let board = catalog
        .boards
        .get(index)
        .ok_or_else(|| AppError::configuration("board selection is out of range"))?;
    println!();
    print_board(board);
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
        same_chip_board_ambiguity: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "uf2-mass-storage")]
    Uf2MassStorage { mount: String },
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
            "--port applies only to ESP serial boards; UF2 boards use a bootloader drive",
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
            let ambiguous_peer = ambiguous_esp_identity_peer(catalog, board);
            let same_chip_board_ambiguity = ambiguous_peer.is_some();
            let note = ambiguous_peer.map(|peer| {
                format!(
                    "This identity check cannot distinguish {} from {} because they share the same detectable chip and flash capacity; physically confirm the selected board.",
                    board.display_name, peer.display_name
                )
            });
            Some(DoctorCheck::EspSerial {
                port: report.port_name,
                detected_chip: report.detected_chip,
                flash_size: report.flash_size,
                same_chip_board_ambiguity,
                note,
            })
        }
        Some(board) => {
            let mount = uf2::doctor_mount_from(detected_mounts.clone(), board)?;
            Some(DoctorCheck::Uf2MassStorage {
                mount: mount.display().to_string(),
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
        schema: 1,
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
    if board.is_none_or(|board| board.transport == Transport::EspSerial) {
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
        Some(DoctorCheck::Uf2MassStorage { mount }) => {
            rendered.push_str("non-writing UF2 preflight: passed\n");
            rendered.push_str(&format!("  identifiable UF2 bootloader mount: {mount}\n"));
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

fn ambiguous_esp_identity_peer<'a>(
    catalog: &'a BoardCatalog,
    board: &BoardCatalogEntry,
) -> Option<&'a BoardCatalogEntry> {
    catalog.boards.iter().find(|candidate| {
        candidate.slug != board.slug
            && candidate.transport == Transport::EspSerial
            && candidate.expected_chip == board.expected_chip
            && candidate.flash_size == board.flash_size
    })
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

fn print_board(board: &BoardCatalogEntry) {
    ui::print_section(&board.display_name);
    ui::print_key_value("silicon", &board.silicon);
    ui::print_key_value("transport", transport_label(board.transport));
    ui::print_key_value("interfaces", &board.interfaces.join(", "));
    if board.slug == "heltec-v4" || board.slug == "heltec-v4-r8" {
        ui::print_note(
            "Heltec V4 S3R2 and S3R8 share the same chip and 16MB flash; pick the matching firmware because the S3R2 pinout prevents Octal PSRAM from operating on the S3R8.",
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
    fn esp_doctor_json_exposes_same_chip_ambiguity() {
        let encoded = json_line(&DoctorOutput {
            schema: 1,
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
                same_chip_board_ambiguity: true,
                note: Some("cannot distinguish these two board models".to_string()),
            }),
        })
        .expect("doctor output serializes");
        assert_eq!(
            encoded,
            r#"{"schema":1,"event":"doctor","phase":"complete","board":"heltec-v4","requested_port":"fake-port","serial_ports":[{"name":"fake-port","kind":"usb"}],"techo_mounts":[],"check":{"transport":"esp-serial","port":"fake-port","detected_chip":"esp32s3","flash_size":16777216,"same_chip_board_ambiguity":true,"note":"cannot distinguish these two board models"}}"#
        );
    }

    #[test]
    fn catalog_capacities_distinguish_shipping_esp_identities() {
        let catalog = board_catalog().expect("catalog");
        assert_eq!(
            ambiguous_esp_identity_peer(&catalog, catalog.board("heltec-v4").expect("Heltec"))
                .map(|board| board.slug.as_str()),
            Some("heltec-v4-r8")
        );
        assert_eq!(
            ambiguous_esp_identity_peer(
                &catalog,
                catalog.board("heltec-v4-r8").expect("Heltec R8")
            )
            .map(|board| board.slug.as_str()),
            Some("heltec-v4")
        );
        assert!(ambiguous_esp_identity_peer(
            &catalog,
            catalog.board("t-beam-supreme").expect("T-Beam")
        )
        .is_none());
        assert!(ambiguous_esp_identity_peer(
            &catalog,
            catalog.board("xiao-esp32-c6").expect("XIAO")
        )
        .is_none());
    }

    #[test]
    fn esp_human_doctor_output_prioritizes_usb_and_omits_techo_mounts() {
        let catalog = board_catalog().expect("catalog");
        let board = catalog.board("t-beam-supreme").expect("T-Beam");
        let output = DoctorOutput {
            schema: 1,
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
                same_chip_board_ambiguity: false,
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
            schema: 1,
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
            }),
        };

        assert_eq!(
            human_doctor_output(&output, Some(board), None),
            "board: t-echo\nUF2 bootloader mounts:\n  /media/operator/TECHOBOOT\nnon-writing UF2 preflight: passed\n  identifiable UF2 bootloader mount: /media/operator/TECHOBOOT\n"
        );
    }
}
