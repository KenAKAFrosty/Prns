use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Arbitrary inspection selection and clear histories expire before routing
        - Clock invariants preserve the list and do not hide observation failures
        - Expiry precedes admission and clear and domain refusals remain flat
        - Routes verified availability and fresh selection with owned snapshots
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
