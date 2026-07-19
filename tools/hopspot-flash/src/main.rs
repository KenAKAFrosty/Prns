mod boards;
mod build;
mod cli;
mod flash;
mod splash;
mod toolchain;
mod ui;
mod wifi;

use std::path::{Path, PathBuf};

use clap::Parser;

use boards::{BoardBackend, BoardTarget, BOARDS};
use build::{build_board, default_artifact_root, print_build_output};
use cli::{Cli, CommandMode};
use flash::flash_board;
use wifi::{prompt_wifi_config, wifi_config_from_args};

type AppResult<T> = Result<T, String>;

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

fn repo_root() -> AppResult<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("cannot determine repo root from {}", manifest_dir.display()))
}
