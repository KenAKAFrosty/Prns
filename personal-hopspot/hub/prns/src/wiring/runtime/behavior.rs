use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Native preparation attaches usb without running and owns the lock
        - Terminal persistence failure is reported before releasing the lock
        - Usb rescans and persisted authorization survive a graceful restart
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
