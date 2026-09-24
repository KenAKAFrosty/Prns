use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use personal_hopspot_appliance::{
    Board, BootTime, RadioDevice, RadioFile, RadioFileImage, RadioPlan, RadioPlatform,
    RadioPreparation, RadioProfile, RadioSnapshot, RadioTransactionError, RadioVendorOperation,
    RestoredRadioReadiness, UciSection,
};

const MAX_FILE_BYTES: u64 = 131072;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_CHECK_INTERVAL: Duration = Duration::from_millis(20);
const SERVICE_START_TIMEOUT: Duration = Duration::from_secs(5);
const RECOVERY_SERVICE: &str = "/etc/init.d/hopspot-radio-recovery";
const RECOVERY_BOOT_LINK: &str = "/etc/rc.d/S18hopspot-radio-recovery";
const RECOVERY_SERVICE_NAME: &str = "hopspot-radio-recovery";
const UCI: &str = "/sbin/uci";

pub(super) struct OpenWrtRadio {
    manager_root: PathBuf,
    board: Board,
}

impl OpenWrtRadio {
    pub(super) fn new(manager_root: PathBuf, board: Board) -> Self {
        Self {
            manager_root,
            board,
        }
    }

    pub(super) fn start_recovery_service(&mut self) -> Result<(), RadioTransactionError> {
        self.require_enabled_service()?;
        execute(
            Command::new(RECOVERY_SERVICE).arg("restart"),
            RadioVendorOperation::Inspection,
        )?;
        let deadline = Instant::now() + SERVICE_START_TIMEOUT;
        loop {
            if self.require_recovery_service().is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(RadioTransactionError::RecoveryProtection);
            }
            thread::sleep(COMMAND_CHECK_INTERVAL);
        }
    }

    fn require_enabled_service(&self) -> Result<(), RadioTransactionError> {
        if !fs::symlink_metadata(RECOVERY_SERVICE)?
            .file_type()
            .is_file()
            || fs::read_link(RECOVERY_BOOT_LINK)? != Path::new("../init.d/hopspot-radio-recovery")
        {
            return Err(RadioTransactionError::RecoveryProtection);
        }
        Ok(())
    }
}

impl RadioPlatform for OpenWrtRadio {
    fn time(&mut self) -> Result<BootTime, RadioTransactionError> {
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_owned()
            .try_into()?;
        let uptime = fs::read_to_string("/proc/uptime")?;
        let uptime_seconds = uptime
            .split_whitespace()
            .next()
            .and_then(|value| value.split('.').next())
            .and_then(|value| value.parse().ok())
            .ok_or(RadioTransactionError::BootIdentity)?;
        Ok(BootTime {
            boot,
            uptime_seconds,
        })
    }

    fn prepare(
        &mut self,
        profile: &RadioProfile,
    ) -> Result<RadioPreparation, RadioTransactionError> {
        let plan = super::inspect(profile, &self.board).map_err(|error| match error {
            super::CommandError::PendingUciChanges => RadioTransactionError::PendingConfiguration,
            super::CommandError::RadioProfile(error) => RadioTransactionError::Profile(error),
            super::CommandError::RadioTransaction(error) => error,
            super::CommandError::Io(error) => RadioTransactionError::Io(error),
            _ => RadioTransactionError::Vendor(RadioVendorOperation::Inspection),
        })?;
        let original = RadioSnapshot {
            wireless: self.read(&RadioFile::Wireless)?,
            mesh11sd: self.read(&RadioFile::Mesh11sd)?,
            system: self.read(&RadioFile::System)?,
            morse_module: self.read(&RadioFile::MorseModule)?,
        };
        let shadow = tempfile::Builder::new()
            .prefix("hopspot-radio-")
            .tempdir_in("/tmp")?;
        let delta = shadow.path().join("delta");
        fs::create_dir(&delta)?;
        for (file, image) in [
            ("wireless", &original.wireless),
            ("mesh11sd", &original.mesh11sd),
        ] {
            write_private(&shadow.path().join(file), &image.bytes)?;
        }
        let batch = shadow.path().join("batch");
        let committed_batch = plan
            .uci_batch()
            .replace(
                " wireless.",
                &format!(" {}/wireless.", shadow.path().display()),
            )
            .replace(
                " mesh11sd.",
                &format!(" {}/mesh11sd.", shadow.path().display()),
            );
        write_private(&batch, committed_batch.as_bytes())?;
        let mut command = Command::new(UCI);
        command
            .arg("-c")
            .arg(shadow.path())
            .arg("-t")
            .arg(&delta)
            .args(["-q", "batch"])
            .stdin(File::open(batch)?);
        execute(&mut command, RadioVendorOperation::Staging)?;
        for package in ["wireless", "mesh11sd"] {
            execute(
                Command::new(UCI)
                    .arg("-c")
                    .arg(shadow.path())
                    .arg("-t")
                    .arg(&delta)
                    .args(["-q", "commit"])
                    .arg(shadow.path().join(package)),
                RadioVendorOperation::Staging,
            )?;
        }
        let mut projection = Vec::new();
        for package in ["wireless", "mesh11sd"] {
            projection.extend(capture(
                Command::new(UCI)
                    .args(["-q", "-X", "show"])
                    .arg(shadow.path().join(package)),
                RadioVendorOperation::Staging,
            )?);
        }
        plan.verify_uci_projection(
            std::str::from_utf8(&projection)
                .map_err(|_| RadioTransactionError::Vendor(RadioVendorOperation::Staging))?,
        )?;
        Ok(RadioPreparation {
            plan,
            original,
            candidate_wireless: bounded_read(&shadow.path().join("wireless"))?,
            candidate_mesh11sd: bounded_read(&shadow.path().join("mesh11sd"))?,
        })
    }

