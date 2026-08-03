use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use prns_flash_manifest::{BoardBuild, BoardCatalog, BoardCatalogEntry, Uf2BoardIdPrefix, Uf2Build};

use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedUf2Target;

const REBOOT_TIMEOUT: Duration = Duration::from_secs(20);

enum Uf2CopyOutcome {
    Synchronized,
    RebootObserved,
}

pub(crate) fn flash(
    board: &BoardCatalogEntry,
    target: &PreparedUf2Target,
    mount_override: Option<&Path>,
    reporter: Reporter,
) -> Result<(), AppError> {
    let build = uf2_build(board)?;
    let mount_label = build.mount_label.as_str();
    let mount = select_mount(board, detect_mounts_for(board)?, mount_override)?;

    let destination = mount.join("prns-hopspot.uf2");
    reporter.phase(
        Phase::Writing,
        Some(&board.slug),
        &format!("Copying verified UF2 to {}…", destination.display()),
    );
    let copy_outcome = copy_uf2(
        &destination,
        &mount,
        target.part().bytes(),
        &board.slug,
        mount_label,
        reporter,
    )?;

    if matches!(copy_outcome, Uf2CopyOutcome::Synchronized) {
        reporter.phase(
            Phase::Resetting,
            Some(&board.slug),
            &format!("Waiting for {mount_label} to disappear as the device reboots…"),
        );
        wait_for_reboot(&mount, mount_label, REBOOT_TIMEOUT, Duration::from_millis(200))?;
    }
    if crate::esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    reporter.success(
        &board.slug,
        &format!(
            "Verified UF2 delivered and the {} bootloader drive rebooted.",
            board.display_name
        ),
    );
    Ok(())
}

fn copy_uf2(
    destination: &Path,
    mount: &Path,
    bytes: &[u8],
    board_slug: &str,
    mount_label: &str,
    reporter: Reporter,
) -> Result<Uf2CopyOutcome, AppError> {
    let mut output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)
        .map_err(|error| {
            AppError::uf2_delivery(format!("could not create UF2 on {mount_label}: {error}"))
        })?;
    let mut written = 0usize;
    for chunk in bytes.chunks(64 * 1024) {
        if crate::esp::cancelled() {
            drop(output);
            let _ = fs::remove_file(destination);
            return Err(AppError::Cancelled);
        }
        output
            .write_all(chunk)
            .map_err(|error| AppError::uf2_delivery(format!("UF2 copy failed: {error}")))?;
        written += chunk.len();
        reporter.progress(
            Phase::Writing,
            Some(board_slug),
            written as u64,
            bytes.len() as u64,
        );
    }
    let file_sync = output.flush().and_then(|_| output.sync_all());
    drop(output);
    if let Err(error) = file_sync {
        return confirm_reboot_after_synchronization_interruption(
            mount,
            board_slug,
            mount_label,
            reporter,
            "UF2 flush/sync failed",
            error,
            REBOOT_TIMEOUT,
            Duration::from_millis(200),
        );
    }
    if let Err(error) = sync_mount_directory(mount) {
        return confirm_reboot_after_synchronization_interruption(
            mount,
            board_slug,
            mount_label,
            reporter,
            &format!("{mount_label} directory sync failed"),
            error,
            REBOOT_TIMEOUT,
            Duration::from_millis(200),
        );
    }
    Ok(Uf2CopyOutcome::Synchronized)
}

#[allow(clippy::too_many_arguments)]
fn confirm_reboot_after_synchronization_interruption(
    mount: &Path,
    board_slug: &str,
    mount_label: &str,
    reporter: Reporter,
    operation: &str,
    error: std::io::Error,
    timeout: Duration,
    poll: Duration,
) -> Result<Uf2CopyOutcome, AppError> {
    reporter.phase(
        Phase::Resetting,
        Some(board_slug),
        &format!("UF2 synchronization was interrupted; checking whether {mount_label} rebooted…"),
    );
    match wait_for_reboot(mount, mount_label, timeout, poll) {
        Ok(()) => Ok(Uf2CopyOutcome::RebootObserved),
        Err(AppError::Cancelled) => Err(AppError::Cancelled),
        Err(_) => Err(AppError::uf2_delivery(format!("{operation}: {error}"))),
    }
}

