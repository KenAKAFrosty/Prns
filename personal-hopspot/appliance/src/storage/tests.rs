use std::{fs, num::NonZeroU8, path::Path};

use super::*;

const KEY: &str = include_str!("fixtures/public.key");

fn fixture(name: &str) -> tempfile::TempDir {
    let (manifest, signature, compressed): (&[u8], &[u8], &[u8]) = match name {
        "first" => (
            include_bytes!("fixtures/first/manifest.json"),
            include_bytes!("fixtures/first/manifest.minisig"),
            include_bytes!("fixtures/first/app.gz"),
        ),
        "second" => (
            include_bytes!("fixtures/second/manifest.json"),
            include_bytes!("fixtures/second/manifest.minisig"),
            include_bytes!("fixtures/second/app.gz"),
        ),
        "hard-float" => (
            include_bytes!("fixtures/hard-float/manifest.json"),
            include_bytes!("fixtures/hard-float/manifest.minisig"),
            include_bytes!("fixtures/hard-float/app.gz"),
        ),
        "oversized-expansion" => (
            include_bytes!("fixtures/oversized-expansion/manifest.json"),
            include_bytes!("fixtures/oversized-expansion/manifest.minisig"),
            include_bytes!("fixtures/oversized-expansion/app.gz"),
        ),
        "wrong-executable-hash" => (
            include_bytes!("fixtures/wrong-executable-hash/manifest.json"),
            include_bytes!("fixtures/wrong-executable-hash/manifest.minisig"),
            include_bytes!("fixtures/wrong-executable-hash/app.gz"),
        ),
        _ => panic!("unknown fixture"),
    };
    let directory = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        ("manifest.json", manifest),
        ("manifest.minisig", signature),
        ("app.gz", compressed),
    ] {
        fs::write(directory.path().join(name), bytes).unwrap();
    }
    directory
}
fn budgets() -> Budgets {
    Budgets {
        compressed_bytes: 4096,
        executable_bytes: 4096,
    }
}
fn space() -> SpaceBudget {
    SpaceBudget {
        available_bytes: 1_000_000,
        reserve_bytes: 4096,
    }
}
fn open(root: &Path) -> Appliance<'static> {
    Appliance::open(root, KEY, Board::ThinkNodeG4, budgets(), UnobservedWrites).unwrap()
}
fn stage(app: &mut Appliance<'_>, name: &str) -> CandidateId {
    let package = app.verify_directory(fixture(name).path()).unwrap();
    app.stage(
        package,
        LaunchBudget::new(NonZeroU8::new(3).unwrap()),
        space(),
    )
    .unwrap()
}

#[test]
fn exhausted_trials_restore_confirmed_bytes_without_touching_identity_or_grants() {
    let root = tempfile::tempdir().unwrap();
    let ram = tempfile::tempdir().unwrap();
    let mut app = open(root.path());
    fs::create_dir(root.path().join("state")).unwrap();
    fs::write(root.path().join("state/private-identity"), b"never replace").unwrap();
    fs::write(
        root.path().join("state/controller-grants"),
        b"revoked stays revoked",
    )
    .unwrap();
    let first = stage(&mut app, "first");
    app.confirm(&first).unwrap();
    let second = stage(&mut app, "second");
    for _ in 0..3 {
        assert_eq!(app.prepare_launch(ram.path(), space()).unwrap().1, second);
    }
    drop(app);
    let mut app = open(root.path());
    let (executable, restored) = app.prepare_launch(ram.path(), space()).unwrap();
    assert_eq!(restored, first);
    assert_eq!(
        prns_flash_manifest::sha256_hex(&fs::read(executable).unwrap()),
        first.executable_sha256.as_str()
    );
    assert!(matches!(
        app.confirm(&second),
        Err(Error::StaleConfirmation)
    ));
    assert_eq!(
        fs::read(root.path().join("state/private-identity")).unwrap(),
        b"never replace"
    );
    assert_eq!(
        fs::read(root.path().join("state/controller-grants")).unwrap(),
        b"revoked stays revoked"
    );
}

