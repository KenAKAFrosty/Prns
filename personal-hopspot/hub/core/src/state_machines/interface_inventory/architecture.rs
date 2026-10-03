use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};

#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    let inventory = architecture.state_machine("InterfaceInventory")?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● InterfaceInventory
        ├── ▸ CloseInterfaceInventory
        │   └── ◆ CloseInterfaceInventoryOutcome
        │       ├── Closed
        │       └── AlreadyClosed
        ├── ▸ InterfaceRefreshFailed
        │   └── ◆ InterfaceRefreshFailedOutcome
        │       ├── Failed
        │       └── StaleRequest
        ├── ▸ ReadInterfaces
        │   └── ◆ InterfaceInventorySnapshot
        ├── ▸ ReceiveInterfacePage
        │   └── ◆ ReceiveInterfacePageOutcome
        │       ├── More
        │       ├── Complete
        │       ├── StaleRequest
        │       ├── OutOfOrder
        │       └── CapacityExceeded
        └── ▸ RefreshInterfaces
            └── ◆ RefreshInterfacesOutcome
                ├── Requested
                ├── Busy
                ├── Closed
                └── IdentifiersExhausted
    "#]]
    .assert_eq(&inventory.render(DiagramTarget::TextTree));
    Ok(())
}
