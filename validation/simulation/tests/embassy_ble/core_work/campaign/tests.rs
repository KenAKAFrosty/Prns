use super::*;
#[test]
fn routine_core_work_campaign() {
    subprocess::campaign(case::routine(), "routine");
}
#[test]
#[ignore = "extended core qualification"]
fn extended_core_work_campaign() {
    subprocess::campaign(case::extended(), "extended");
}
#[test]
fn cases_have_explicit_budgets_and_valid_atomic_reductions() {
    assert_eq!(case::routine().len(), 120);
    assert_eq!(case::extended().len(), 1920);
    for case in case::routine().into_iter().chain(case::extended()) {
        assert_eq!(case.validate(), Ok(()));
        let bytes = serde_json::to_vec(&case).expect("case schema");
        assert_eq!(
            serde_json::from_slice::<Case>(&bytes).expect("case round trip"),
            case
        );
        for index in 0..case.actions.len() {
            let mut reduced = case.clone();
            reduced.actions.remove(index);
            assert_eq!(reduced.validate(), Ok(()));
        }
    }
}
#[test]
fn all_atomic_workloads_replay_across_the_execution_matrix() {
    use super::super::profile;
    for profile in profile::all() {
        let case = Case {
            version: case::VERSION,
            seed: 42,
            profile: profile.clone(),
            family: case::Family::MixedPressure,
            actions: vec![
                Action::Overlap { marker: 1 },
                Action::WindowPressure,
                Action::RefusedResponse,
                Action::LoseReply,
                Action::CancelReply,
                Action::Inventory,
                Action::Watch,
                Action::ByteStream { marker: 2 },
                Action::PersistenceGate,
                Action::HeldWorker,
                Action::CancelTransfer,
                Action::RetireWorker,
                Action::RestartController,
                Action::RestartTarget,
            ],
        };
        let left = execute::run(&case);
        let right = execute::run(&case);
        assert!(
            matches!(left, Run::Passed(_)),
            "profile {profile:?}: {:?}",
            match &left {
                Run::Failed { failure, .. } => Some(failure),
                Run::Passed(_) => None,
            }
        );
        assert_eq!(left, right, "atomic workload replay {profile:?}");
    }
}

#[test]
fn compact_artifacts_keep_inputs_and_digests_and_replay_failures_keep_both_traces() {
    use artifact::{Artifact, Evidence, RecordedRun, Retention};
    let case = case::routine().remove(0);
    let report = Report {
        observations: Vec::new(),
        trace: serde_json::json!({"actual_trace": [1,2,3]}),
    };
    let passed = Artifact::from_runs(
        case.clone(),
        [Run::Passed(report.clone()), Run::Passed(report.clone())],
        Retention::CompactPassing,
    );
    assert!(passed.passed());
    assert!(passed.runs.iter().all(|run| matches!(run, RecordedRun::Passed(Evidence::Compact { trace_digest, .. }) if *trace_digest == personal_rns::crypto::sha256(&serde_json::to_vec(&report.trace).expect("trace bytes")))));
    let other = Report {
        trace: serde_json::json!({"actual_trace": [1,2,4]}),
        ..report.clone()
    };
    let failed = Artifact::from_runs(
        case,
        [Run::Passed(report.clone()), Run::Passed(other.clone())],
        Retention::CompactPassing,
    );
    assert_eq!(
        failed.failure().expect("replay discrepancy").class,
        FailureClass::Replay
    );
    assert!(
        matches!(&failed.runs, [RecordedRun::Failed { report: Some(left), .. }, RecordedRun::Failed { report: Some(right), .. }] if *left == report && *right == other)
    );
}

#[test]
fn reducer_preserves_prerequisites_and_original_evidence() {
    use artifact::{Artifact, Retention};
    let mut case = case::routine().remove(0);
    case.actions = vec![
        Action::Inventory,
        Action::CancelReply,
        Action::LoseReply,
        Action::RestartTarget,
        Action::Watch,
    ];
    let execute = |case: &Case| {
        let report = Report {
            observations: vec![],
            trace: serde_json::Value::Null,
        };
        let run = match case
            .actions
            .iter()
            .position(|action| *action == Action::Watch)
        {
            Some(stage) if case.actions[..stage].contains(&Action::CancelReply) => Run::Failed {
                failure: Failure {
                    class: FailureClass::Runtime,
                    stage,
                    action: Some(Action::Watch),
                    detail: "synthetic reducer prerequisite".to_owned(),
                },
                report: Some(report),
            },
            _ => Run::Passed(report),
        };
        Artifact::from_runs(case.clone(), [run.clone(), run], Retention::Full)
    };
    let original = execute(&case);
    let bytes = serde_json::to_vec(&original).expect("original bytes");
    let reduced = reduction::reduce(&original, execute);
    assert_eq!(reduced.case.actions, [Action::CancelReply, Action::Watch]);
    assert_eq!(
        serde_json::to_vec(&original).expect("retained original"),
        bytes
    );
}
