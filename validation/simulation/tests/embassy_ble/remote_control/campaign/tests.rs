use super::*;
#[test]
fn routine_remote_control_campaign() {
    subprocess::campaign(case::routine(), "routine");
}
#[test]
#[ignore = "separate one-hour extended qualification lane"]
fn extended_remote_control_campaign() {
    subprocess::campaign(case::extended(), "extended");
}
#[test]
fn corpus_budgets_schema_and_reduction_prerequisites_are_explicit() {
    let routine = case::routine();
    assert_eq!(routine.len(), 96);
    assert!(routine.iter().all(|case| case.actions.len() == 32));
    let extended = case::extended();
    assert_eq!(extended.len(), 1024);
    assert!(extended.iter().all(|case| case.actions.len() == 128));
    for case in routine.into_iter().chain(extended) {
        assert_eq!(case.validate(), Ok(()));
        let encoded = serde_json::to_vec(&case).expect("case encoding");
        assert_eq!(
            serde_json::from_slice::<Case>(&encoded).expect("case decoding"),
            case
        );
        for index in 0..case.actions.len() {
            let mut reduced = case.clone();
            reduced.actions.remove(index);
            assert_eq!(
                reduced.validate(),
                Ok(()),
                "atomic lifecycle actions preserve their own prerequisites"
            );
        }
    }
}
