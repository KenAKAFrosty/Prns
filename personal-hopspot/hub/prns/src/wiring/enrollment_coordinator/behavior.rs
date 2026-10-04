use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Capacity refusal is retryable and target refusals are preserved
        - Matching authorization persists before success and duplicate events do not write
        - Protocol failures keep their reasons and leave other devices untouched
        - Stale generation and forgotten records cannot be completed or failed by old coordinators
        - Unexpected settlement acknowledgment preserves the invariant outcome
        - Write failure retains authorization across abort and explicit retry
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
