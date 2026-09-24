use super::*;

#[macro_use]
mod partitions;
use partitions::FaultPartition;

mod cancellation;
mod compaction_budget;
mod compaction_commit;
mod compaction_steps;
mod continuation;
mod flash;
mod grants;
mod names;
mod settlement;
mod transaction;
use flash::{Control, Cut, Flash, Operation};

const WRITE_TIME: InstantMillis = InstantMillis(100);
const MAX_PROGRESS_STEPS: usize = 32;

#[derive(Clone, Copy, Debug)]
enum Campaign {
    Append,
    CompactThenAppend,
}

async fn baseline(campaign: Campaign) -> [u8; CAPACITY] {
    let (mut journal, _) = FlashJournal::open(
        TestFlash::new(),
        LAYOUT,
        &mut [0; RECORD_SCRATCH_LEN],
        |_| {},
    )
    .await
    .unwrap();
    journal.initialize_empty().await.unwrap();
    let mut payload = [0; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN];
    let len = discovery_group_snapshot("confirmed").encode_into_max(&mut payload);
    journal
        .append(
            FlashJournalRecordKind::DiscoveryGroupConfigurations,
            &payload[..len],
        )
        .await
        .unwrap();
    if matches!(campaign, Campaign::CompactThenAppend) {
        let mut full = false;
        for _ in 0..32 {
            match journal
                .append(
                    FlashJournalRecordKind::DiscoveryGroupConfigurations,
                    &payload[..len],
                )
                .await
            {
                Ok(()) => {}
                Err(FlashJournalError::ArenaFull) => {
                    full = true;
                    break;
                }
                Err(error) => panic!("unexpected baseline error: {error:?}"),
            }
        }
        assert!(full, "bounded arena fill");
    }
    journal.release().bytes
}

struct Outcome {
    image: [u8; CAPACITY],
    trace: Vec<Operation>,
    completion: Completion,
    published: DiscoveryGroupConfigurationSnapshot,
    write_failures: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Completion {
    Settled(Result<(), EmbeddedPersistenceFailure>),
    ConfirmationPending,
    PowerRemoved,
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    ReportError(Cut),
    RemovePower(Cut),
}

async fn write(image: [u8; CAPACITY], fault: Fault) -> Outcome {
    let exchange = DiscoveryGroupConfigurationStoreExchange::new();
    let control = Rc::new(RefCell::new(Control::new()));
    let write_failures = Cell::new(0);
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        Flash::boot(image, control.clone()), LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
        FixedRouteSnapshotKeys::new(), |event| {
            if matches!(event, EmbeddedPersistenceDiagnostic::WriteFailed { .. }) {
                write_failures.set(write_failures.get() + 1);
            }
        }, &exchange,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    assert_eq!(
        exchange.restored_now(),
        Some(discovery_group_snapshot("confirmed"))
    );
    match fault {
        Fault::None => control.borrow_mut().arm(None),
        Fault::ReportError(cut) => control.borrow_mut().arm(Some(cut)),
        Fault::RemovePower(cut) => control.borrow_mut().remove_power_at(cut),
    }
    let interface =
        crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
    let candidate = discovery_group_snapshot("candidate");
    let change = DiscoveryGroupConfigurationChange::upsert(
        interface,
        *candidate.groups_for(interface).unwrap(),
    );
    let mut completion = core::pin::pin!(exchange.store(change));
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
    let mut result = None;
    for _ in 0..MAX_PROGRESS_STEPS {
        let progress = {
            let mut progress = core::pin::pin!(owner.progress(&mut engine, WRITE_TIME));
            core::future::Future::poll(progress.as_mut(), &mut context)
        };
        if progress.is_pending() {
            assert!(matches!(fault, Fault::RemovePower(_)));
            assert!(control.borrow().power_removed());
            assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
            result = Some(Completion::PowerRemoved);
            break;
        }
        if owner.pending_confirmation.is_some() {
            assert!(core::future::Future::poll(completion.as_mut(), &mut context).is_pending());
            result = Some(Completion::ConfirmationPending);
            break;
        }
        if let core::task::Poll::Ready(value) =
            core::future::Future::poll(completion.as_mut(), &mut context)
        {
            result = Some(Completion::Settled(value));
            break;
        }
    }
    let completion = result.expect("bounded persistence settlement");
    let published = exchange.restored_now().unwrap();
    let image = owner.journal.take().unwrap().release().into_image();
    let trace = control.borrow().trace.clone();
    Outcome {
        image,
        trace,
        completion,
        published,
        write_failures: write_failures.get(),
    }
}

async fn reboot(image: [u8; CAPACITY]) -> DiscoveryGroupConfigurationSnapshot {
    let exchange = DiscoveryGroupConfigurationStoreExchange::new();
    let mut flash = TestFlash::new();
    flash.bytes = image;
    let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _>::with_discovery_group_store(
        flash, LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0)),
        FixedRouteSnapshotKeys::new(), (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &exchange,
    );
    let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = available_remote_control(&mut engine);
    let report = owner
        .restore(&mut engine, &mut remote, InstantMillis(0))
        .await;
    assert!(report.discovery_group_configuration_restored);
    assert_eq!(report.discovery_group_configuration_refused_count, 0);
    assert!(!owner.state_not_saved());
    exchange.restored_now().unwrap()
}

