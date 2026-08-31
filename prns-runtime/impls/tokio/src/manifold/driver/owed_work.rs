use crate::manifold::Host;
use crate::routing::links::resources::send::ResourceBuildPlan;

use super::crypto_pool::{
    run_resource_build_job, CryptoJob, CryptoPool, CryptoResult, ResourceBuildJob,
};
use super::host_protocol::{HostResourceMetadata, HostResourcePayload};

struct PendingResourceBuild {
    plan: ResourceBuildPlan,
    data: HostResourcePayload,
    compressed_candidate: Option<HostResourcePayload>,
    metadata: HostResourceMetadata,
}

/// Runtime-owned work waiting for the current engine call to release its borrows.
///
/// This is an ordinary manifold-local queue, not a synchronization primitive. The manifold drains
/// it before parking. A worker-bound resource command moves its original payload grant through
/// [`push_resource_build`](Self::push_resource_build). Other work-producing entry points provide
/// their own typed materializers, so this hot resource path never needs a borrowed-data fallback.
pub(super) struct PendingOwedWork {
    resource_builds: std::vec::Vec<PendingResourceBuild>,
}

impl PendingOwedWork {
    pub(super) const fn new() -> Self {
        Self {
            resource_builds: std::vec::Vec::new(),
        }
    }

    pub(super) fn push_resource_build(
        &mut self,
        plan: ResourceBuildPlan,
        data: HostResourcePayload,
        compressed_candidate: Option<HostResourcePayload>,
        metadata: HostResourceMetadata,
    ) {
        self.resource_builds.push(PendingResourceBuild {
            plan,
            data,
            compressed_candidate,
            metadata,
        });
    }

    pub(super) fn dispatch<H: Host>(
        &mut self,
        host: &mut H,
        pool: Option<&CryptoPool>,
        inline: &mut std::vec::Vec<CryptoResult>,
    ) -> bool {
        if self.resource_builds.is_empty() {
            return false;
        }
        for pending in self.resource_builds.drain(..) {
            let mut seal_iv = [0u8; 16];
            host.fill_entropy(&mut seal_iv);
            let mut nonces = [[0u8; crate::routing::links::resources::RESOURCE_NONCE_LEN];
                crate::routing::links::resources::build_outgoing::SALT_REROLL_CAP + 1];
            for nonce in &mut nonces {
                host.fill_entropy(nonce);
            }
            let job = ResourceBuildJob {
                plan: pending.plan,
                data: pending.data,
                compressed_candidate: pending.compressed_candidate,
                metadata: pending.metadata,
                seal_iv,
                nonces,
            };
            match pool {
                Some(pool) => pool.submit(CryptoJob::BuildResource(Box::new(job))),
                None => inline.push(run_resource_build_job(job)),
            }
        }
        true
    }
}
