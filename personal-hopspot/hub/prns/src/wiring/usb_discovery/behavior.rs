use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Native preparation initializes discovery from the attached usb and persisted clock
        - Poison and clock failures are reported without recovering uncertain state
        - Signed native ingress routes discovery and callbacks can inspect without deadlock
        - Unrelated native messages and diagnostics remain available to the caller
        - Verified events and cloned handles share the native clock and selection
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
