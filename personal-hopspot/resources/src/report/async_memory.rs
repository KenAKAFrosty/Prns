use crate::analysis;

use super::model::{
    AsyncMemoryIdentity, NamedFutureSizeIdentity, ScenarioFutureSizesIdentity,
    TaskPoolAccountingIdentity, TaskPoolAllocationIdentity,
};

pub(super) fn identity(
    analysis: analysis::AsyncMemoryAnalysis,
    future_sizes: &crate::semantic_futures::Measurements,
) -> AsyncMemoryIdentity {
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
        scenario_futures: ScenarioFutureSizesIdentity::Measured {
            futures: future_sizes
                .iter()
                .map(|future| NamedFutureSizeIdentity {
                    scenario: future.scenario.clone(),
                    bytes: future.bytes,
                })
                .collect(),
        },
    }
}
