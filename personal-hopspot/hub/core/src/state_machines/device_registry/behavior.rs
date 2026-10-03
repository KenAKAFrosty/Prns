use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Arbitrary cancel retry sequences never accept superseded results
        - Arbitrary labels and removals agree with an independent record model
        - Arbitrary reconnect histories isolate sessions and preserve pairing
        - Architecture remains reviewable
        - Bounded records support duplicate labels and stable ids after removal
        - Cancellation and failure preserve reasons and reject old generations
        - Connection generations exhaust without wrapping or disturbing other devices
        - Construction failure retains its original storage diagnostic
        - Enrollment generations exhaust without wrapping or changing state
        - Exhausted storage returns the unmodified rejected label
        - Forgetting invalidates every enrollment step even after row reuse
        - Forgetting returns pending or live connection and invalidates late results
        - Only paired devices connect and confirmation binds the expected target
        - Query steps own snapshots and refuse incomplete lists
        - Retries preserve failure reasons and reject superseded callbacks
        - Two devices pair independently and reject replacement
        - Wrong target and duplicate binding leave attempts unchanged
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
