use super::{
    artifact::{Artifact, RecordedRun},
    *,
};
const MAX_ATTEMPTS: usize = 512;
pub fn same_failure(expected: &Failure, artifact: &Artifact) -> bool {
    artifact.runs.iter().all(|run| matches!(run, RecordedRun::Failed { failure, .. } if failure.class == expected.class && failure.action == expected.action && failure.detail == expected.detail))
}
pub fn reduce(original: &Artifact, mut execute: impl FnMut(&Case) -> Artifact) -> Artifact {
    let failure = original.failure().expect("failure to reduce");
    let mut best = original.clone();
    let mut attempts = 0;
    let mut width = best.case.actions.len().div_ceil(2);
    while width > 0 && attempts < MAX_ATTEMPTS {
        let mut index = 0;
        while index < best.case.actions.len() && attempts < MAX_ATTEMPTS {
            let mut candidate = best.case.clone();
            candidate
                .actions
                .drain(index..(index + width).min(candidate.actions.len()));
            if candidate.validate().is_err() {
                index += width;
                continue;
            }
            attempts += 1;
            let artifact = execute(&candidate);
            if same_failure(failure, &artifact) {
                best = artifact;
            } else {
                index += width;
            }
        }
        width /= 2;
    }
    best
}