#[test]
fn empty_installation_fails_closed_after_its_trial_budget() {
    let root = tempfile::tempdir().unwrap();
    let ram = tempfile::tempdir().unwrap();
    let mut app = open(root.path());
    stage(&mut app, "first");
    for _ in 0..3 {
        app.prepare_launch(ram.path(), space()).unwrap();
    }
    assert!(matches!(
        app.prepare_launch(ram.path(), space()),
        Err(Error::NoApplication)
    ));
    assert_eq!(app.activation.status(), &Status::Uninstalled);
}

struct CutAt {
    remaining: usize,
}
impl ObserveWrites for CutAt {
    fn reached(&mut self, _: Checkpoint) -> std::io::Result<()> {
        self.remaining -= 1;
        if self.remaining == 0 {
            return Err(std::io::Error::other("simulated interruption"));
        }
        Ok(())
    }
}

#[test]
fn every_stage_publication_cut_retains_a_verified_launch_and_the_original_state() {
    // Three file syncs, slot publication, and three journal publication steps.
    for cut in 1..=7 {
        let root = tempfile::tempdir().unwrap();
        let ram = tempfile::tempdir().unwrap();
        let mut app = open(root.path());
        let first = stage(&mut app, "first");
        app.confirm(&first).unwrap();
        fs::write(root.path().join("identity-sentinel"), b"private").unwrap();
        drop(app);
        let mut interrupted = Appliance::open(
            root.path(),
            KEY,
            Board::ThinkNodeG4,
            budgets(),
            CutAt { remaining: cut },
        )
        .unwrap();
        let package = interrupted
            .verify_directory(fixture("second").path())
            .unwrap();
        assert!(matches!(
            interrupted.stage(
                package,
                LaunchBudget::new(NonZeroU8::new(3).unwrap()),
                space()
            ),
            Err(Error::Io(_))
        ));
        assert!(matches!(
            interrupted.prepare_launch(ram.path(), space()),
            Err(Error::ReopenRequired)
        ));
        drop(interrupted);
        let mut recovered = open(root.path());
        let (executable, candidate) = recovered.prepare_launch(ram.path(), space()).unwrap();
        assert_eq!(
            prns_flash_manifest::sha256_hex(&fs::read(executable).unwrap()),
            candidate.executable_sha256.as_str()
        );
        recovered
            .rollback()
            .unwrap_or_else(|error| assert!(matches!(error, Error::NoApplication)));
        assert_eq!(
            recovered.prepare_launch(ram.path(), space()).unwrap().1,
            first
        );
        assert_eq!(
            fs::read(root.path().join("identity-sentinel")).unwrap(),
            b"private"
        );
    }
}

#[test]
fn every_launch_cut_consumes_an_attempt_before_execution_or_keeps_the_previous_record() {
    for cut in 1..=4 {
        let root = tempfile::tempdir().unwrap();
        let ram = tempfile::tempdir().unwrap();
        let mut app = open(root.path());
        stage(&mut app, "first");
        drop(app);
        let mut interrupted = Appliance::open(
            root.path(),
            KEY,
            Board::ThinkNodeG4,
            budgets(),
            CutAt { remaining: cut },
        )
        .unwrap();
        assert!(interrupted.prepare_launch(ram.path(), space()).is_err());
        drop(interrupted);
        let recovered = open(root.path());
        let Status::Trial {
            launches_remaining, ..
        } = recovered.activation.status()
        else {
            panic!("trial lost")
        };
        assert_eq!(*launches_remaining, if cut == 1 { 3 } else { 2 });
    }
}

