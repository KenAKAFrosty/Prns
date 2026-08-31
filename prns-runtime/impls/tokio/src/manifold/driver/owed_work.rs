use crate::engine::{CryptoOwed, OwedWork};
use crate::manifold::Host;
use crate::routing::links::resources::send::ResourceBuildPlan;
use crate::routing::links::resources::ResourceMetadata;

use super::crypto_pool::{
    run_crypto_job_inline, CryptoJob, CryptoPool, CryptoResult, ResourceBuildJob,
    ResourceDecompressionJob,
};
use super::host_protocol::{HostResourceMetadata, HostResourcePayload};

struct PendingResourceBuild {
    plan: ResourceBuildPlan,
    data: HostResourcePayload,
    compressed_candidate: Option<HostResourcePayload>,
    metadata: HostResourceMetadata,
}

enum PendingJob {
    Ready(CryptoJob),
    ResourceBuild(PendingResourceBuild),
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

    pub(super) fn push(&mut self, work: OwedWork<'_>) {
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
            };
            match pool {
                Some(pool) => pool.submit(job),
                None => inline.push(run_crypto_job_inline(job)),
            }
        }
        true
    }
}
