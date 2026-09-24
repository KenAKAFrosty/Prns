use super::*;

const BOOT_A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const BOOT_B: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
const BOOT_C: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";

enum Protection {
    Running,
    Missing,
}
struct Platform {
    now: BootTime,
    files: RadioSnapshot,
    protection: Protection,
    readiness: RestoredRadioReadiness,
    writes: Vec<RadioFile>,
    reboots: usize,
}

fn image(bytes: &[u8], mode: u32) -> RadioFileImage {
    RadioFileImage {
        bytes: bytes.to_vec(),
        mode: mode.try_into().unwrap(),
    }
}

fn original() -> RadioSnapshot {
    RadioSnapshot {
        wireless: image(b"original credentials and radio", 0o600),
        mesh11sd: image(b"original mesh settings", 0o600),
        system: image(b"original system", 0o600),
        morse_module: image(b"original module options", 0o644),
    }
}

fn platform() -> Platform {
    Platform {
        now: BootTime {
            boot: BOOT_A.to_owned().try_into().unwrap(),
            uptime_seconds: 10,
        },
        files: original(),
        protection: Protection::Running,
        readiness: RestoredRadioReadiness::Pending,
        writes: Vec::new(),
        reboots: 0,
    }
}

impl RadioPlatform for Platform {
    fn time(&mut self) -> Result<BootTime, RadioTransactionError> {
        Ok(self.now.clone())
    }
    fn prepare(
        &mut self,
        profile: &RadioProfile,
    ) -> Result<RadioPreparation, RadioTransactionError> {
        Ok(RadioPreparation {
            plan: profile.qualified_plan(
                &Board::ThinkNodeG4,
                super::super::G4_BOOT_ADAPTER_SHA256.to_owned(),
            ),
            original: RadioSnapshot {
                wireless: self.files.wireless.clone(),
                mesh11sd: self.files.mesh11sd.clone(),
                system: self.files.system.clone(),
                morse_module: self.files.morse_module.clone(),
            },
            candidate_wireless: b"qualified mesh radio".to_vec(),
            candidate_mesh11sd: b"qualified proactive paths".to_vec(),
        })
    }
    fn validate(&mut self, _: &RadioPlan) -> Result<(), RadioTransactionError> {
        Ok(())
    }
    fn read(&mut self, file: &RadioFile) -> Result<RadioFileImage, RadioTransactionError> {
        Ok(self.files.image(file).clone())
    }
    fn replace_synced(
        &mut self,
        file: &RadioFile,
        image: &RadioFileImage,
    ) -> Result<(), RadioTransactionError> {
        match file {
            RadioFile::Wireless => self.files.wireless = image.clone(),
            RadioFile::Mesh11sd => self.files.mesh11sd = image.clone(),
            RadioFile::System => self.files.system = image.clone(),
            RadioFile::MorseModule => self.files.morse_module = image.clone(),
        }
        self.writes.push(file.clone());
        Ok(())
    }
    fn require_recovery_service(&mut self) -> Result<(), RadioTransactionError> {
        match self.protection {
            Protection::Running => Ok(()),
            Protection::Missing => Err(RadioTransactionError::RecoveryProtection),
        }
    }
    fn reload(&mut self, _: &crate::UciSection) -> Result<(), RadioTransactionError> {
        Ok(())
    }
    fn request_reboot(&mut self) -> Result<(), RadioTransactionError> {
        self.reboots += 1;
        Ok(())
    }
    fn restored_readiness(
        &mut self,
        _: &crate::RadioDevice,
    ) -> Result<RestoredRadioReadiness, RadioTransactionError> {
        Ok(match self.readiness {
            RestoredRadioReadiness::Pending => RestoredRadioReadiness::Pending,
            RestoredRadioReadiness::Operational => RestoredRadioReadiness::Operational,
        })
    }
}

