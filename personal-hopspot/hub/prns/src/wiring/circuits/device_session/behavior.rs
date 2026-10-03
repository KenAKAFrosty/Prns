use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Conduction preserves each route and recovers the registry on mailbox close
        - Stale readiness does not route and foreign messages surface the exact invariant
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
