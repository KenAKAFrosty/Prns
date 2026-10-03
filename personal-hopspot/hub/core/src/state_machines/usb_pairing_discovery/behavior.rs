use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Advancing time expires at the deadline recovers capacity and refuses to rewind
        - Arbitrary arrival and expiry orders match an independent bounded model
        - Architecture remains reviewable
        - Newer observations update in place even when full and older ones cannot extend expiry
        - Other interfaces future and expired observations cannot change the list
        - Verified usb availability is listed and selection preserves its provenance
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