#[test]
fn queued_group_append_recovers_at_every_torn_io_boundary() {
    campaign(Campaign::Append, FaultPartition::All);
}

partitioned_campaign!(
    queued_group_compaction_recovers_at_every_torn_io_boundary,
    campaign,
    Campaign::CompactThenAppend,
);

fn campaign(campaign: Campaign, partition: FaultPartition) {
    embassy_futures::block_on(async {
        let image = baseline(campaign).await;
        let reference = write(image, Fault::None).await;
        assert_eq!(
            reference
                .trace
                .iter()
                .any(|op| matches!(op, Operation::Erase { .. })),
            matches!(campaign, Campaign::CompactThenAppend)
        );
        let confirmed = discovery_group_snapshot("confirmed");
        let candidate = discovery_group_snapshot("candidate");
        assert_eq!(reference.completion, Completion::Settled(Ok(())));
        assert_eq!(reference.write_failures, 0);
        assert_eq!(reference.published, candidate);
        assert_eq!(reboot(reference.image).await, candidate);
        let commit = reference
            .trace
            .iter()
            .rposition(|op| matches!(op, Operation::Write { .. }))
            .unwrap();
        assert!(matches!(
            reference.trace[commit],
            Operation::Write { len: 4, .. }
        ));
        let mut cuts = 0;
        let mut boundary = 0;
        let mut deferred = 0;
        for (operation, event) in reference.trace.iter().enumerate() {
            let prefixes: Vec<_> = match event {
                Operation::Read { len, .. } => std::vec![0, *len],
                Operation::Write { len, .. } => (0..=*len).collect(),
                Operation::Erase { len, .. } => (0..=*len).collect(),
            };
            for completed_bytes in prefixes {
                let selected = partition.includes(boundary);
                boundary += 1;
                if !selected {
                    continue;
                }
                let cut = Cut {
                    operation,
                    completed_bytes,
                };
                let outcome = write(image, Fault::ReportError(cut)).await;
                assert_eq!(outcome.trace, reference.trace[..=operation], "{cut:?}");
                assert_eq!(
                    outcome.completion,
                    if operation >= commit {
                        Completion::ConfirmationPending
                    } else {
                        Completion::Settled(Err(EmbeddedPersistenceFailure::Flash))
                    },
                    "{cut:?}"
                );
                assert_eq!(outcome.published, confirmed, "{cut:?}");
                assert!(outcome.write_failures > 0, "{cut:?}");
                let durable = if operation > commit || (operation == commit && completed_bytes == 4)
                {
                    candidate
                } else {
                    confirmed
                };
                for _ in 0..2 {
                    assert_eq!(reboot(outcome.image).await, durable, "{cut:?}");
                }
                let removed = write(image, Fault::RemovePower(cut)).await;
                assert_eq!(removed.trace, outcome.trace, "{cut:?}");
                assert_eq!(removed.image, outcome.image, "{cut:?}");
                assert_eq!(removed.completion, Completion::PowerRemoved, "{cut:?}");
                assert_eq!(removed.write_failures, 0, "{cut:?}");
                assert_eq!(removed.published, confirmed, "{cut:?}");
                for _ in 0..2 {
                    assert_eq!(reboot(removed.image).await, durable, "{cut:?}");
                }
                if matches!(
                    continuation::verify(removed.image, durable).await,
                    continuation::Recovery::AfterCooldown
                ) {
                    deferred += 1;
                }
                cuts += 1;
            }
        }
        match campaign {
            Campaign::Append => assert_eq!(deferred, 0),
            Campaign::CompactThenAppend => assert!(deferred > 0 && deferred < cuts),
        }
        assert!(cuts > 0);
        std::eprintln!(
            "verified {cuts} queued-owner {campaign:?} cuts with both fault modes and follow-up writes ({deferred} cooldowns)"
        );
    });
}