#[test]
fn confirmation_and_rollback_interruptions_choose_only_complete_journals() {
    for operation in ["confirm", "rollback"] {
        for cut in 1..=3 {
            let root = tempfile::tempdir().unwrap();
            let ram = tempfile::tempdir().unwrap();
            let mut app = open(root.path());
            let first = stage(&mut app, "first");
            app.confirm(&first).unwrap();
            let second = stage(&mut app, "second");
            drop(app);
            let mut interrupted = Appliance::open(
                root.path(),
                KEY,
                Board::ThinkNodeG4,
                budgets(),
                CutAt { remaining: cut },
            )
            .unwrap();
            let outcome = match operation {
                "confirm" => interrupted.confirm(&second),
                "rollback" => interrupted.rollback(),
                _ => unreachable!(),
            };
            assert!(outcome.is_err());
            drop(interrupted);
            let mut recovered = open(root.path());
            let (_, selected) = recovered.prepare_launch(ram.path(), space()).unwrap();
            let expected = if operation == "rollback" && cut > 1 {
                &first
            } else {
                &second
            };
            assert_eq!(&selected, expected);
        }
    }
}

#[test]
fn torn_newest_bank_falls_back_and_two_corrupt_or_missing_banks_refuse_startup() {
    let root = tempfile::tempdir().unwrap();
    let mut app = open(root.path());
    let first = stage(&mut app, "first");
    app.confirm(&first).unwrap();
    drop(app);
    fs::write(root.path().join("journal-0.json"), b"{torn").unwrap();
    let app = open(root.path());
    assert!(matches!(app.activation.status(), Status::Trial { .. }));
    drop(app);
    fs::write(root.path().join("journal-1.json"), b"{torn").unwrap();
    assert!(matches!(
        Appliance::open(
            root.path(),
            KEY,
            Board::ThinkNodeG4,
            budgets(),
            UnobservedWrites
        ),
        Err(Error::CorruptJournal)
    ));
    fs::remove_file(root.path().join("journal-0.json")).unwrap();
    fs::remove_file(root.path().join("journal-1.json")).unwrap();
    assert!(matches!(
        Appliance::open(
            root.path(),
            KEY,
            Board::ThinkNodeG4,
            budgets(),
            UnobservedWrites
        ),
        Err(Error::CorruptJournal)
    ));
}

#[test]
fn corrupt_trial_rolls_back_but_corrupt_confirmed_application_never_executes() {
    let root = tempfile::tempdir().unwrap();
    let ram = tempfile::tempdir().unwrap();
    let mut app = open(root.path());
    let first = stage(&mut app, "first");
    app.confirm(&first).unwrap();
    stage(&mut app, "second");
    fs::write(root.path().join("slots/b/app.gz"), b"broken").unwrap();
    assert_eq!(app.prepare_launch(ram.path(), space()).unwrap().1, first);
    fs::write(root.path().join("slots/a/app.gz"), b"broken").unwrap();
    assert!(app.prepare_launch(ram.path(), space()).is_err());
}

#[test]
fn writers_and_overlapping_trials_are_refused_and_confirmations_are_generation_bound() {
    let root = tempfile::tempdir().unwrap();
    let mut app = open(root.path());
    assert!(matches!(
        Appliance::open(
            root.path(),
            KEY,
            Board::ThinkNodeG4,
            budgets(),
            UnobservedWrites
        ),
        Err(Error::Locked(_))
    ));
    let first = stage(&mut app, "first");
    let package = app.verify_directory(fixture("second").path()).unwrap();
    assert!(matches!(
        app.stage(
            package,
            LaunchBudget::new(NonZeroU8::new(3).unwrap()),
            space()
        ),
        Err(Error::TrialInProgress)
    ));
    let stale = CandidateId {
        revision: NonZeroU64::new(first.revision.get() + 1).unwrap(),
        executable_sha256: first.executable_sha256.clone(),
    };
    assert!(matches!(app.confirm(&stale), Err(Error::StaleConfirmation)));
}

