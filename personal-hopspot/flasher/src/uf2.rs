use std::collections::HashSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nusb::{DeviceId, MaybeFuture};
use prns_flash_manifest::{
    BoardBuild, BoardCatalog, BoardCatalogEntry, SoftdeviceIdentity, Uf2ApplicationUsb,
    Uf2BoardIdMatch, Uf2BootloaderIdentity, Uf2MountLabel,
};

use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedUf2Target;

const REBOOT_TIMEOUT: Duration = Duration::from_secs(20);
const APPLICATION_ENUMERATION_TIMEOUT: Duration = Duration::from_secs(20);
const PRNS_USB_VENDOR_ID: u16 = 0x1209;
const PRNS_USB_PRODUCT_ID: u16 = 0x0001;
const INFO_UF2_READ_LIMIT: u64 = 4097;

#[derive(Clone, Debug)]
pub(crate) struct DetectedUf2Device {
    mount: PathBuf,
    identity: Uf2BootloaderIdentity,
    compatibility_variant: String,
}

impl DetectedUf2Device {
    pub(crate) fn mount(&self) -> &Path {
        &self.mount
    }

    pub(crate) fn identity(&self) -> &Uf2BootloaderIdentity {
        &self.identity
    }

    pub(crate) fn softdevice(&self) -> &SoftdeviceIdentity {
        self.identity.softdevice()
    }

    pub(crate) fn compatibility_variant(&self) -> &str {
        &self.compatibility_variant
    }
}

enum Uf2CopyOutcome {
    Synchronized,
    RebootObserved,
}

struct CatalogedUf2Board<'a> {
    entry: &'a BoardCatalogEntry,
    mount_label: Uf2MountLabel,
    board_id_match: Uf2BoardIdMatch,
    application_usb: &'a Uf2ApplicationUsb,
}

impl<'a> CatalogedUf2Board<'a> {
    fn try_from_entry(entry: &'a BoardCatalogEntry) -> Result<Self, AppError> {
        match &entry.build {
            BoardBuild::Uf2(build) => Ok(Self {
                entry,
                mount_label: Uf2MountLabel::parse(build.mount_label.clone())
                    .map_err(|error| AppError::trust_catalog(error.to_string()))?,
                board_id_match: build
                    .board_identity
                    .validated()
                    .map_err(|error| AppError::trust_catalog(error.to_string()))?,
                application_usb: &build.application_usb,
            }),
            BoardBuild::Esp(_) => Err(AppError::unsupported_operation(
                "ESP board cannot use the UF2 bootloader engine",
            )),
            BoardBuild::NrfSerialDfu(_) => Err(AppError::unsupported_operation(
                "Nordic serial DFU board cannot use the direct UF2 engine",
            )),
        }
    }

    fn slug(&self) -> &str {
        &self.entry.slug
    }

    fn display_name(&self) -> &str {
        &self.entry.display_name
    }

    fn mount_label(&self) -> &str {
        self.mount_label.as_str()
    }

    fn board_id_match(&self) -> &Uf2BoardIdMatch {
        &self.board_id_match
    }
}

pub(crate) fn flash(
    entry: &BoardCatalogEntry,
    target: &PreparedUf2Target,
    device: DetectedUf2Device,
    reporter: Reporter,
) -> Result<(), AppError> {
    let board = CatalogedUf2Board::try_from_entry(entry)?;
    if target.compatibility().softdevice() != device.softdevice() {
        return Err(AppError::trust_identity(
            "prepared UF2 compatibility does not match the detected bootloader foundation",
        ));
    }
    let mount = device.mount;
    let baseline_usb = matching_prns_application_usb_ids(board.application_usb)?;

    let destination = mount.join("prns-hopspot.uf2");
    reporter.phase(
        Phase::Writing,
        Some(board.slug()),
        &format!("Copying verified UF2 to {}…", destination.display()),
    );
    let copy_outcome = copy_uf2(
        &destination,
        &mount,
        target.part().bytes(),
        &board,
        reporter,
    )?;

    if matches!(copy_outcome, Uf2CopyOutcome::Synchronized) {
        reporter.phase(
            Phase::Resetting,
            Some(board.slug()),
            &format!(
                "Waiting for {} to disappear as the device reboots…",
                board.mount_label()
            ),
        );
        wait_for_reboot(&mount, &board, REBOOT_TIMEOUT, Duration::from_millis(200))?;
    }
    if crate::esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    wait_for_application_usb(
        &board,
        &baseline_usb,
        APPLICATION_ENUMERATION_TIMEOUT,
        Duration::from_millis(200),
    )?;
    reporter.success(
        board.slug(),
        &format!(
            "Verified UF2 delivered; the {} bootloader drive rebooted and Personal Hopspot USB enumerated.",
            board.display_name()
        ),
    );
    Ok(())
}