fn profile() -> RadioProfile {
    serde_json::from_str(include_str!("../../../openwrt/radio-profile.example.json")).unwrap()
}
fn prepare<O: ObserveRadioWrites>(
    owner: &mut RadioTransaction<O>,
    host: &mut Platform,
) -> RadioCandidateId {
    owner
        .prepare(
            host,
            profile(),
            100_u16.try_into().unwrap(),
            1_u8.try_into().unwrap(),
            SpaceBudget {
                available_bytes: 1_000_000,
                reserve_bytes: 4096,
            },
        )
        .unwrap()
}
fn open(root: &Path) -> RadioTransaction {
    RadioTransaction::open(root, UnobservedWrites).unwrap()
}
fn assert_original(host: &Platform) {
    for file in RadioFile::ALL {
        assert!(
            host.files.image(&file) == original().image(&file),
            "original file and mode restored: {file:?}"
        );
    }
}
fn settle_restoration(root: &Path, host: &mut Platform) {
    let mut owner = open(root);
    let progress = owner.recover(host).unwrap();
    if progress == RadioRecoveryProgress::RebootRequested {
        host.now = BootTime {
            boot: BOOT_C.to_owned().try_into().unwrap(),
            uptime_seconds: 1,
        };
        host.readiness = RestoredRadioReadiness::Operational;
        assert_eq!(
            owner.recover(host).unwrap(),
            RadioRecoveryProgress::Restored
        );
    }
    assert_original(host);
}

#[test]
fn apply_requires_live_recovery_unchanged_files_and_exact_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    host.protection = Protection::Missing;
    assert!(matches!(
        owner.apply(&mut host, &id),
        Err(RadioTransactionError::RecoveryProtection)
    ));
    assert!(host.writes.is_empty());
    host.protection = Protection::Running;
    host.files.system.bytes.push(b'x');
    assert!(matches!(
        owner.apply(&mut host, &id),
        Err(RadioTransactionError::ConfigurationChanged)
    ));
    assert!(host.writes.is_empty());
    host.files.system = original().system;
    owner.apply(&mut host, &id).unwrap();
    assert_eq!(host.writes, RadioFile::CANDIDATE);
    let mut stale = id.clone();
    stale.profile_sha256 = RadioDigest::of(b"another profile");
    assert!(matches!(
        owner.confirm(&mut host, &stale),
        Err(RadioTransactionError::StaleCandidate)
    ));
    owner.confirm(&mut host, &id).unwrap();
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Inactive
    );
    assert_eq!(host.reboots, 0);
}

#[test]
fn expiry_restores_files_and_modes_then_requires_new_boot_and_operating_radio() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    host.now.uptime_seconds = 110;
    assert!(matches!(
        owner.confirm(&mut host, &id),
        Err(RadioTransactionError::LeaseExpired)
    ));
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::RebootRequested
    );
    assert_original(&host);
    assert!(matches!(
        owner.status.state,
        RadioTransactionState::Active {
            phase: RadioPhase::RebootRequired { .. },
            ..
        }
    ));
    host.now = BootTime {
        boot: BOOT_B.to_owned().try_into().unwrap(),
        uptime_seconds: 5,
    };
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Wait { seconds: 2 }
    );
    assert!(matches!(
        owner.status.state,
        RadioTransactionState::Active {
            phase: RadioPhase::CheckingRestoration { .. },
            ..
        }
    ));
    host.readiness = RestoredRadioReadiness::Operational;
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Restored
    );
}

#[test]
fn reboot_allowance_is_persisted_and_no_regular_wait_renews_the_lease() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    host.now = BootTime {
        boot: BOOT_B.to_owned().try_into().unwrap(),
        uptime_seconds: 1,
    };
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Wait { seconds: 2 }
    );
    let renewed_once = owner.status.clone();
    for tick in 2..20 {
        host.now.uptime_seconds = tick;
        owner.recover(&mut host).unwrap();
        assert_eq!(owner.status, renewed_once);
    }
    drop(owner);
    let mut owner = open(root.path());
    host.now = BootTime {
        boot: BOOT_C.to_owned().try_into().unwrap(),
        uptime_seconds: 1,
    };
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::RebootRequested
    );
    assert_original(&host);
}

