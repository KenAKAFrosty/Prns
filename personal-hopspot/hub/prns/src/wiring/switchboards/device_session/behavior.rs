use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Already settled initial inventory is not dispatched again
        - Arbitrary sessions match connection ownership and reject retired callbacks
        - Connect and disconnect attempts preserve tokens and duplicate confirmations preserve inventory
        - Connect drives pages to a complete snapshot and refresh reuses the link
        - Exhausted generation errors remain precise invariants
        - Late confirmations close only the old token and schedule new inventory after cleanup
        - Malformed and oversized pages stop dispatch and wrong target duplicates are not accepted
        - Mismatched targets are closed and foreign callbacks preserve both devices
        - Missing links and busy fittings settle only matching connections
        - Missing unpaired and unconnected devices never dispatch transport work
        - Synchronization can start inventory without duplicating or dispatching cancelled requests
        - Typed failures settle the core without erasing last complete inventory
    "#]].assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