#[test]
fn signatures_board_abi_lengths_and_read_budgets_are_checked_before_activation() {
    let root = tempfile::tempdir().unwrap();
    let app = open(root.path());
    assert!(matches!(
        app.verify_directory(fixture("hard-float").path()),
        Err(Error::Package(PackageError::Abi))
    ));
    let other = tempfile::tempdir().unwrap();
    let other_app = Appliance::open(
        other.path(),
        KEY,
        Board::HeltecHtHd01V2,
        budgets(),
        UnobservedWrites,
    )
    .unwrap();
    assert!(matches!(
        other_app.verify_directory(fixture("first").path()),
        Err(Error::Package(PackageError::BoardMismatch))
    ));
    let directory = tempfile::tempdir().unwrap();
    for name in ["manifest.json", "manifest.minisig", "app.gz"] {
        fs::copy(
            fixture("first").path().join(name),
            directory.path().join(name),
        )
        .unwrap();
    }
    fs::write(directory.path().join("app.gz"), b"truncated").unwrap();
    assert!(matches!(
        app.verify_directory(directory.path()),
        Err(Error::Package(PackageError::ArtifactMismatch))
    ));
    fs::write(directory.path().join("manifest.minisig"), b"untrusted").unwrap();
    assert!(matches!(
        app.verify_directory(directory.path()),
        Err(Error::Package(PackageError::Signature(_)))
    ));
    fs::write(directory.path().join("manifest.minisig"), [0xff]).unwrap();
    assert!(matches!(
        app.verify_directory(directory.path()),
        Err(Error::SignatureEncoding)
    ));
    assert!(matches!(
        bounded_read(&fixture("first").path().join("app.gz"), u64::MAX),
        Err(Error::ReadBudget)
    ));
    fs::write(directory.path().join("manifest.json"), vec![0; 4097]).unwrap();
    assert!(matches!(
        app.verify_directory(directory.path()),
        Err(Error::ReadBudget)
    ));
    assert_eq!(app.activation.status(), &Status::Uninstalled);
}

#[test]
fn low_space_refuses_staging_without_disturbing_the_confirmed_application() {
    let root = tempfile::tempdir().unwrap();
    let ram = tempfile::tempdir().unwrap();
    let mut app = open(root.path());
    let first = stage(&mut app, "first");
    app.confirm(&first).unwrap();
    let revision = app.activation.revision();
    let package = app.verify_directory(fixture("second").path()).unwrap();
    assert!(matches!(
        app.stage(
            package,
            LaunchBudget::new(NonZeroU8::new(3).unwrap()),
            SpaceBudget {
                available_bytes: 0,
                reserve_bytes: 4096
            }
        ),
        Err(Error::LowSpace)
    ));
    assert_eq!(app.activation.revision(), revision);
    assert_eq!(app.prepare_launch(ram.path(), space()).unwrap().1, first);
    assert!(matches!(
        app.prepare_launch(
            ram.path(),
            SpaceBudget {
                available_bytes: 0,
                reserve_bytes: 4096
            }
        ),
        Err(Error::LowSpace)
    ));
}

#[test]
fn publisher_signed_expansion_and_digest_mistakes_never_publish_a_slot() {
    let root = tempfile::tempdir().unwrap();
    let app = open(root.path());
    for name in ["oversized-expansion", "wrong-executable-hash"] {
        assert!(matches!(
            app.verify_directory(fixture(name).path()),
            Err(Error::Package(PackageError::ArtifactMismatch))
        ));
    }
    assert_eq!(app.activation.status(), &Status::Uninstalled);
    assert!(!root.path().join("slots/a").exists());
    let shipping = tempfile::tempdir().unwrap();
    let app = Appliance::open(
        shipping.path(),
        prns_flash_manifest::PINNED_MINISIGN_PUBLIC_KEY,
        Board::ThinkNodeG4,
        budgets(),
        UnobservedWrites,
    )
    .unwrap();
    assert!(matches!(
        app.verify_directory(fixture("first").path()),
        Err(Error::Package(PackageError::Signature(_)))
    ));
}
