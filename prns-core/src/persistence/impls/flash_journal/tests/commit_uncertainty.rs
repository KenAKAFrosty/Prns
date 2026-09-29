use super::*;

#[derive(Clone, Copy)]
enum CommitWrite {
    Complete,
    Torn,
    Suspend,
}

#[test]
fn compaction_confirmation_selects_only_the_exact_committed_arena_marker() {
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        baseline
            .append(FlashJournalRecordKind::RouteUpsert, b"prior")
            .await
            .unwrap();
        let bytes = baseline.release().bytes;
        for commit in [CommitWrite::Complete, CommitWrite::Torn] {
            let mut inner = FakeFlash::new();
            inner.bytes = bytes;
            let flash = UncertainCommitFlash {
                inner,
                commit: CommitWrite::Complete,
                commit_attempted: false,
                fail_readback: false,
            };
            let (mut journal, _) =
                FlashJournal::open(flash, LAYOUT, &mut [0; IO_CHUNK_LEN], |_| {})
                    .await
                    .unwrap();
            journal.begin_compaction().unwrap();
            let wrong_kind_at = journal.compaction_append_offset().unwrap();
            journal
                .append_compacted(FlashJournalRecordKind::RouteUpsert, b"candidate")
                .await
                .unwrap();
            let at = journal.compaction_append_offset().unwrap();
            journal.flash.commit = commit;
            journal.flash.commit_attempted = false;
            journal.flash.fail_readback = true;
            assert!(
                matches!(journal.commit_compaction().await, Err(FlashJournalError::CommitUnconfirmed { at: found, .. }) if found == at)
            );
            let image = journal.flash.inner.bytes;
            assert_eq!(
                journal.confirm_compaction_commit(at).await,
                Err(FlashJournalError::Flash(FakeError::Interrupted))
            );
            assert_eq!(journal.active_epoch(), Some(0));
            assert_eq!(
                journal
                    .append(FlashJournalRecordKind::RouteRemoval, b"blocked")
                    .await,
                Err(FlashJournalError::CompactionInProgress)
            );
            journal.flash.fail_readback = false;
            assert_eq!(
                journal.confirm_compaction_commit(u32::MAX).await,
                Err(FlashJournalError::PayloadTooLarge)
            );
            assert_eq!(
                journal.confirm_compaction_commit(wrong_kind_at).await,
                Err(FlashJournalError::VerificationFailed)
            );
            assert_eq!(journal.active_epoch(), Some(0));
            assert_eq!(
                journal.confirm_compaction_commit(at).await,
                Ok(match commit {
                    CommitWrite::Complete => FlashJournalCommitResolution::Committed,
                    CommitWrite::Torn => FlashJournalCommitResolution::NotCommitted,
                    CommitWrite::Suspend => unreachable!(),
                })
            );
            assert_eq!(journal.flash.inner.bytes, image);
            let expected = match commit {
                CommitWrite::Complete => {
                    assert_eq!(journal.active_epoch(), Some(1));
                    assert_eq!(
                        journal.confirm_compaction_commit(at).await,
                        Err(FlashJournalError::NoCompaction)
                    );
                    journal.flash.commit = CommitWrite::Complete;
                    journal
                        .append(FlashJournalRecordKind::RouteRemoval, b"after")
                        .await
                        .unwrap();
                    vec![
                        (FlashJournalRecordKind::RouteUpsert, b"candidate".to_vec()),
                        (FlashJournalRecordKind::RouteRemoval, b"after".to_vec()),
                    ]
                }
                CommitWrite::Torn => {
                    assert_eq!(journal.active_epoch(), Some(0));
                    journal.abort_compaction();
                    assert_eq!(
                        journal
                            .append(FlashJournalRecordKind::RouteRemoval, b"blocked")
                            .await,
                        Err(FlashJournalError::ArenaFull)
                    );
                    vec![(FlashJournalRecordKind::RouteUpsert, b"prior".to_vec())]
                }
                CommitWrite::Suspend => unreachable!(),
            };
            let (_, _, records) = open(journal.release().inner).await;
            assert_eq!(records, expected);
        }
    });
}

struct UncertainCommitFlash {
    inner: FakeFlash,
    commit: CommitWrite,
    commit_attempted: bool,
    fail_readback: bool,
}

impl ErrorType for UncertainCommitFlash {
    type Error = FakeError;
}

