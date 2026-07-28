use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use prns_flash_manifest::BoardCatalogEntry;

use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedUf2Target;

const REBOOT_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn flash(
    board: &BoardCatalogEntry,
    target: &PreparedUf2Target,
    mount_override: Option<&Path>,
    reporter: Reporter,
) -> Result<(), AppError> {
    let mount = select_mount(detect_mounts(), mount_override)?;

    let destination = mount.join("prns-hopspot.uf2");
    reporter.phase(
        Phase::Writing,
        Some(&board.slug),
        &format!("Copying verified UF2 to {}…", destination.display()),
    );
    copy_uf2(
        &destination,
        &mount,
        target.part().bytes(),
        &board.slug,
        reporter,
    )?;

    reporter.phase(
        Phase::Resetting,
        Some(&board.slug),
        "Waiting for TECHOBOOT to disappear as the device reboots…",
    );
    wait_for_reboot(&mount, REBOOT_TIMEOUT, Duration::from_millis(200))?;
    if crate::esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    reporter.success(
        &board.slug,
        "Verified UF2 delivered and the T-Echo bootloader drive rebooted.",
    );
    Ok(())
}

fn copy_uf2(
    destination: &Path,
    mount: &Path,
    bytes: &[u8],
    board_slug: &str,
    reporter: Reporter,
) -> Result<(), AppError> {
    let mut output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)
        .map_err(|error| {
            AppError::uf2_delivery(format!("could not create UF2 on TECHOBOOT: {error}"))
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
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|error| AppError::uf2_delivery(format!("UF2 flush/sync failed: {error}")))?;
    drop(output);
    sync_mount_directory(mount)
}

fn wait_for_reboot(mount: &Path, timeout: Duration, poll: Duration) -> Result<(), AppError> {
    let deadline = Instant::now() + timeout;
    while mount.exists() && Instant::now() < deadline {
        if crate::esp::cancelled() {
            return Err(AppError::Cancelled);
        }
        std::thread::sleep(poll);
    }
    if mount.exists() {
        return Err(AppError::uf2_delivery(
            "UF2 was synchronized, but TECHOBOOT did not disappear within 20 seconds",
        ));
    }
    Ok(())
}

fn select_mount(
    candidates: Vec<PathBuf>,
    mount_override: Option<&Path>,
) -> Result<PathBuf, AppError> {
    if let Some(mount) = mount_override {
        return validate_mount(mount);
    }
    match candidates.as_slice() {
        [] => Err(AppError::uf2_mount(
            "TECHOBOOT is not mounted; double-tap RESET and wait for the drive",
        )),
        [mount] => validate_mount(mount),
        _ => Err(AppError::uf2_mount(format!(
            "multiple identifiable T-Echo UF2 bootloader drives were found ({}); disconnect or unmount the extras, then retry",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

#[cfg(unix)]
fn sync_mount_directory(mount: &Path) -> Result<(), AppError> {
    std::fs::File::open(mount)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            AppError::uf2_delivery(format!("TECHOBOOT directory sync failed: {error}"))
        })
}

#[cfg(windows)]
fn sync_mount_directory(_mount: &Path) -> Result<(), AppError> {
    // File::sync_all above flushes the copied UF2. Windows does not permit opening a directory
    // with std::fs::File, so there is no additional portable directory handle to flush.
    Ok(())
}

pub(crate) fn detect_mounts() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("HOPSPOT_TECHOBOOT") {
        push_if_techo(&mut candidates, PathBuf::from(path));
    }
    for root in ["/Volumes", "/mnt", "/media", "/run/media"] {
        scan_root(Path::new(root), 2, &mut candidates);
    }
    #[cfg(windows)]
    for letter in b'D'..=b'Z' {
        push_if_techo(
            &mut candidates,
            PathBuf::from(format!("{}:\\", letter as char)),
        );
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

pub(crate) fn doctor_mount_from(candidates: Vec<PathBuf>) -> Result<PathBuf, AppError> {
    select_mount(candidates, None)
}

fn scan_root(root: &Path, depth: usize, candidates: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            push_if_techo(candidates, path.clone());
            scan_root(&path, depth - 1, candidates);
        }
    }
}

fn push_if_techo(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if is_techo_mount(&path) {
        candidates.push(path);
    }
}

fn validate_mount(path: &Path) -> Result<PathBuf, AppError> {
    if is_techo_mount(path) {
        Ok(path.to_path_buf())
    } else {
        Err(AppError::uf2_mount(format!(
            "{} does not contain a T-Echo Board-ID in INFO_UF2.TXT",
            path.display()
        )))
    }
}

fn is_techo_mount(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Ok(info) = fs::read_to_string(path.join("INFO_UF2.TXT")) else {
        return false;
    };
    info.lines().any(|line| {
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

        // LilyGO's bootloader identifies this board as
        // `nRF52840-TEcho-v1`. Accept later hardware revisions while keeping
        // the model portion exact; a generic UF2 drive or a coincidental mount
        // label is not sufficient identity.
        let board_id = value.trim().to_ascii_lowercase().replace('_', "-");
        let Some(revision) = board_id.strip_prefix("nrf52840-techo-v") else {
            return false;
        };
        !revision.is_empty()
            && revision
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '.')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_mount(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("hopspot-flash-{name}-{nonce}"))
    }

    #[test]
    fn absent_override_is_not_accepted() {
        assert!(validate_mount(Path::new("/definitely/not/a/techo/mount")).is_err());
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
            doctor_mount_from(vec![mount.clone()]).expect("doctor fake mount"),
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
        assert!(validate_mount(&labelled).is_err());
        fs::write(
            labelled.join("INFO_UF2.TXT"),
            "Model: LilyGo T-Echo\nBoard-ID: nRF52840-Feather-revD\n",
        )
        .expect("write generic UF2 identity");
        assert!(validate_mount(&labelled).is_err());
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
        assert_eq!(validate_mount(&mount).expect("T-Echo identity"), mount);
        fs::remove_dir_all(&mount).expect("remove mount");
    }

    #[test]
    fn zero_and_multiple_mounts_are_explicit_failures() {
        assert!(matches!(
            doctor_mount_from(Vec::new()),
            Err(AppError::Preflight(_))
        ));
        let first = temporary_mount("multiple-a");
        let second = temporary_mount("multiple-b");
        for mount in [&first, &second] {
            fs::create_dir(mount).expect("create mount");
            fs::write(mount.join("INFO_UF2.TXT"), "Board-ID: nRF52840-TEcho-v1\n")
                .expect("write identity");
        }
        let error = doctor_mount_from(vec![first.clone(), second.clone()])
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
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .expect("detect disappearance");
        thread.join().expect("join remover");

        let stuck = temporary_mount("stuck");
        fs::create_dir(&stuck).expect("create stuck mount");
        assert!(matches!(
            wait_for_reboot(&stuck, Duration::ZERO, Duration::from_millis(1)),
            Err(AppError::WriteVerifyReset(_))
        ));
        fs::remove_dir(stuck).expect("remove stuck mount");
    }
}
