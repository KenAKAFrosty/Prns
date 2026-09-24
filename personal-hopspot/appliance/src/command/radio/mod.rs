use super::*;
use personal_hopspot_appliance::{
    RadioCandidateId, RadioPlan, RadioProfile, RadioRecoveryProgress, RadioTransaction,
    RadioTransactionError, RadioTransactionStatus,
};

mod vendor;

const RADIO_STATE_ROOT: &str = "/etc/hopspot-radio";
const IDLE_CHECK_SECONDS: u64 = 30;
const REBOOT_WAIT_SECONDS: u64 = 60;

#[derive(Subcommand)]
pub(super) enum RadioAction {
    Prepare {
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        recovery_seconds: u16,
        #[arg(long)]
        trial_boots: u8,
    },
    Apply {
        #[arg(long)]
        revision: NonZeroU64,
        #[arg(long)]
        profile_sha256: String,
    },
    Confirm {
        #[arg(long)]
        revision: NonZeroU64,
        #[arg(long)]
        profile_sha256: String,
    },
    Rollback {
        #[arg(long)]
        revision: NonZeroU64,
        #[arg(long)]
        profile_sha256: String,
    },
    Status,
    Recover,
    Watch,
}

const MAX_PROFILE_BYTES: u64 = 4096;
const MAX_BOOT_ADAPTER_BYTES: u64 = 131072;

pub(super) fn print_plan(profile: &Path, board: &Board) -> Result<(), CommandError> {
    let plan = inspect_profile(profile, board)?;
    println!("{}", serde_json::to_string(&plan)?);
    Ok(())
}

pub(super) fn inspect_profile(profile: &Path, board: &Board) -> Result<RadioPlan, CommandError> {
    let profile = read_profile(profile)?;
    inspect(&profile, board)
}

pub(super) fn boot_time(
    root: &Path,
    board: &Board,
) -> Result<personal_hopspot_appliance::BootTime, CommandError> {
    use personal_hopspot_appliance::RadioPlatform;
    Ok(vendor::OpenWrtRadio::new(root.to_owned(), board.clone()).time()?)
}

fn read_profile(path: &Path) -> Result<RadioProfile, CommandError> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        return Err(CommandError::Configuration);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn inspect(profile: &RadioProfile, board: &Board) -> Result<RadioPlan, CommandError> {
    let plan = inspect_contract(profile, board)?;
    let changes = vendor::capture(
        Command::new("/sbin/uci").args(["-q", "changes"]),
        personal_hopspot_appliance::RadioVendorOperation::Inspection,
    )?;
    if !changes.is_empty() {
        return Err(CommandError::PendingUciChanges);
    }
    Ok(plan)
}

fn inspect_contract(profile: &RadioProfile, board: &Board) -> Result<RadioPlan, CommandError> {
    let mut adapter = Vec::new();
    fs::File::open("/lib/netifd/wireless/morse.sh")?
        .take(MAX_BOOT_ADAPTER_BYTES + 1)
        .read_to_end(&mut adapter)?;
    if adapter.len() as u64 > MAX_BOOT_ADAPTER_BYTES {
        return Err(CommandError::Configuration);
    }
    let plan = profile.plan(board, &adapter)?;
    let radio = profile.binding.radio.as_str();
    let interface = profile.binding.interface.as_str();
    for (key, expected) in [
        (format!("wireless.{radio}"), "wifi-device"),
        (format!("wireless.{radio}.type"), "morse"),
        (format!("wireless.{interface}"), "wifi-iface"),
        (format!("wireless.{interface}.device"), radio),
        ("mesh11sd.mesh_params".to_owned(), "mesh11sd"),
    ] {
        let committed_key = format!("/etc/config/{key}");
        let result = vendor::capture(
            Command::new("/sbin/uci").args(["-q", "get", &committed_key]),
            personal_hopspot_appliance::RadioVendorOperation::Inspection,
        )?;
        if result != format!("{expected}\n").as_bytes() {
            return Err(CommandError::RadioBinding);
        }
    }
    Ok(plan)
}