impl ReadNorFlash for UncertainCommitFlash {
    const READ_SIZE: usize = FakeFlash::READ_SIZE;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if self.commit_attempted && self.fail_readback {
            return Err(FakeError::Interrupted);
        }
        self.inner.read(offset, bytes).await
    }

    fn capacity(&self) -> usize {
        self.inner.capacity()
    }
}

impl NorFlash for UncertainCommitFlash {
    const WRITE_SIZE: usize = FakeFlash::WRITE_SIZE;
    const ERASE_SIZE: usize = FakeFlash::ERASE_SIZE;

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        if bytes != COMMIT_WORD.to_le_bytes() {
            return self.inner.write(offset, bytes).await;
        }
        self.commit_attempted = true;
        match self.commit {
            CommitWrite::Complete => self.inner.write(offset, bytes).await?,
            CommitWrite::Torn => self.inner.bytes[offset as usize] &= bytes[0],
            CommitWrite::Suspend => {
                self.inner.write(offset, bytes).await?;
                core::future::pending::<()>().await;
            }
        }
        Err(FakeError::Interrupted)
    }

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.inner.erase(from, to).await
    }
}

#[test]
fn cancelled_appends_and_compaction_cannot_reprogram_an_uncertain_tail() {
    enum WritePhase {
        Append,
        CompactAppend,
        CompactCommit,
    }
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        let bytes = baseline.release().bytes;
        for phase in [
            WritePhase::Append,
            WritePhase::CompactAppend,
            WritePhase::CompactCommit,
        ] {
            let mut flash = FakeFlash::new();
            flash.bytes = bytes;
            let flash = UncertainCommitFlash {
                inner: flash,
                commit: CommitWrite::Suspend,
                commit_attempted: false,
                fail_readback: false,
            };
            let (mut journal, _) =
                FlashJournal::open(flash, LAYOUT, &mut [0; IO_CHUNK_LEN], |_| {})
                    .await
                    .unwrap();
            if !matches!(phase, WritePhase::Append) {
                journal.begin_compaction().unwrap();
            }
            let candidate_at = journal.active_append_offset().unwrap();
            {
                let mut write = core::pin::pin!(async {
                    match phase {
                        WritePhase::CompactCommit => journal.commit_compaction().await,
                        WritePhase::CompactAppend => {
                            journal
                                .append_compacted(FlashJournalRecordKind::RouteUpsert, b"candidate")
                                .await
                        }
                        WritePhase::Append => {
                            journal
                                .append(FlashJournalRecordKind::RouteUpsert, b"candidate")
                                .await
                        }
                    }
                });
                let mut context = core::task::Context::from_waker(core::task::Waker::noop());
                assert!(core::future::Future::poll(write.as_mut(), &mut context).is_pending());
            }
            let image = journal.flash.inner.bytes;
            if matches!(phase, WritePhase::Append) {
                assert_eq!(
                    journal
                        .confirm_append(
                            candidate_at,
                            FlashJournalRecordKind::RouteUpsert,
                            b"candidate"
                        )
                        .await,
                    Ok(FlashJournalCommitResolution::Committed)
                );
                assert_eq!(journal.flash.inner.bytes, image);
            }
            if !matches!(phase, WritePhase::Append) {
                assert_eq!(
                    journal
                        .append_compacted(FlashJournalRecordKind::RouteRemoval, b"next")
                        .await,
                    Err(FlashJournalError::ArenaFull)
                );
                assert_eq!(
                    journal.commit_compaction().await,
                    Err(FlashJournalError::ArenaFull)
                );
            } else {
                assert_eq!(
                    journal
                        .append(FlashJournalRecordKind::RouteRemoval, b"next")
                        .await,
                    Err(FlashJournalError::ArenaFull)
                );
            }
            assert_eq!(journal.flash.inner.bytes, image);
            if matches!(phase, WritePhase::CompactCommit) {
                journal.abort_compaction();
                assert_eq!(
                    journal
                        .append(FlashJournalRecordKind::RouteRemoval, b"old epoch")
                        .await,
                    Err(FlashJournalError::ArenaFull)
                );
                assert_eq!(journal.flash.inner.bytes, image);
            }
            let (_, _, records) = open(journal.release().inner).await;
            assert_eq!(
                records,
                if !matches!(phase, WritePhase::Append) {
                    vec![]
                } else {
                    vec![(FlashJournalRecordKind::RouteUpsert, b"candidate".to_vec())]
                }
            );
        }
    });
}

