use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Accepted messages drain before closed and rejections preserve the input
        - Arbitrary mailbox turns match a bounded fifo
        - Interrupted and spurious polls retry but other errors preserve their source
        - Real poll observes wakes and last sender drop after the empty check
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
