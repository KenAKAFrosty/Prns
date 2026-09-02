use crate::engine::{CryptoOwed, OpenedResourceSpan, OwedWork, ResourceOpenOwed};
use crate::manifold::Host;
use crate::routing::links::resources::send::{ResourceBuildPlan, ResourceSealPlan};
use crate::routing::links::resources::ResourceMetadata;

use super::crypto_pool::{
    run_crypto_job_inline, CryptoJob, CryptoPool, CryptoResult, OpenSpanJob, OpenedSpanResult,
    ResourceBuildJob, ResourceDecompressionJob,
};
use super::host_protocol::{HostResourceMetadata, HostResourcePayload};

struct PendingResourceBuild {
    plan: ResourceBuildPlan,
    data: HostResourcePayload,
    compressed_candidate: Option<HostResourcePayload>,
    metadata: HostResourceMetadata,
}

struct PendingResourceSeal {
    plan: ResourceSealPlan,
    plaintext: Vec<u8>,
}

// Keeping completed inline work by value avoids adding an allocation to the zero-copy open path.
#[allow(clippy::large_enum_variant)]
enum PendingJob {
    Ready(CryptoJob),
    Completed(CryptoResult),
    ResourceBuild(PendingResourceBuild),
    ResourceSeal(PendingResourceSeal),
}

/// Runtime-owned work waiting for the current engine call to release its borrows.
///
/// This is an ordinary manifold-local queue, not a synchronization primitive. The manifold drains
/// it before parking. A worker-bound resource command moves its original payload grant through
/// [`push_resource_build`](Self::push_resource_build). Other work-producing entry points provide
/// their own typed materializers, so this hot resource path never needs a borrowed-data fallback.
pub(super) struct PendingOwedWork {
    jobs: std::vec::Vec<PendingJob>,
}