pub(crate) fn detect_device(
    entry: &BoardCatalogEntry,
    mount_override: Option<&Path>,
) -> Result<DetectedUf2Device, AppError> {
    let board = CatalogedUf2Board::try_from_entry(entry)?;
    let mount = select_mount(&board, detect_mounts_for(&board), mount_override)?;
    let identity = read_identity(&mount)?;
    if !identity.matches_board(board.board_id_match()) {
        return Err(AppError::device_identity(format!(
            "{} reports Board-ID {:?}, not {}",
            mount.display(),
            identity.board_id(),
            board.display_name()
        )));
    }
    let BoardBuild::Uf2(build) = &entry.build else {
        return Err(AppError::unsupported_operation(
            "ESP board cannot use the UF2 bootloader engine",
        ));
    };
    let variant = build
        .variants
        .iter()
        .find(|variant| {
            variant.softdevice_family == identity.softdevice().family().as_str()
                && variant.softdevice_version == identity.softdevice().version().as_str()
        })
        .ok_or_else(|| {
            AppError::device_identity(format!(
                "{} reports unsupported SoftDevice {}",
                mount.display(),
                identity.softdevice()
            ))
        })?;
    Ok(DetectedUf2Device {
        mount,
        compatibility_variant: format!(
            "{}-{}-fwid-{}",
            variant.softdevice_family, variant.softdevice_version, variant.fwid
        ),
        identity,
    })
}

fn copy_uf2(
    destination: &Path,
    mount: &Path,
    bytes: &[u8],
    board: &CatalogedUf2Board<'_>,
    reporter: Reporter,
) -> Result<Uf2CopyOutcome, AppError> {
    let mut output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)
        .map_err(|error| {
            AppError::uf2_delivery(format!(
                "could not create UF2 on {}: {error}",
                board.mount_label()
            ))
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
            Some(board.slug()),
            written as u64,
            bytes.len() as u64,
        );
    }
    let file_sync = output.flush().and_then(|_| output.sync_all());
    drop(output);
    if let Err(error) = file_sync {
        return confirm_reboot_after_synchronization_interruption(
            mount,
            board,
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
            board,
            reporter,
            &format!("{} directory sync failed", board.mount_label()),
            error,
            REBOOT_TIMEOUT,
            Duration::from_millis(200),
        );
    }
    Ok(Uf2CopyOutcome::Synchronized)
}

fn confirm_reboot_after_synchronization_interruption(
    mount: &Path,
    board: &CatalogedUf2Board<'_>,
    reporter: Reporter,
    operation: &str,
    error: std::io::Error,
    timeout: Duration,
    poll: Duration,
) -> Result<Uf2CopyOutcome, AppError> {
    reporter.phase(
        Phase::Resetting,
        Some(board.slug()),
        &format!(
            "UF2 synchronization was interrupted; checking whether {} rebooted…",
            board.mount_label()
        ),
    );
    match wait_for_reboot(mount, board, timeout, poll) {
        Ok(()) => Ok(Uf2CopyOutcome::RebootObserved),
        Err(AppError::Cancelled) => Err(AppError::Cancelled),
        Err(_) => Err(AppError::uf2_delivery(format!("{operation}: {error}"))),
    }
}

fn wait_for_reboot(
    mount: &Path,
    board: &CatalogedUf2Board<'_>,
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
            "UF2 was synchronized, but {} did not disappear within {timeout:?}",
            board.mount_label(),
        )));
    }
    Ok(())
}

fn matching_prns_application_usb_ids(
    expected: &Uf2ApplicationUsb,
) -> Result<HashSet<DeviceId>, AppError> {
    nusb::list_devices()
        .wait()
        .map_err(|error| {
            AppError::host_preflight(format!("could not enumerate USB devices: {error}"))
        })
        .map(|devices| {
            devices
                .filter(|device| {
                    device.vendor_id() == PRNS_USB_VENDOR_ID
                        && device.product_id() == PRNS_USB_PRODUCT_ID
                        && device.manufacturer_string() == Some(expected.manufacturer.as_str())
                        && device.product_string() == Some(expected.product.as_str())
                        && device.serial_number() == Some(expected.serial_number.as_str())
                })
                .map(|device| device.id())
                .collect()
        })
}

