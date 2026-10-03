use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};

#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - An empty terminal page completes a full buffer
        - Arbitrary ordered pages roundtrip wire and publish exactly once
        - Architecture remains reviewable
        - Closing cancels pending work erases data and rejects reuse
        - Duplicate or backwards pages and capacity overflow never publish partial data
        - Failed refreshes preserve last complete data and reject old responses
        - Refresh generations exhaust without wrapping or losing published data
        - Wire pages publish only complete inventory for the connected device
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