    fn validate(&mut self, plan: &RadioPlan) -> Result<(), RadioTransactionError> {
        let actual_board = Board::from_vendor_name(&fs::read_to_string("/tmp/sysinfo/board_name")?)
            .map_err(|_| RadioTransactionError::Vendor(RadioVendorOperation::Inspection))?;
        if actual_board != self.board || plan.board() != &self.board {
            return Err(RadioTransactionError::ConfigurationChanged);
        }
        let actual_plan = super::inspect_contract(plan.profile(), &self.board)
            .map_err(|_| RadioTransactionError::Vendor(RadioVendorOperation::Inspection))?;
        if actual_plan != *plan {
            return Err(RadioTransactionError::ConfigurationChanged);
        }
        Ok(())
    }

    fn read(&mut self, file: &RadioFile) -> Result<RadioFileImage, RadioTransactionError> {
        read_image(file_path(file))
    }

    fn replace_synced(
        &mut self,
        file: &RadioFile,
        image: &RadioFileImage,
    ) -> Result<(), RadioTransactionError> {
        let path = file_path(file);
        read_image(path)?;
        let package = match file {
            RadioFile::Wireless => Some("wireless"),
            RadioFile::Mesh11sd => Some("mesh11sd"),
            RadioFile::System => Some("system"),
            RadioFile::MorseModule => None,
        };
        if let Some(package) = package {
            execute(
                Command::new(UCI).args(["-q", "revert", package]),
                RadioVendorOperation::Staging,
            )?;
        }
        let parent = path
            .parent()
            .ok_or(RadioTransactionError::ConfigurationChanged)?;
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        staged.write_all(&image.bytes)?;
        set_mode(staged.as_file(), image.mode.unix_bits())?;
        staged.as_file().sync_all()?;
        staged
            .persist(path)
            .map_err(|error| RadioTransactionError::Io(error.error))?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }

    fn require_recovery_service(&mut self) -> Result<(), RadioTransactionError> {
        self.require_enabled_service()?;
        let services: serde_json::Value = serde_json::from_slice(&capture(
            Command::new("/bin/ubus").args([
                "call",
                "service",
                "list",
                "{\"name\":\"hopspot-radio-recovery\"}",
            ]),
            RadioVendorOperation::Inspection,
        )?)?;
        let manager = self.manager_root.join("manager");
        let expected_manager = manager
            .to_str()
            .ok_or(RadioTransactionError::RecoveryProtection)?;
        let expected_root = self
            .manager_root
            .to_str()
            .ok_or(RadioTransactionError::RecoveryProtection)?;
        let instances = services
            .get(RECOVERY_SERVICE_NAME)
            .and_then(|service| service.get("instances"))
            .and_then(serde_json::Value::as_object)
            .ok_or(RadioTransactionError::RecoveryProtection)?;
        for instance in instances.values() {
            if instance.get("running") != Some(&serde_json::Value::Bool(true)) {
                continue;
            }
            let Some(command) = instance
                .get("command")
                .and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            let args: Vec<_> = command
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect();
            if args.first() == Some(&expected_manager)
                && args
                    .windows(2)
                    .any(|pair| pair == ["--root", expected_root])
                && args.ends_with(&["radio", "watch"])
            {
                let changes = capture(
                    Command::new(UCI).args(["-q", "changes"]),
                    RadioVendorOperation::Inspection,
                )?;
                if changes.is_empty() {
                    return Ok(());
                }
                return Err(RadioTransactionError::PendingConfiguration);
            }
        }
        Err(RadioTransactionError::RecoveryProtection)
    }