#[test]
fn unfinished_prepare_cancels_without_mutation_and_restoration_wait_is_bounded() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.rollback(&id).unwrap();
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Inactive
    );
    assert!(host.writes.is_empty());
    assert_eq!(host.reboots, 0);
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    owner.rollback(&id).unwrap();
    owner.recover(&mut host).unwrap();
    host.now = BootTime {
        boot: BOOT_B.to_owned().try_into().unwrap(),
        uptime_seconds: 1,
    };
    owner.recover(&mut host).unwrap();
    host.now.uptime_seconds = 361;
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::RestorationUnavailable
    );
    assert!(matches!(
        owner.prepare(
            &mut host,
            profile(),
            100_u16.try_into().unwrap(),
            1_u8.try_into().unwrap(),
            SpaceBudget {
                available_bytes: 1_000_000,
                reserve_bytes: 4096
            }
        ),
        Err(RadioTransactionError::InProgress)
    ));
    host.readiness = RestoredRadioReadiness::Operational;
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Restored
    );
}

enum Fault {
    Record,
    Cut(usize),
    AtCheckpoint(RadioCheckpoint),
}
struct Observer {
    fault: Fault,
    seen: Vec<RadioCheckpoint>,
}
impl ObserveRadioWrites for Observer {
    fn reached(&mut self, checkpoint: RadioCheckpoint) -> std::io::Result<()> {
        self.seen.push(checkpoint);
        if matches!(self.fault, Fault::Cut(at) if at == self.seen.len())
            || matches!(&self.fault, Fault::AtCheckpoint(expected) if self.seen.last() == Some(expected))
        {
            return Err(std::io::Error::other("injected publication interruption"));
        }
        Ok(())
    }
}

#[test]
fn every_prepare_and_apply_publication_cut_keeps_or_recovers_original_files() {
    let count = {
        let root = tempfile::tempdir().unwrap();
        let mut host = platform();
        let mut owner = RadioTransaction::open(
            root.path(),
            Observer {
                fault: Fault::Record,
                seen: Vec::new(),
            },
        )
        .unwrap();
        let id = prepare(&mut owner, &mut host);
        owner.apply(&mut host, &id).unwrap();
        owner.observer.seen.len()
    };
    for cut in 1..=count {
        let root = tempfile::tempdir().unwrap();
        let mut host = platform();
        let mut owner = RadioTransaction::open(
            root.path(),
            Observer {
                fault: Fault::Cut(cut),
                seen: Vec::new(),
            },
        )
        .unwrap();
        let result = owner.prepare(
            &mut host,
            profile(),
            100_u16.try_into().unwrap(),
            1_u8.try_into().unwrap(),
            SpaceBudget {
                available_bytes: 1_000_000,
                reserve_bytes: 4096,
            },
        );
        let failure = match result {
            Ok(id) => owner.apply(&mut host, &id),
            Err(error) => Err(error),
        };
        assert!(failure.is_err(), "cut {cut}");
        assert!(matches!(
            owner.admit(),
            Err(RadioTransactionError::ReopenRequired)
        ));
        drop(owner);
        host.now.uptime_seconds = 1000;
        settle_restoration(root.path(), &mut host);
    }
}

#[test]
fn every_restore_publication_cut_retries_idempotently_before_requesting_reboot() {
    let count = {
        let root = tempfile::tempdir().unwrap();
        let mut host = platform();
        let mut owner = open(root.path());
        let id = prepare(&mut owner, &mut host);
        owner.apply(&mut host, &id).unwrap();
        owner.rollback(&id).unwrap();
        drop(owner);
        let mut owner = RadioTransaction::open(
            root.path(),
            Observer {
                fault: Fault::Record,
                seen: Vec::new(),
            },
        )
        .unwrap();
        owner.recover(&mut host).unwrap();
        owner.observer.seen.len()
    };
    for cut in 1..=count {
        let root = tempfile::tempdir().unwrap();
        let mut host = platform();
        let mut owner = open(root.path());
        let id = prepare(&mut owner, &mut host);
        owner.apply(&mut host, &id).unwrap();
        owner.rollback(&id).unwrap();
        drop(owner);
        let mut owner = RadioTransaction::open(
            root.path(),
            Observer {
                fault: Fault::Cut(cut),
                seen: Vec::new(),
            },
        )
        .unwrap();
        assert!(owner.recover(&mut host).is_err());
        drop(owner);
        settle_restoration(root.path(), &mut host);
    }
}

