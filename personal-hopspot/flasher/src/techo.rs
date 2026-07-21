use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use prns_flash_manifest::BoardCatalogEntry;

use crate::error::AppError;
use crate::events::Reporter;
use crate::release::PreparedPart;

const REBOOT_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn flash(
    board: &BoardCatalogEntry,
    parts: &[PreparedPart],
    mount_override: Option<&Path>,
    reporter: Reporter,
) -> Result<(), AppError> {
    let part = match parts {
        [part] => part,
        _ => {
            return Err(AppError::trust(
                "T-Echo release must contain exactly one UF2",
            ))
        }
    };
    let mount = select_mount(detect_mounts(), mount_override)?;

    let destination = mount.join("prns-hopspot.uf2");
    reporter.phase(
        "writing",
        Some(&board.slug),
        &format!("Copying verified UF2 to {}…", destination.display()),
    );
    copy_uf2(&destination, &mount, &part.bytes, &board.slug, reporter)?;

    reporter.phase(
        "resetting",
        Some(&board.slug),
        "Waiting for TECHOBOOT to disappear as the device reboots…",
    );
    wait_for_reboot(&mount, REBOOT_TIMEOUT, Duration::from_millis(200))?;
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
        .map_err(|error| AppError::flash(format!("could not create UF2 on TECHOBOOT: {error}")))?;
    let mut written = 0usize;
    for chunk in bytes.chunks(64 * 1024) {
        if crate::esp::cancelled() {
            drop(output);
            let _ = fs::remove_file(destination);
            return Err(AppError::Cancelled);
        }
        output
            .write_all(chunk)
            .map_err(|error| AppError::flash(format!("UF2 copy failed: {error}")))?;
        written += chunk.len();
        reporter.progress(
            "writing",
            Some(board_slug),
            written as u64,
            bytes.len() as u64,
        );
    }
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|error| AppError::flash(format!("UF2 flush/sync failed: {error}")))?;
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
        return Err(AppError::flash(
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
        [] => Err(AppError::preflight(
            "TECHOBOOT is not mounted; double-tap RESET and wait for the drive",
        )),
        [mount] => Ok(mount.clone()),
        _ => Err(AppError::preflight(format!(
            "multiple UF2 bootloader drives were found ({}); use --mount",
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
    File::open(mount)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| AppError::flash(format!("TECHOBOOT directory sync failed: {error}")))
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
        Err(AppError::preflight(format!(
            "{} is not an identifiable TECHOBOOT drive",
            path.display()
        )))
    }
}

fn is_techo_mount(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("TECHOBOOT"))
    {
        return true;
    }
    let info_path = path.join("INFO_UF2.TXT");
    let Ok(mut file) = File::open(info_path) else {
        return false;
    };
    let mut info = String::new();
    if file.read_to_string(&mut info).is_err() {
        return false;
    }
    let lower = info.to_ascii_lowercase();
    lower.contains("t-echo") || lower.contains("techo")
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
        fs::write(mount.join("INFO_UF2.TXT"), "Board-ID: LilyGO T-Echo\n").expect("write info");
        assert_eq!(
            select_mount(vec![mount.clone()], None).expect("select fake mount"),
            mount
        );
        fs::remove_dir_all(&mount).expect("remove fake mount");
    }

    #[test]
    fn zero_and_multiple_mounts_are_explicit_failures() {
        assert!(matches!(
            select_mount(Vec::new(), None),
            Err(AppError::Preflight(_))
        ));
        assert!(matches!(
            select_mount(vec![PathBuf::from("a"), PathBuf::from("b")], None),
            Err(AppError::Preflight(_))
        ));
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
            Reporter::new(true),
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
            Err(AppError::Flash(_))
        ));
        fs::remove_dir(stuck).expect("remove stuck mount");
    }
}