pub(super) struct RadioContext<'a> {
    pub manager_root: &'a Path,
    pub flash_reserve_bytes: u64,
    pub board: Board,
}

pub(super) fn run(
    action: RadioAction,
    context: RadioContext<'_>,
    observer: impl personal_hopspot_appliance::ObserveRadioWrites,
) -> Result<(), CommandError> {
    let mut platform = vendor::OpenWrtRadio::new(context.manager_root.to_owned(), context.board);
    if let RadioAction::Watch = action {
        return watch(&mut platform);
    }
    let mut owner = RadioTransaction::open(Path::new(RADIO_STATE_ROOT), observer)?;
    match action {
        RadioAction::Prepare {
            profile,
            recovery_seconds,
            trial_boots,
        } => {
            let window = recovery_seconds.try_into()?;
            let boots = trial_boots.try_into()?;
            let profile = read_profile(&profile)?;
            let space = filesystem_space(Path::new(RADIO_STATE_ROOT), context.flash_reserve_bytes)?;
            let candidate = owner.prepare(&mut platform, profile, window, boots, space)?;
            platform.start_recovery_service()?;
            println!("{}", serde_json::to_string(&candidate)?);
        }
        RadioAction::Apply {
            revision,
            profile_sha256,
        } => {
            owner.apply(&mut platform, &candidate(revision, profile_sha256)?)?;
            println!("{}", serde_json::to_string(owner.status())?);
        }
        RadioAction::Confirm {
            revision,
            profile_sha256,
        } => {
            owner.confirm(&mut platform, &candidate(revision, profile_sha256)?)?;
            println!("{}", serde_json::to_string(owner.status())?);
        }
        RadioAction::Rollback {
            revision,
            profile_sha256,
        } => {
            owner.rollback(&candidate(revision, profile_sha256)?)?;
            println!("{}", serde_json::to_string(owner.status())?);
        }
        RadioAction::Status => println!("{}", serde_json::to_string(owner.status())?),
        RadioAction::Recover => {
            let progress = owner.recover(&mut platform)?;
            println!("{}", serde_json::to_string(&progress)?);
            println!("{}", serde_json::to_string(owner.status())?);
        }
        RadioAction::Watch => unreachable!("watch entered before opening a transaction"),
    }
    Ok(())
}

fn candidate(
    revision: NonZeroU64,
    profile_sha256: String,
) -> Result<RadioCandidateId, CommandError> {
    Ok(RadioCandidateId {
        revision,
        profile_sha256: profile_sha256.try_into()?,
    })
}

enum ReportedStatus {
    NoneReported,
    Previous(RadioTransactionStatus),
}

fn watch(platform: &mut vendor::OpenWrtRadio) -> Result<(), CommandError> {
    let mut reported = ReportedStatus::NoneReported;
    loop {
        let mut owner = match RadioTransaction::open(Path::new(RADIO_STATE_ROOT), UnobservedWrites)
        {
            Ok(owner) => owner,
            Err(RadioTransactionError::Locked(_)) => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let progress = owner.recover(platform)?;
        if !matches!(&reported, ReportedStatus::Previous(previous) if previous == owner.status()) {
            eprintln!("radio_recovery {}", serde_json::to_string(owner.status())?);
            reported = ReportedStatus::Previous(owner.status().clone());
        }
        drop(owner);
        let seconds = match progress {
            RadioRecoveryProgress::Inactive | RadioRecoveryProgress::Restored => IDLE_CHECK_SECONDS,
            RadioRecoveryProgress::Wait { seconds } => seconds,
            RadioRecoveryProgress::RebootRequested => REBOOT_WAIT_SECONDS,
            RadioRecoveryProgress::RebootUnavailable => {
                return Err(RadioTransactionError::Vendor(
                    personal_hopspot_appliance::RadioVendorOperation::Reboot,
                )
                .into())
            }
            RadioRecoveryProgress::RestorationUnavailable => {
                return Err(RadioTransactionError::Vendor(
                    personal_hopspot_appliance::RadioVendorOperation::Readiness,
                )
                .into())
            }
        };
        std::thread::sleep(std::time::Duration::from_secs(seconds));
    }
}
