use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Every shutdown initiator preserves both owner results
        - Requested shutdown drains authentication and closes before stopping the node
        - Session completion requests native shutdown without waiting for an external signal
        - Spontaneous node failure stops the session and retains the node error
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