#[test]
fn lost_applying_journal_cannot_disguise_partial_mutation_as_canceled_preparation() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = RadioTransaction::open(
        root.path(),
        Observer {
            fault: Fault::AtCheckpoint(RadioCheckpoint::VendorFileReplaced(RadioFile::Wireless)),
            seen: Vec::new(),
        },
    )
    .unwrap();
    let id = prepare(&mut owner, &mut host);
    assert!(owner.apply(&mut host, &id).is_err());
    assert_eq!(host.files.wireless.bytes, b"qualified mesh radio");
    assert!(host.files.mesh11sd == original().mesh11sd);
    let applying_bank = owner.status.revision % 2;
    fs::write(
        root.path().join(format!("journal-{applying_bank}.json")),
        b"torn",
    )
    .unwrap();
    drop(owner);
    settle_restoration(root.path(), &mut host);
}

#[test]
fn corrupt_backups_refuse_all_host_writes_and_confirmation_cuts_do_not_accept_another_candidate() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    owner.rollback(&id).unwrap();
    fs::write(root.path().join("active/original-system"), b"corrupt").unwrap();
    host.writes.clear();
    assert!(matches!(
        owner.recover(&mut host),
        Err(RadioTransactionError::BackupIntegrity)
    ));
    assert!(host.writes.is_empty());
    assert_eq!(host.reboots, 0);
    for cut in 1..=3 {
        let root = tempfile::tempdir().unwrap();
        let mut host = platform();
        let mut owner = open(root.path());
        let id = prepare(&mut owner, &mut host);
        owner.apply(&mut host, &id).unwrap();
        drop(owner);
        let mut owner = RadioTransaction::open(
            root.path(),
            Observer {
                fault: Fault::Cut(cut),
                seen: Vec::new(),
            },
        )
        .unwrap();
        assert!(owner.confirm(&mut host, &id).is_err());
        drop(owner);
        let owner = open(root.path());
        assert!(
            matches!(owner.status.state, RadioTransactionState::Active { candidate, phase: RadioPhase::Trial { .. } | RadioPhase::Confirmed } if candidate == id)
        );
    }
}

#[test]
fn repeated_transactions_keep_one_guard_and_cannot_alias_old_confirmations() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let first = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &first).unwrap();
    owner.confirm(&mut host, &first).unwrap();
    for _ in 0..8 {
        let current = prepare(&mut owner, &mut host);
        assert!(current.revision > first.revision);
        owner.apply(&mut host, &current).unwrap();
        assert!(matches!(
            owner.confirm(&mut host, &first),
            Err(RadioTransactionError::StaleCandidate)
        ));
        owner.confirm(&mut host, &current).unwrap();
        assert!(!root.path().join("staging").exists());
        assert_eq!(fs::read_dir(root.path().join("active")).unwrap().count(), 8);
    }
}

#[test]
fn invalid_time_policy_low_space_and_concurrent_writers_are_refused_before_mutation() {
    assert!(RecoveryWindow::try_from(0).is_err());
    assert!(RecoveryWindow::try_from(3601).is_err());
    assert!(TrialBoots::try_from(0).is_err());
    assert!(TrialBoots::try_from(4).is_err());
    assert!(BootId::try_from("not-a-boot".to_owned()).is_err());
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    assert!(matches!(
        RadioTransaction::open(root.path(), UnobservedWrites),
        Err(RadioTransactionError::Locked(_))
    ));
    assert!(matches!(
        owner.prepare(
            &mut host,
            profile(),
            100_u16.try_into().unwrap(),
            1_u8.try_into().unwrap(),
            SpaceBudget {
                available_bytes: 1,
                reserve_bytes: 4096
            }
        ),
        Err(RadioTransactionError::LowSpace)
    ));
    assert!(host.writes.is_empty());
    assert_eq!(owner.status.state, RadioTransactionState::Empty);
}

#[test]
fn reboot_acknowledgment_without_new_epoch_has_a_persisted_deadline() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    owner.rollback(&id).unwrap();
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::RebootRequested
    );
    drop(owner);
    host.now.uptime_seconds += 60;
    let mut owner = open(root.path());
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::RebootUnavailable
    );
    assert_eq!(host.reboots, 1);
    drop(owner);
    let mut owner = open(root.path());
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::RebootUnavailable
    );
    assert_eq!(host.reboots, 1);
    assert!(matches!(
        owner.prepare(
            &mut host,
            profile(),
            100_u16.try_into().unwrap(),
            1_u8.try_into().unwrap(),
            SpaceBudget {
                available_bytes: 1_000_000,
                reserve_bytes: 4096
            }
        ),
        Err(RadioTransactionError::InProgress)
    ));
    host.now.boot = BOOT_B.to_owned().try_into().unwrap();
    host.now.uptime_seconds = 1;
    host.readiness = RestoredRadioReadiness::Operational;
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Restored
    );
}

