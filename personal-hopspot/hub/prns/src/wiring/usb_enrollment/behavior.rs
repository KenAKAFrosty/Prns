use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};
#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Attached tbeam display controls are acknowledged over usb [ignored]
        - Attached tbeam enrolls and reports live interfaces [ignored]
        - Direct enrollment persists both authorizations and hands off to live interface inventory
        - Exhausted enrollment identifiers preserve the rejected offer and close its link
        - Native preparation is inert and event overflow or shutdown is explicit
        - Offers and registry refusals close the attempt without granting or recording
        - Settlement correlation duplicate confirmation and failed persistence preserve retry
        - Stopped events and unadvanced approval return resources for recovery
        - Unavailable candidates and failed initiation leave the registry and store owned
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