fn wait_for_application_usb(
    board: &CatalogedUf2Board<'_>,
    baseline: &HashSet<DeviceId>,
    timeout: Duration,
    poll: Duration,
) -> Result<(), AppError> {
    let deadline = Instant::now() + timeout;
    loop {
        if crate::esp::cancelled() {
            return Err(AppError::Cancelled);
        }
        let current =
            matching_prns_application_usb_ids(board.application_usb).map_err(|error| {
                AppError::verify(format!(
                "UF2 delivery completed, but application USB verification is incomplete: {error}"
            ))
            })?;
        let newly_enumerated = current.difference(baseline).count();
        match (newly_enumerated, current.len()) {
            (1, 1) => return Ok(()),
            (1.., 2..) => {
                return Err(AppError::verify(format!(
                    "UF2 delivery completed, but multiple indistinguishable {:?} USB devices enumerated; application verification is incomplete",
                    board.application_usb.product
                )));
            }
            _ if Instant::now() >= deadline => {
                return Err(AppError::verify(format!(
                    "UF2 delivery completed, but no newly enumerated {:?} USB identity appeared within {timeout:?}; application verification is incomplete",
                    board.application_usb.product
                )));
            }
            _ => std::thread::sleep(poll),
        }
    }
}

fn select_mount(
    board: &CatalogedUf2Board<'_>,
    candidates: Vec<PathBuf>,
    mount_override: Option<&Path>,
) -> Result<PathBuf, AppError> {
    if let Some(mount) = mount_override {
        if mount.is_dir() && mount.join("INFO_UF2.TXT").is_file() {
            return Ok(mount.to_path_buf());
        }
        return Err(AppError::uf2_mount(format!(
            "{} is not a mounted UF2 bootloader directory",
            mount.display()
        )));
    }
    match candidates.as_slice() {
        [] => Err(AppError::uf2_mount(format!(
            "{} is not mounted; double-tap RESET and wait for the drive",
            board.mount_label()
        ))),
        [mount] => Ok(mount.clone()),
        _ => Err(AppError::uf2_mount(format!(
            "multiple identifiable {} UF2 bootloader drives were found ({}); disconnect or unmount the extras, then retry",
            board.display_name(),
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

fn detect_mounts_for(board: &CatalogedUf2Board<'_>) -> Vec<PathBuf> {
    scan(
        std::slice::from_ref(board.board_id_match()),
        Some(board.mount_label()),
    )
}

pub(crate) fn detect_any_uf2_mounts(catalog: &BoardCatalog) -> Vec<PathBuf> {
    let board_id_matches = catalog
        .boards
        .iter()
        .filter_map(|board| match &board.build {
            BoardBuild::Uf2(build) => build.board_identity.validated().ok(),
            BoardBuild::Esp(_) => None,
            BoardBuild::NrfSerialDfu(build) => build.recovery.board_identity.validated().ok(),
        })
        .collect::<Vec<_>>();
    scan(&board_id_matches, None)
}

fn scan(board_id_matches: &[Uf2BoardIdMatch], mount_label: Option<&str>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("HOPSPOT_TECHOBOOT") {
        push_if_identified(
            &mut candidates,
            PathBuf::from(path),
            board_id_matches,
            mount_label,
        );
    }
    for root in ["/Volumes", "/mnt", "/media", "/run/media"] {
        scan_root(
            Path::new(root),
            2,
            board_id_matches,
            mount_label,
            &mut candidates,
        );
    }
    #[cfg(windows)]
    for letter in b'D'..=b'Z' {
        push_if_identified(
            &mut candidates,
            PathBuf::from(format!("{}:\\", letter as char)),
            board_id_matches,
            mount_label,
        );
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn scan_root(
    root: &Path,
    depth: usize,
    board_id_matches: &[Uf2BoardIdMatch],
    mount_label: Option<&str>,
    candidates: &mut Vec<PathBuf>,
) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            push_if_identified(candidates, path.clone(), board_id_matches, mount_label);
            scan_root(&path, depth - 1, board_id_matches, mount_label, candidates);
        }
    }
}

fn push_if_identified(
    candidates: &mut Vec<PathBuf>,
    path: PathBuf,
    board_id_matches: &[Uf2BoardIdMatch],
    mount_label: Option<&str>,
) {
    let labelled = mount_label.is_some_and(|label| {
        path.file_name().and_then(|name| name.to_str()) == Some(label)
            && path.join("INFO_UF2.TXT").is_file()
    });
    if labelled || mount_identity_matches(&path, board_id_matches) {
        candidates.push(path);
    }
}

fn mount_identity_matches(path: &Path, board_id_matches: &[Uf2BoardIdMatch]) -> bool {
    if !path.is_dir() {
        return false;
    }
    let Ok(identity) = read_identity(path) else {
        return false;
    };
    board_id_matches
        .iter()
        .any(|board_id_match| identity.matches_board(board_id_match))
}

fn read_identity(path: &Path) -> Result<Uf2BootloaderIdentity, AppError> {
    if !path.is_dir() {
        return Err(AppError::uf2_mount(format!(
            "{} is not a mounted directory",
            path.display()
        )));
    }
    let info_path = path.join("INFO_UF2.TXT");
    let file = File::open(&info_path).map_err(|error| {
        AppError::device_identity(format!("could not read {}: {error}", info_path.display()))
    })?;
    let mut bytes = Vec::new();
    file.take(INFO_UF2_READ_LIMIT)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            AppError::device_identity(format!("could not read {}: {error}", info_path.display()))
        })?;
    Uf2BootloaderIdentity::parse(&bytes)
        .map_err(|error| AppError::device_identity(format!("{}: {error}", info_path.display())))
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

    fn cataloged_uf2(entry: &BoardCatalogEntry) -> CatalogedUf2Board<'_> {
        CatalogedUf2Board::try_from_entry(entry).expect("cataloged UF2 board")
    }

    #[test]
    fn only_valid_uf2_entries_can_enter_the_uf2_engine() {
        let catalog = prns_flash_manifest::board_catalog().expect("catalog");
        let esp = catalog.board("heltec-v4").expect("ESP board");
        assert!(CatalogedUf2Board::try_from_entry(esp).is_err());

        let mut malformed = t_echo_board();
        let BoardBuild::Uf2(build) = &mut malformed.build else {
            panic!("expected UF2 build");
        };
        build.mount_label = "../TECHOBOOT".to_string();
        assert!(CatalogedUf2Board::try_from_entry(&malformed).is_err());
    }

    fn temporary_mount(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("hopspot-flash-{name}-{nonce}"))
    }

    fn info(board_id: &str, softdevice_version: &str) -> String {
        format!(
            "UF2 Bootloader 0.6.1\nModel: LilyGo T-Echo\nBoard-ID: {board_id}\nSoftDevice: S140 version {softdevice_version}\n"
        )
    }

    #[test]
    fn absent_override_is_not_accepted() {
        let entry = t_echo_board();
        assert!(detect_device(&entry, Some(Path::new("/definitely/not/a/techo/mount"))).is_err());
    }

    #[test]
    fn uf2_info_identifies_a_fake_mount() {
        let mount = temporary_mount("mount");
        fs::create_dir(&mount).expect("create mount");
        fs::write(
            mount.join("INFO_UF2.TXT"),
            info("nRF52840-TEcho-v1", "7.3.0"),
        )
        .expect("write info");
        assert_eq!(
            detect_device(&t_echo_board(), Some(&mount))
                .expect("doctor fake mount")
                .mount(),
            mount.as_path()
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
        let entry = t_echo_board();
        let labelled = temporary_mount("TECHOBOOT").join("TECHOBOOT");
        fs::create_dir_all(&labelled).expect("create labelled mount");
        assert!(detect_device(&entry, Some(&labelled)).is_err());
        fs::write(
            labelled.join("INFO_UF2.TXT"),
            info("nRF52840-Feather-revD", "7.3.0"),
        )
        .expect("write generic UF2 identity");
        assert!(detect_device(&entry, Some(&labelled)).is_err());
        fs::remove_dir_all(labelled.parent().expect("temporary parent"))
            .expect("remove labelled mount");
    }

    #[test]
    fn board_id_spelling_and_later_revisions_are_supported() {
        let entry = t_echo_board();
        let mount = temporary_mount("board-id-variant");
        fs::create_dir(&mount).expect("create mount");
        fs::write(
            mount.join("INFO_UF2.TXT"),
            "UF2 Bootloader 0.6.1\nBoard ID: nRF52840_TEcho_v2.1\nSoftDevice: S140 version 7.3.0\n",
        )
        .expect("write identity");
        assert_eq!(
            detect_device(&entry, Some(&mount))
                .expect("T-Echo identity")
                .mount(),
            mount.as_path()
        );
        fs::remove_dir_all(&mount).expect("remove mount");
    }

    #[test]
    fn unsupported_softdevice_version_is_rejected() {
        let mount = temporary_mount("unsupported-softdevice");
        fs::create_dir(&mount).expect("create mount");
        fs::write(
            mount.join("INFO_UF2.TXT"),
            info("nRF52840-TEcho-v1", "7.2.0"),
        )
        .expect("write identity");
        let error = detect_device(&t_echo_board(), Some(&mount))
            .expect_err("unsupported SoftDevice must fail");
        assert!(error
            .to_string()
            .contains("unsupported SoftDevice S140 7.2.0"));
        fs::remove_dir_all(&mount).expect("remove mount");
    }

    #[test]
    fn a_cataloged_match_rule_does_not_answer_for_another_board() {
        let mount = temporary_mount("cross-board");
        fs::create_dir(&mount).expect("create mount");
        fs::write(
            mount.join("INFO_UF2.TXT"),
            info("nRF52840-TEcho-v1", "7.3.0"),
        )
        .expect("write identity");
        let wrong_board = Uf2BoardIdMatch::parse(
            prns_flash_manifest::Uf2BoardIdMatchKind::RevisionPrefix,
            "nrf52840-heltec-t114-v",
        )
        .expect("match rule");
        let t_echo = Uf2BoardIdMatch::parse(
            prns_flash_manifest::Uf2BoardIdMatchKind::RevisionPrefix,
            "nrf52840-techo-v",
        )
        .expect("match rule");
        assert!(!mount_identity_matches(&mount, &[wrong_board]));
        assert!(mount_identity_matches(&mount, &[t_echo]));
        fs::remove_dir_all(&mount).expect("remove mount");
    }

    #[test]
    fn zero_and_multiple_mounts_are_explicit_failures() {
        let entry = t_echo_board();
        let board = cataloged_uf2(&entry);
        assert!(matches!(
            select_mount(&board, Vec::new(), None),
            Err(AppError::Preflight(_))
        ));
        let first = temporary_mount("multiple-a");
        let second = temporary_mount("multiple-b");
        for mount in [&first, &second] {
            fs::create_dir(mount).expect("create mount");
            fs::write(
                mount.join("INFO_UF2.TXT"),
                info("nRF52840-TEcho-v1", "7.3.0"),
            )
            .expect("write identity");
        }
        let error = select_mount(&board, vec![first.clone(), second.clone()], None)
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
        let entry = t_echo_board();
        let board = cataloged_uf2(&entry);
        let mount = temporary_mount("copy");
        fs::create_dir(&mount).expect("create mount");
        let destination = mount.join("firmware.uf2");
        copy_uf2(
            &destination,
            &mount,
            b"signed uf2 bytes",
            &board,
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
        let entry = t_echo_board();
        let board = cataloged_uf2(&entry);
        let disappearing = temporary_mount("disappearing");
        fs::create_dir(&disappearing).expect("create disappearing mount");
        let remover = disappearing.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            fs::remove_dir(remover).expect("remove disappearing mount");
        });
        wait_for_reboot(
            &disappearing,
            &board,
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .expect("detect disappearance");
        thread.join().expect("join remover");

        let stuck = temporary_mount("stuck");
        fs::create_dir(&stuck).expect("create stuck mount");
        let error = wait_for_reboot(&stuck, &board, Duration::ZERO, Duration::from_millis(1))
            .expect_err("persistent mount must time out");
        assert!(matches!(&error, AppError::WriteVerifyReset(_)));
        assert_eq!(
            error.to_string(),
            "UF2 was synchronized, but TECHOBOOT did not disappear within 0ns"
        );
        fs::remove_dir(stuck).expect("remove stuck mount");
    }

    #[test]
    fn reboot_after_sync_interruption_is_success_only_when_mount_disappears() {
        let entry = t_echo_board();
        let board = cataloged_uf2(&entry);
        let disappearing = temporary_mount("sync-interrupted-disappearing");
        fs::create_dir(&disappearing).expect("create disappearing mount");
        let remover = disappearing.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            fs::remove_dir(remover).expect("remove disappearing mount");
        });
        let outcome = confirm_reboot_after_synchronization_interruption(
            &disappearing,
            &board,
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
            &board,
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