impl PendingOwedWork {
    pub(super) const fn new() -> Self {
        Self {
            jobs: std::vec::Vec::new(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.jobs.len()
    }

    pub(super) fn push(&mut self, work: OwedWork<'_>, pool: Option<&CryptoPool>) {
        match work {
            OwedWork::Crypto(owed) => self.push_crypto(owed),
            OwedWork::ResourceBuild(owed) => {
                let body = owed.body();
                let metadata = match body.metadata {
                    ResourceMetadata::None => HostResourceMetadata::None,
                    ResourceMetadata::Packed(packed) => {
                        HostResourceMetadata::Packed(packed.to_vec().into())
                    }
                    ResourceMetadata::SentInFirstSegment { packed_len } => {
                        HostResourceMetadata::SentInFirstSegment { packed_len }
                    }
                };
                let data = body.data.to_vec().into();
                let compressed_candidate = body
                    .compressed_candidate
                    .map(|candidate| candidate.to_vec().into());
                let plan = owed.into_plan();
                self.push_resource_build(plan, data, compressed_candidate, metadata);
            }
            OwedWork::ResourceSeal(owed) => {
                let plaintext = owed.workspace().to_vec();
                let plan = owed.into_plan();
                self.jobs
                    .push(PendingJob::ResourceSeal(PendingResourceSeal {
                        plan,
                        plaintext,
                    }));
            }
            OwedWork::ResourceOpen(owed) => self.push_resource_open(owed, pool),
            OwedWork::WholeResourceOpen(owed) => {
                self.jobs.push(PendingJob::Completed(
                    CryptoResult::WholeResourceOpenUnavailable {
                        reservation: owed.plan().reservation(),
                    },
                ));
            }
            OwedWork::ResourceDecompression(owed) => {
                self.jobs
                    .push(PendingJob::Ready(CryptoJob::DecompressResource(Box::new(
                        ResourceDecompressionJob {
                            link_id: owed.link_id,
                            hash: owed.hash,
                            stream: owed.stream.to_vec(),
                            uncompressed_data_bytes: owed.uncompressed_data_bytes,
                        },
                    ))));
            }
        }
    }

    pub(super) fn push_resource_open(
        &mut self,
        owed: ResourceOpenOwed<'_>,
        pool: Option<&CryptoPool>,
    ) {
        let offload =
            owed.other_transfers_in_flight && pool.is_some_and(|pool| pool.has_queue_capacity(1));
        if offload {
            let ResourceOpenOwed {
                link_id,
                hash,
                span_start,
                state,
                bytes,
                other_transfers_in_flight: _,
            } = owed;
            self.jobs
                .push(PendingJob::Ready(CryptoJob::OpenSpan(Box::new(
                    OpenSpanJob {
                        link_id,
                        hash,
                        span_start,
                        state,
                        bytes: bytes.to_vec(),
                    },
                ))));
        } else {
            let completed = owed.fulfill_inline();
            let opened = match completed.opened {
                OpenedResourceSpan::InPlace { byte_len } => OpenedSpanResult::InPlace { byte_len },
                OpenedResourceSpan::Returned(bytes) => OpenedSpanResult::Owned(bytes.to_vec()),
            };
            self.jobs
                .push(PendingJob::Completed(CryptoResult::SpanOpened {
                    link_id: completed.link_id,
                    hash: completed.hash,
                    span_start: completed.span_start,
                    state: completed.state,
                    opened,
                }));
        }
    }

    pub(super) fn push_crypto(&mut self, owed: CryptoOwed) {
        self.jobs
            .push(PendingJob::Ready(CryptoJob::from_owed(owed)));
    }

    pub(super) fn push_resource_build(
        &mut self,
        plan: ResourceBuildPlan,
        data: HostResourcePayload,
        compressed_candidate: Option<HostResourcePayload>,
        metadata: HostResourceMetadata,
    ) {
        self.jobs
            .push(PendingJob::ResourceBuild(PendingResourceBuild {
                plan,
                data,
                compressed_candidate,
                metadata,
            }));
    }

    pub(super) fn dispatch<H: Host>(
        &mut self,
        host: &mut H,
        pool: Option<&CryptoPool>,
        inline: &mut std::vec::Vec<CryptoResult>,
    ) -> bool {
        if self.jobs.is_empty() {
            return false;
        }
        for pending in self.jobs.drain(..) {
            let job = match pending {
                PendingJob::Ready(job) => job,
                PendingJob::Completed(result) => {
                    inline.push(result);
                    continue;
                }
                PendingJob::ResourceBuild(pending) => {
                    let mut seal_iv = [0u8; 16];
                    host.fill_random(&mut seal_iv);
                    let mut nonces = [[0u8; crate::routing::links::resources::RESOURCE_NONCE_LEN];
                        crate::routing::links::resources::build_outgoing::SALT_REROLL_CAP + 1];
                    for nonce in &mut nonces {
                        host.fill_random(nonce);
                    }
                    CryptoJob::BuildResource(Box::new(ResourceBuildJob {
                        plan: pending.plan,
                        data: pending.data,
                        compressed_candidate: pending.compressed_candidate,
                        metadata: pending.metadata,
                        seal_iv,
                        nonces,
                    }))
                }
                PendingJob::ResourceSeal(pending) => {
                    let mut seal_iv = [0u8; 16];
                    host.fill_random(&mut seal_iv);
                    let mut salts = [[0u8; crate::routing::links::resources::RESOURCE_NONCE_LEN];
                        crate::routing::links::resources::build_outgoing::SALT_REROLL_CAP];
                    for salt in &mut salts {
                        host.fill_random(salt);
                    }
                    CryptoJob::SealStaged(Box::new(super::crypto_pool::StagedSealJob {
                        plan: pending.plan,
                        plaintext: pending.plaintext,
                        seal_iv,
                        salts,
                    }))
                }
            };
            match pool {
                Some(pool) => pool.submit(job),
                None => inline.push(run_crypto_job_inline(job)),
            }
        }
        true
    }
}
