use crate::analysis;

use super::model::{
    AsyncMemoryIdentity, FutureSizeUnavailableReasonIdentity, ScenarioFutureSizesIdentity,
    TaskPoolAccountingIdentity, TaskPoolAllocationIdentity,
};

pub(super) fn identity(analysis: analysis::AsyncMemoryAnalysis) -> AsyncMemoryIdentity {
    AsyncMemoryIdentity {
        task_pool_accounting: TaskPoolAccountingIdentity::IncludedInStaticRam,
        task_pool_bytes: analysis.task_pool_bytes,
        task_pools: analysis
            .task_pools
            .into_iter()
            .map(|pool| TaskPoolAllocationIdentity {
                task: pool.task,
                address: pool.address,
                bytes: pool.bytes,
                section: pool.section,
            })
            .collect(),
        scenario_futures: ScenarioFutureSizesIdentity::Unavailable {
            reason: FutureSizeUnavailableReasonIdentity::SemanticHarnessNotProduced,
        },
    }
}
