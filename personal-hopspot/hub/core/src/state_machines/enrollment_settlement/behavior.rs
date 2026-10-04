use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Arbitrary event retry and acknowledgment histories match the phase model
        - Architecture remains reviewable
        - Authorization latches before retry and terminal recording is idempotent
        - Protocol failure preserves every reason and never enables persistence
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