#[test]
fn damaged_journals_and_intent_still_restore_from_the_valid_guard() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    drop(owner);
    for path in [
        "journal-0.json",
        "journal-1.json",
        "active/mutation-intent.json",
    ] {
        fs::write(root.path().join(path), b"interrupted metadata").unwrap();
    }
    let mut owner = open(root.path());
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::RebootRequested
    );
    assert_original(&host);
    assert_eq!(host.reboots, 1);
}

#[test]
fn every_replacement_preparation_cut_preserves_the_confirmed_configuration() {
    let observed = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(observed.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    owner.confirm(&mut host, &id).unwrap();
    drop(owner);
    let mut owner = RadioTransaction::open(
        observed.path(),
        Observer {
            fault: Fault::Record,
            seen: Vec::new(),
        },
    )
    .unwrap();
    prepare(&mut owner, &mut host);
    let count = owner.observer.seen.len();
    drop(owner);
    for cut in 1..=count {
        let root = tempfile::tempdir().unwrap();
        let mut host = platform();
        let mut owner = open(root.path());
        let id = prepare(&mut owner, &mut host);
        owner.apply(&mut host, &id).unwrap();
        owner.confirm(&mut host, &id).unwrap();
        let confirmed_wireless = host.files.wireless.clone();
        let confirmed_mesh = host.files.mesh11sd.clone();
        drop(owner);
        let mut owner = RadioTransaction::open(
            root.path(),
            Observer {
                fault: Fault::Cut(cut),
                seen: Vec::new(),
            },
        )
        .unwrap();
        assert!(owner
            .prepare(
                &mut host,
                profile(),
                100_u16.try_into().unwrap(),
                1_u8.try_into().unwrap(),
                SpaceBudget {
                    available_bytes: 1_000_000,
                    reserve_bytes: 4096
                }
            )
            .is_err());
        drop(owner);
        host.now.uptime_seconds = 200;
        open(root.path()).recover(&mut host).unwrap();
        assert!(host.files.wireless == confirmed_wireless);
        assert!(host.files.mesh11sd == confirmed_mesh);
        assert_eq!(host.reboots, 0);
    }
}

#[test]
fn a_valid_confirmation_remains_authoritative_when_old_intent_is_damaged() {
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    owner.confirm(&mut host, &id).unwrap();
    let confirmed = owner.status().clone();
    drop(owner);
    fs::write(
        root.path().join("active/mutation-intent.json"),
        b"damaged old intent",
    )
    .unwrap();
    let mut owner = open(root.path());
    assert_eq!(owner.status(), &confirmed);
    assert_eq!(
        owner.recover(&mut host).unwrap(),
        RadioRecoveryProgress::Inactive
    );
    assert_eq!(host.reboots, 0);
    assert_eq!(host.writes, RadioFile::CANDIDATE);
}

#[test]
fn reopening_for_regular_waits_preserves_flash_inode_metadata() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let mut host = platform();
    let mut owner = open(root.path());
    let id = prepare(&mut owner, &mut host);
    owner.apply(&mut host, &id).unwrap();
    drop(owner);
    let paths = [
        root.path().to_owned(),
        root.path().join("radio.lock"),
        root.path().join("journal-0.json"),
        root.path().join("journal-1.json"),
    ];
    let metadata = || {
        paths
            .iter()
            .map(|path| {
                let stat = fs::metadata(path).unwrap();
                (
                    stat.mode(),
                    stat.ctime(),
                    stat.ctime_nsec(),
                    stat.mtime(),
                    stat.mtime_nsec(),
                    stat.len(),
                )
            })
            .collect::<Vec<_>>()
    };
    let before = metadata();
    for _ in 0..5 {
        assert!(matches!(
            open(root.path()).recover(&mut host).unwrap(),
            RadioRecoveryProgress::Wait { .. }
        ));
    }
    assert_eq!(metadata(), before);
}
