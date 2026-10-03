use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Arbitrary reopens and competing owners preserve one identity
        - Corrupt identity material is refused without replacement
        - Installation directory and new lock are private
        - Release unlocks even while a duplicated descriptor is still open
        - Reopening preserves distinct identities and lock file contents
        - Startup errors preserve their stage and release the installation lock
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