#[test]
fn uncertain_commit_is_successful_only_when_readback_proves_the_commit() {
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        baseline
            .append(FlashJournalRecordKind::RouteUpsert, b"prior")
            .await
            .unwrap();
        let candidate_at = baseline.active.as_ref().unwrap().append_at;
        let bytes = baseline.release().bytes;
        for commit in [CommitWrite::Complete, CommitWrite::Torn] {
            for fail_readback in [false, true] {
                let mut flash = FakeFlash::new();
                flash.bytes = bytes;
                let flash = UncertainCommitFlash {
                    inner: flash,
                    commit,
                    commit_attempted: false,
                    fail_readback,
                };
                let (mut journal, _) =
                    FlashJournal::open(flash, LAYOUT, &mut [0; IO_CHUNK_LEN], |_| {})
                        .await
                        .unwrap();
                let result = journal
                    .append(
                        FlashJournalRecordKind::RemoteControlControllerGrants,
                        b"candidate",
                    )
                    .await;
                let expected = if matches!(commit, CommitWrite::Complete) && !fail_readback {
                    Ok(())
                } else if fail_readback {
                    Err(FlashJournalError::CommitUnconfirmed {
                        at: candidate_at,
                        error: FakeError::Interrupted,
                    })
                } else {
                    Err(FlashJournalError::Flash(FakeError::Interrupted))
                };
                assert_eq!(result, expected);
                if result.is_ok() {
                    journal
                        .append(FlashJournalRecordKind::RouteRemoval, b"next")
                        .await
                        .unwrap();
                } else {
                    let before_retry = journal.flash.inner.bytes;
                    assert_eq!(
                        journal
                            .append(FlashJournalRecordKind::RouteRemoval, b"different")
                            .await,
                        Err(FlashJournalError::ArenaFull)
                    );
                    assert_eq!(journal.flash.inner.bytes, before_retry);
                }
                let mut expected_records =
                    vec![(FlashJournalRecordKind::RouteUpsert, b"prior".to_vec())];
                if matches!(commit, CommitWrite::Complete) {
                    expected_records.push((
                        FlashJournalRecordKind::RemoteControlControllerGrants,
                        b"candidate".to_vec(),
                    ));
                    if !fail_readback {
                        expected_records
                            .push((FlashJournalRecordKind::RouteRemoval, b"next".to_vec()));
                    }
                }
                let (_, report, records) = open(journal.release().inner).await;
                assert_eq!(report.warning, None);
                assert_eq!(records, expected_records);
            }
        }
    });
}

#[test]
fn uncertain_append_confirmation_is_read_only_exact_and_retryable() {
    embassy_futures::block_on(async {
        let (mut baseline, _, _) = open(FakeFlash::new()).await;
        baseline.initialize_empty().await.unwrap();
        let bytes = baseline.release().bytes;
        for commit in [CommitWrite::Complete, CommitWrite::Torn] {
            let mut inner = FakeFlash::new();
            inner.bytes = bytes;
            let flash = UncertainCommitFlash {
                inner,
                commit,
                commit_attempted: false,
                fail_readback: true,
            };
            let (mut journal, _) =
                FlashJournal::open(flash, LAYOUT, &mut [0; IO_CHUNK_LEN], |_| {})
                    .await
                    .unwrap();
            let kind = FlashJournalRecordKind::RemoteControlControllerGrants;
            let candidate = b"candidate";
            let Err(FlashJournalError::CommitUnconfirmed { at, .. }) =
                journal.append(kind, candidate).await
            else {
                panic!("commit confirmation must remain unresolved");
            };
            let image = journal.flash.inner.bytes;
            assert_eq!(
                journal.confirm_append(at, kind, candidate).await,
                Err(FlashJournalError::Flash(FakeError::Interrupted))
            );
            journal.flash.fail_readback = false;
            if matches!(commit, CommitWrite::Complete) {
                assert_eq!(
                    journal.confirm_append(at, kind, b"different").await,
                    Err(FlashJournalError::VerificationFailed)
                );
                assert_eq!(
                    journal
                        .confirm_append(
                            at,
                            FlashJournalRecordKind::RemoteControlTargetAccesses,
                            candidate
                        )
                        .await,
                    Err(FlashJournalError::VerificationFailed)
                );
            }
            for _ in 0..2 {
                assert_eq!(
                    journal.confirm_append(at, kind, candidate).await,
                    Ok(match commit {
                        CommitWrite::Complete => FlashJournalCommitResolution::Committed,
                        CommitWrite::Torn => FlashJournalCommitResolution::NotCommitted,
                        CommitWrite::Suspend => unreachable!(),
                    })
                );
                assert_eq!(journal.flash.inner.bytes, image);
            }
        }
    });
}
