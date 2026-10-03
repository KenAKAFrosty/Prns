use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Arbitrary reconnect histories preserve cancellation and reject every retired request
        - Architecture remains reviewable
        - Disconnect and forget cancel pending pages without touching another device
        - Explicit close is idempotent and does not recreate tokens for the same connection
        - Only confirmed connections start inventory and repeated synchronization preserves work
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
