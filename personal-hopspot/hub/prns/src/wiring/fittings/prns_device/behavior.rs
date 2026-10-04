use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Abandoned connect finishes identification and releases its link
        - Absent management routes are discovered once and failures keep their stage
        - Admission and exchange failures preserve request and link ownership
        - Arbitrary commands match single link ownership
        - Command acknowledgments and uncertain exchanges remain distinct
        - Connected pages preserve tokens cursors status and rtt
        - Controls require the exact connection and permission before transmitting
        - Native tcp nodes authenticate report inventory and execute controls
        - Stale generations and foreign devices never touch the backend
        - Unexpected targets are rejected and worker panics are invariants
        - Upstream failures remain typed and identification failure closes once
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