fn wait_for_reboot(
    mount: &Path,
    mount_label: &str,
    timeout: Duration,
    poll: Duration,
) -> Result<(), AppError> {
    let deadline = Instant::now() + timeout;
    while mount.exists() && Instant::now() < deadline {
        if crate::esp::cancelled() {
            return Err(AppError::Cancelled);
        }
        std::thread::sleep(poll);
    }
    if mount.exists() {
        return Err(AppError::uf2_delivery(format!(
            "UF2 was synchronized, but {mount_label} did not disappear within 20 seconds"
        )));
    }
    Ok(())
}

fn select_mount(
    board: &BoardCatalogEntry,
    candidates: Vec<PathBuf>,
    mount_override: Option<&Path>,
) -> Result<PathBuf, AppError> {
    if let Some(mount) = mount_override {
        return validate_mount(board, mount);
    }
    let mount_label = uf2_build(board)?.mount_label.as_str();
    match candidates.as_slice() {
        [] => Err(AppError::uf2_mount(format!(
            "{mount_label} is not mounted; double-tap RESET and wait for the drive"
        ))),
        [mount] => validate_mount(board, mount),
        _ => Err(AppError::uf2_mount(format!(
            "multiple identifiable {} UF2 bootloader drives were found ({}); disconnect or unmount the extras, then retry",
            board.display_name,
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

#[cfg(unix)]
fn sync_mount_directory(mount: &Path) -> std::io::Result<()> {
    std::fs::File::open(mount).and_then(|directory| directory.sync_all())
}

#[cfg(windows)]
fn sync_mount_directory(_mount: &Path) -> std::io::Result<()> {
    // File::sync_all above flushes the copied UF2. Windows does not permit opening a directory
    // with std::fs::File, so there is no additional portable directory handle to flush.
    Ok(())
}

fn uf2_build(board: &BoardCatalogEntry) -> Result<&Uf2Build, AppError> {
    match &board.build {
        BoardBuild::Uf2(build) => Ok(build),
        BoardBuild::Esp(_) => Err(AppError::unsupported_operation(
            "ESP board cannot use the UF2 bootloader engine",
        )),
    }
}

/// Mounts that identify as the selected board, and only that board.
fn detect_mounts_for(board: &BoardCatalogEntry) -> Result<Vec<PathBuf>, AppError> {
    Ok(scan(&[uf2_build(board)?.board_id_prefix.as_str()]))
}

/// Every cataloged UF2 bootloader, for diagnostics that have no selected board.
pub(crate) fn detect_any_uf2_mounts(catalog: &BoardCatalog) -> Vec<PathBuf> {
    let prefixes = catalog
        .boards
        .iter()
        .filter_map(|board| match &board.build {
            BoardBuild::Uf2(build) => Some(build.board_id_prefix.as_str()),
            BoardBuild::Esp(_) => None,
        })
        .collect::<Vec<_>>();
    scan(&prefixes)
}

pub(crate) fn doctor_mount_from(
    candidates: Vec<PathBuf>,
    board: &BoardCatalogEntry,
) -> Result<PathBuf, AppError> {
    let prefix = uf2_build(board)?.board_id_prefix.clone();
    let candidates = candidates
        .into_iter()
        .filter(|path| mount_identity_matches(path, &[prefix.as_str()]))
        .collect();
    select_mount(board, candidates, None)
}

fn scan(prefixes: &[&str]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("HOPSPOT_TECHOBOOT") {
        push_if_identified(&mut candidates, PathBuf::from(path), prefixes);
    }
    for root in ["/Volumes", "/mnt", "/media", "/run/media"] {
        scan_root(Path::new(root), 2, prefixes, &mut candidates);
    }
    #[cfg(windows)]
    for letter in b'D'..=b'Z' {
        push_if_identified(
            &mut candidates,
            PathBuf::from(format!("{}:\\", letter as char)),
            prefixes,
        );
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn scan_root(root: &Path, depth: usize, prefixes: &[&str], candidates: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            push_if_identified(candidates, path.clone(), prefixes);
            scan_root(&path, depth - 1, prefixes, candidates);
        }
    }
}

fn push_if_identified(candidates: &mut Vec<PathBuf>, path: PathBuf, prefixes: &[&str]) {
    if mount_identity_matches(&path, prefixes) {
        candidates.push(path);
    }
}

fn validate_mount(board: &BoardCatalogEntry, path: &Path) -> Result<PathBuf, AppError> {
    if mount_identity_matches(path, &[uf2_build(board)?.board_id_prefix.as_str()]) {
        Ok(path.to_path_buf())
    } else {
        Err(AppError::uf2_mount(format!(
            "{} does not contain a {} Board-ID in INFO_UF2.TXT",
            path.display(),
            board.display_name
        )))
    }
}

fn mount_identity_matches(path: &Path, prefixes: &[&str]) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Ok(info) = fs::read_to_string(path.join("INFO_UF2.TXT")) else {
        return false;
    };
    info.lines().any(|line| board_id_matches(line, prefixes))
}

fn board_id_matches(line: &str, prefixes: &[&str]) -> bool {
    let Some((field, value)) = line.split_once(':') else {
        return false;
    };
    let field = field
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    if field != "boardid" {
        return false;
    }
    let board_id = Uf2BoardIdPrefix::normalize(value);
    prefixes.iter().any(|prefix| {
        // Bootloaders append a hardware revision to the cataloged prefix. Requiring a
        // non-empty revision keeps a bare prefix, a generic UF2 drive, and a coincidental
        // mount label from passing as identity.
        board_id.strip_prefix(prefix).is_some_and(|revision| {
            !revision.is_empty()
                && revision
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '.')
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn t_echo_board() -> BoardCatalogEntry {
        prns_flash_manifest::board_catalog()
            .expect("catalog")
            .board("t-echo")
            .expect("t-echo")
            .clone()
    }

    fn temporary_mount(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("hopspot-flash-{name}-{nonce}"))
    }

    #[test]
    fn absent_override_is_not_accepted() {
        assert!(validate_mount(&t_echo_board(), Path::new("/definitely/not/a/techo/mount")).is_err());
    }

    #[test]
    fn uf2_info_identifies_a_fake_mount() {
        let mount = temporary_mount("mount");
        fs::create_dir(&mount).expect("create mount");
        fs::write(
            mount.join("INFO_UF2.TXT"),
            "UF2 Bootloader 0.6.1\nModel: LilyGo T-Echo\nBoard-ID: nRF52840-TEcho-v1\n",
        )
        .expect("write info");
        assert_eq!(
            doctor_mount_from(vec![mount.clone()], &t_echo_board()).expect("doctor fake mount"),
            mount
        );
        assert_eq!(
            fs::read_dir(&mount).expect("read fake mount").count(),
            1,
            "doctor must not copy or alter UF2 files"
        );
        fs::remove_dir_all(&mount).expect("remove fake mount");
    }

    #[test]
    fn mount_label_or_generic_uf2_info_cannot_impersonate_a_t_echo() {
        let labelled = temporary_mount("TECHOBOOT").join("TECHOBOOT");
        fs::create_dir_all(&labelled).expect("create labelled mount");
        assert!(validate_mount(&t_echo_board(), &labelled).is_err());
        fs::write(
            labelled.join("INFO_UF2.TXT"),
            "Model: LilyGo T-Echo\nBoard-ID: nRF52840-Feather-revD\n",
        )
        .expect("write generic UF2 identity");
        assert!(validate_mount(&t_echo_board(), &labelled).is_err());
        fs::remove_dir_all(labelled.parent().expect("temporary parent"))
            .expect("remove labelled mount");
    }

    #[test]
    fn board_id_spelling_and_later_revisions_are_supported() {
        let mount = temporary_mount("board-id-variant");
        fs::create_dir(&mount).expect("create mount");
        fs::write(
            mount.join("INFO_UF2.TXT"),
            "Board ID: nRF52840_TEcho_v2.1\n",
        )
        .expect("write identity");
        assert_eq!(
            validate_mount(&t_echo_board(), &mount).expect("T-Echo identity"),
            mount
        );
        fs::remove_dir_all(&mount).expect("remove mount");
    }

    #[test]
    fn a_cataloged_prefix_does_not_answer_for_another_board() {
        let line = "Board-ID: nRF52840-TEcho-v1";
        assert!(board_id_matches(line, &["nrf52840-techo-v"]));
        assert!(!board_id_matches(line, &["nrf52840-heltec-t114-v"]));
        assert!(board_id_matches(
            line,
            &["nrf52840-heltec-t114-v", "nrf52840-techo-v"]
        ));

        let mount = temporary_mount("cross-board");
        fs::create_dir(&mount).expect("create mount");
        fs::write(mount.join("INFO_UF2.TXT"), "Board-ID: nRF52840-TEcho-v1\n")
            .expect("write identity");
        assert!(!mount_identity_matches(&mount, &["nrf52840-heltec-t114-v"]));
        assert!(mount_identity_matches(&mount, &["nrf52840-techo-v"]));
        fs::remove_dir_all(&mount).expect("remove mount");
    }

    #[test]
    fn zero_and_multiple_mounts_are_explicit_failures() {
        assert!(matches!(
            doctor_mount_from(Vec::new(), &t_echo_board()),
            Err(AppError::Preflight(_))
        ));
        let first = temporary_mount("multiple-a");
        let second = temporary_mount("multiple-b");
        for mount in [&first, &second] {
            fs::create_dir(mount).expect("create mount");
            fs::write(mount.join("INFO_UF2.TXT"), "Board-ID: nRF52840-TEcho-v1\n")
                .expect("write identity");
        }
        let error = doctor_mount_from(vec![first.clone(), second.clone()], &t_echo_board())
            .expect_err("multiple mounts must be explicit");
        assert!(matches!(error, AppError::Preflight(_)));
        let message = error.to_string();
        assert!(message.contains("disconnect or unmount"));
        assert!(!message.contains("--mount"));
        fs::remove_dir_all(first).expect("remove first mount");
        fs::remove_dir_all(second).expect("remove second mount");
    }

    #[test]
    fn fake_uf2_copy_is_written_and_synchronized() {
        let mount = temporary_mount("copy");
        fs::create_dir(&mount).expect("create mount");
        let destination = mount.join("firmware.uf2");
        copy_uf2(
            &destination,
            &mount,
            b"signed uf2 bytes",
            "t-echo",
            "TECHOBOOT",
            Reporter::json_lines(),
        )
        .expect("copy fake UF2");
        assert_eq!(
            fs::read(destination).expect("read copied UF2"),
            b"signed uf2 bytes"
        );
        fs::remove_dir_all(mount).expect("remove fake mount");
    }

    #[test]
    fn fake_reboot_disappearance_and_timeout_are_distinct() {
        let disappearing = temporary_mount("disappearing");
        fs::create_dir(&disappearing).expect("create disappearing mount");
        let remover = disappearing.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            fs::remove_dir(remover).expect("remove disappearing mount");
        });
        wait_for_reboot(
            &disappearing,
            "TECHOBOOT",
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .expect("detect disappearance");
        thread.join().expect("join remover");

        let stuck = temporary_mount("stuck");
        fs::create_dir(&stuck).expect("create stuck mount");
        assert!(matches!(
            wait_for_reboot(&stuck, "TECHOBOOT", Duration::ZERO, Duration::from_millis(1)),
            Err(AppError::WriteVerifyReset(_))
        ));
        fs::remove_dir(stuck).expect("remove stuck mount");
    }

    #[test]
    fn reboot_after_sync_interruption_is_success_only_when_mount_disappears() {
        let disappearing = temporary_mount("sync-interrupted-disappearing");
        fs::create_dir(&disappearing).expect("create disappearing mount");
        let remover = disappearing.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            fs::remove_dir(remover).expect("remove disappearing mount");
        });
        let outcome = confirm_reboot_after_synchronization_interruption(
            &disappearing,
            "t-echo",
            "TECHOBOOT",
            Reporter::json_lines(),
            "UF2 flush/sync failed",
            std::io::Error::other("bootloader disconnected"),
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .expect("reboot confirms delivery");
        assert!(matches!(outcome, Uf2CopyOutcome::RebootObserved));
        thread.join().expect("join remover");

        let stuck = temporary_mount("sync-interrupted-stuck");
        fs::create_dir(&stuck).expect("create stuck mount");
        let result = confirm_reboot_after_synchronization_interruption(
            &stuck,
            "t-echo",
            "TECHOBOOT",
            Reporter::json_lines(),
            "UF2 flush/sync failed",
            std::io::Error::other("storage failure"),
            Duration::ZERO,
            Duration::from_millis(1),
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("persistent mount does not prove reboot"),
        };
        assert!(error.to_string().contains("storage failure"));
        fs::remove_dir(stuck).expect("remove stuck mount");
    }
}
