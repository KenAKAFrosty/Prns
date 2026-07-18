mod bucketed;
mod deadline_heap;

pub(crate) use bucketed::HeapTemporalIndex;
pub(crate) use deadline_heap::HeapDeadlineIndex;

const LINEAR_CULL_MIN_CANDIDATES: u64 = 4_096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum IndexReadiness {
    Ready,
    Invalid,
    LinearFallback,
}

#[cfg(test)]
mod tests;
