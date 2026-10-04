use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Arbitrary requests and reconnections never accept retired replies
        - Architecture remains reviewable
        - Disconnect forget and reconnect cancel work without reusing request ids
        - Request exhaustion never wraps or changes pending work
        - Requests require a confirmed connection and preserve the exact command
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
