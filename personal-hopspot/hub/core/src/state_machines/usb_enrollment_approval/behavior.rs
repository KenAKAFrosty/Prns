use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};
#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Approval boundaries match both independent deadlines
        - Architecture remains reviewable
        - Invitation foreign controller endpoint authority and expired windows are refused
        - Matching direct administrator offer is approved exactly once
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