    fn reload(&mut self, radio: &UciSection) -> Result<(), RadioTransactionError> {
        for action in ["down", "up"] {
            execute(
                Command::new("/sbin/wifi").args([action, radio.as_str()]),
                RadioVendorOperation::Reload,
            )?;
        }
        Ok(())
    }

    fn request_reboot(&mut self) -> Result<(), RadioTransactionError> {
        execute(
            &mut Command::new("/sbin/reboot"),
            RadioVendorOperation::Reboot,
        )
    }

    fn restored_readiness(
        &mut self,
        device: &RadioDevice,
    ) -> Result<RestoredRadioReadiness, RadioTransactionError> {
        let sysfs = Path::new("/sys/class/net").join(device.as_str());
        match fs::read_to_string(sysfs.join("carrier")) {
            Ok(carrier) if carrier.trim() == "1" => {}
            Ok(_) => return Ok(RestoredRadioReadiness::Pending),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                ) =>
            {
                return Ok(RestoredRadioReadiness::Pending)
            }
            Err(error) => return Err(error.into()),
        }
        let flags = fs::read_to_string(sysfs.join("flags"))?;
        let flags = u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
            .map_err(|_| RadioTransactionError::Vendor(RadioVendorOperation::Readiness))?;
        if flags & 1 == 0 {
            return Ok(RestoredRadioReadiness::Pending);
        }
        let health = capture(
            Command::new("/sbin/morse_cli").args(["-i", device.as_str(), "health"]),
            RadioVendorOperation::Readiness,
        );
        match health {
            Ok(output)
                if output
                    .split(|byte| *byte == b'\n')
                    .any(|line| line == b"health check: success") =>
            {
                Ok(RestoredRadioReadiness::Operational)
            }
            Ok(_) | Err(RadioTransactionError::Vendor(RadioVendorOperation::Readiness)) => {
                Ok(RestoredRadioReadiness::Pending)
            }
            Err(error) => Err(error),
        }
    }
}

fn file_path(file: &RadioFile) -> &'static Path {
    Path::new(match file {
        RadioFile::Wireless => "/etc/config/wireless",
        RadioFile::Mesh11sd => "/etc/config/mesh11sd",
        RadioFile::System => "/etc/config/system",
        RadioFile::MorseModule => "/etc/modules.d/morse",
    })
}

fn bounded_read(path: &Path) -> Result<Vec<u8>, RadioTransactionError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(RadioTransactionError::ReadBudget);
    }
    Ok(bytes)
}

fn read_image(path: &Path) -> Result<RadioFileImage, RadioTransactionError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(RadioTransactionError::ConfigurationChanged);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(RadioFileImage {
            bytes: bounded_read(path)?,
            mode: (metadata.permissions().mode() & 0o777).try_into()?,
        })
    }
    #[cfg(not(unix))]
    Err(RadioTransactionError::Vendor(
        RadioVendorOperation::Inspection,
    ))
}

fn set_mode(file: &File, mode: u32) -> Result<(), RadioTransactionError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
        Err(RadioTransactionError::Vendor(RadioVendorOperation::Staging))
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), RadioTransactionError> {
    let mut file = File::create_new(path)?;
    set_mode(&file, 0o600)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn execute(
    command: &mut Command,
    operation: RadioVendorOperation,
) -> Result<(), RadioTransactionError> {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    wait(command, operation)
}

pub(super) fn capture(
    command: &mut Command,
    operation: RadioVendorOperation,
) -> Result<Vec<u8>, RadioTransactionError> {
    let output = tempfile::NamedTempFile::new_in("/tmp")?;
    command
        .stdout(output.as_file().try_clone()?)
        .stderr(Stdio::null());
    wait(command, operation)?;
    bounded_read(output.path())
}

fn wait(
    command: &mut Command,
    operation: RadioVendorOperation,
) -> Result<(), RadioTransactionError> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(RadioTransactionError::Vendor(operation))
            };
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(RadioTransactionError::Vendor(operation));
        }
        thread::sleep(COMMAND_CHECK_INTERVAL);
    }
}
