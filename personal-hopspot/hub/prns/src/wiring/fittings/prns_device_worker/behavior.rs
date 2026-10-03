use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Abandoning a duplicate connect does not close the owned session
        - Accepted close runs even when its completion is abandoned
        - An unaccepted connection completion is closed even after delivery
        - Arbitrary cancellation histories preserve link ownership
        - Arbitrary queue bounds retain every rejected command
        - Backpressure and stopped submission return the original input
        - Cancelled connect settles authentication and closes before later work
        - Cancelled queued connect and inventory never reach the backend
        - Cancelling a running inventory unblocks close without waiting for io
        - Fifo work preserves typed outputs and drains before shutdown
        - Fitting invariants and peer failures keep their original types
        - Queue capacity bounds are checked before creating the runtime channel
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
