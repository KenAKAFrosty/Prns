use super::*;
use artifact::Artifact;

pub(super) fn same_failure(expected: &Failure, candidate: &Artifact) -> bool {
    match expected.class {
        FailureClass::Replay => candidate.failure().is_some_and(|failure| failure.class == FailureClass::Replay),
        FailureClass::Expectation | FailureClass::Runtime | FailureClass::InvalidInput => candidate.runs.iter().all(|run| matches!(run, Run::Failed { failure, .. } if failure.class == expected.class && failure.operation == expected.operation && failure.detail == expected.detail)),
    }
}
pub(super) fn reduce(original: &Artifact, mut run: impl FnMut(&Case) -> Artifact) -> Artifact {
    let failure = original.failure().expect("failed original");
    let mut reduced = original.case.clone();
    let mut best = Artifact {
        version: original.version,
        case: original.case.clone(),
        runs: original.runs.clone(),
    };
    let mut width = reduced.actions.len().div_ceil(2);
    while width > 0 {
        let mut index = 0;
        while index < reduced.actions.len() {
            let mut candidate = reduced.clone();
            candidate
                .actions
                .drain(index..(index + width).min(candidate.actions.len()));
            if candidate.validate().is_err() {
                index += width;
                continue;
            }
            let artifact = run(&candidate);
            if same_failure(&failure, &artifact) {
                reduced = candidate;
                best = artifact;
            } else {
                index += width;
            }
        }
        width /= 2;
    }
    best
}

#[test]
fn reduction_preserves_failure_prerequisites_and_original_evidence() {
    let case = Case {
        version: case::VERSION,
        seed: 7,
        runtimes: adapter::PAIRS[0].clone(),
        family: case::Family::Authority,
        actions: vec![
            Action::Describe,
            Action::Permissions(Permission::Absent),
            Action::Echo { marker: 1, len: 0 },
            Action::Build,
            Action::Reconnect,
        ],
    };
    let simulate = |case: &Case| {
        let failure_stage = case
            .actions
            .iter()
            .position(|action| matches!(action, Action::Build));
        let report = Report {
            observations: vec![],
            wire: vec![],
            persistence: [serde_json::Value::Null, serde_json::Value::Null],
        };
        let run = match failure_stage {
            Some(stage)
                if case.actions[..stage].contains(&Action::Permissions(Permission::Absent)) =>
            {
                Run::Failed {
                    failure: Failure {
                        class: FailureClass::Expectation,
                        stage,
                        operation: Some(Action::Build),
                        detail: "test admission violation".to_owned(),
                    },
                    report: Some(report),
                }
            }
            _ => Run::Passed(report),
        };
        Artifact {
            version: case::VERSION,
            case: case.clone(),
            runs: [run.clone(), run],
        }
    };
    let original = simulate(&case);
    let evidence = serde_json::to_vec(&original).expect("original evidence");
    let reduced = reduce(&original, simulate);
    assert_eq!(
        reduced.case.actions,
        [Action::Permissions(Permission::Absent), Action::Build]
    );
    assert_eq!(
        serde_json::to_vec(&original).expect("unchanged evidence"),
        evidence
    );
    assert_eq!(
        reduced.failure().expect("preserved failure").class,
        FailureClass::Expectation
    );
}
