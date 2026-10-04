use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - An unconnected boot window expires at the deadline and cannot reopen
        - Arbitrary approval times match the window and single claim model
        - Architecture remains reviewable
        - Only confirmed unowned restore can open once on the selected usb
        - Only the bound endpoint can claim once before expiry and time cannot rewind
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
