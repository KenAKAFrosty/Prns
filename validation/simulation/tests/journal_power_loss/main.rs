#![allow(clippy::unwrap_used)]

mod flash;
mod model_tests;
mod restore;

use flash::{Cut, Error, Flash, Image, Operation, CAPACITY, PAGE};
use prns_core::persistence::{
    FlashArenaRange, FlashJournal, FlashJournalError, FlashJournalLayout,
    FlashJournalRecordKind as Kind, FlashJournalRestoreReport,
};

const LAYOUT: FlashJournalLayout = FlashJournalLayout::new(
    [0, PAGE as u32],
    [
        FlashArenaRange::new((PAGE * 2) as u32, (PAGE * 4) as u32),
        FlashArenaRange::new((PAGE * 4) as u32, CAPACITY as u32),
    ],
);
const SCRATCH: usize = 128;
type Records = Vec<(Kind, Vec<u8>)>;

async fn open(flash: Flash) -> (FlashJournal<Flash>, FlashJournalRestoreReport, Records) {
    let mut records = Vec::new();
    let (journal, report) = FlashJournal::open(flash, LAYOUT, &mut [0; SCRATCH], |record| {
        records.push((record.kind, record.payload.to_vec()));
    })
    .await
    .unwrap();
    (journal, report, records)
}

fn records(marker: u8) -> Records {
    vec![
        (Kind::RemoteControlControllerGrants, vec![marker; 19]),
        (Kind::DiscoveryGroupConfigurations, vec![marker + 1; 37]),
    ]
}

async fn compact(
    journal: &mut FlashJournal<Flash>,
    records: &Records,
) -> Result<(), FlashJournalError<Error>> {
    for sector in 0..journal.inactive_sector_count() {
        journal.erase_inactive_sector(sector).await?;
    }
    journal.begin_compaction()?;
    for (kind, payload) in records {
        journal.append_compacted(*kind, payload).await?;
    }
    journal.commit_compaction().await
}

async fn baseline() -> Image {
    let (mut journal, _, _) = open(Flash::boot(Image([0xff; CAPACITY]))).await;
    journal.initialize_empty().await.unwrap();
    for (kind, payload) in records(10) {
        journal.append(kind, &payload).await.unwrap();
    }
    compact(&mut journal, &records(20)).await.unwrap();
    journal.release().into_image()
}

#[tokio::test]
async fn every_compaction_cut_restores_one_complete_generation() {
    let image = baseline().await;
    let prior = records(20);
    let candidate = records(30);
    let (mut reference, report, restored) = open(Flash::boot(image.clone())).await;
    assert_eq!((report.active_epoch, restored), (Some(1), prior.clone()));
    compact(&mut reference, &candidate).await.unwrap();
    let reference = reference.release();
    let trace = reference.trace().to_vec();
    let commit_operation = trace
        .iter()
        .rposition(|event| matches!(event, Operation::Write { .. }))
        .unwrap();
    assert!(matches!(
        trace[commit_operation],
        Operation::Write { len: 4, .. }
    ));
    let (healthy, report, restored) = open(Flash::boot(reference.into_image())).await;
    assert_eq!(
        (report.active_epoch, restored),
        (Some(2), candidate.clone())
    );
    drop(healthy);

    // Opening is read-only; only the subsequent compaction is faulted.
    let (baseline_reader, _, _) = open(Flash::boot(image.clone())).await;
    let opening_operations = baseline_reader.release().trace().len();
    let mut cuts = 0;
    for (operation, event) in trace.iter().enumerate().skip(opening_operations) {
        let prefixes: Vec<_> = match event {
            Operation::Read { len, .. } => vec![0, *len],
            Operation::Write { len, .. } | Operation::Erase { len, .. } => (0..=*len).collect(),
        };
        for completed_bytes in prefixes {
            let cut = Cut {
                operation,
                completed_bytes,
            };
            let mut flash = Flash::boot(image.clone());
            flash.arm(cut);
            let (mut journal, _, _) = open(flash).await;
            assert!(
                matches!(
                    compact(&mut journal, &candidate).await,
                    Err(FlashJournalError::Flash(Error::PowerLost)
                        | FlashJournalError::CommitUnconfirmed {
                            error: Error::PowerLost,
                            ..
                        })
                ),
                "{cut:?}: {event:?}"
            );
            let interrupted = journal.release();
            assert_eq!(interrupted.trace(), &trace[..=operation], "{cut:?}");
            let (rebooted, report, restored) = open(Flash::boot(interrupted.into_image())).await;
            let committed = operation > commit_operation
                || (operation == commit_operation && completed_bytes == 4);
            let (epoch, expected_records) = if committed {
                (Some(2), candidate.clone())
            } else {
                (Some(1), prior.clone())
            };
            let expected = (
                FlashJournalRestoreReport {
                    active_epoch: epoch,
                    restored_records: 2,
                    warning: None,
                },
                expected_records,
            );
            let observed = (report, restored);
            assert_eq!(observed, expected, "{cut:?}: {event:?}");
            let (_, second_report, second_records) =
                open(Flash::boot(rebooted.release().into_image())).await;
            assert_eq!((second_report, second_records), observed, "{cut:?}");
            cuts += 1;
        }
    }
    assert!(
        cuts > PAGE * 2,
        "exercise byte-prefix cuts, not just call boundaries"
    );
    eprintln!(
        "verified {cuts} compaction cut points over {} I/O operations",
        trace.len() - opening_operations
    );
}
