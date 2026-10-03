use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};

#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    let interfaces = architecture.state_machine("DeviceInterfaces")?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● DeviceInterfaces
        ├── ▸ CloseInterfaceInventory
        │   └── ◆ CloseInterfaceInventoryOutcome
        │       ├── Closed
        │       └── AlreadyClosed
        ├── ▸ InterfaceRefreshFailed
        │   └── ◆ InterfaceRefreshFailedOutcome
        │       ├── Failed
        │       └── StaleRequest
        ├── ▸ ReadDeviceInterfaces
        │   └── ◆ ReadDeviceInterfacesOutcome
        │       ├── Found
        │       └── Unavailable
        ├── ▸ ReceiveInterfacePage
        │   └── ◆ ReceiveInterfacePageOutcome
        │       ├── More
        │       ├── Complete
        │       ├── StaleRequest
        │       ├── OutOfOrder
        │       └── CapacityExceeded
        ├── ▸ RefreshInterfaces
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── RefreshInterfacesOutcome
        │       │       ├── Requested
        │       │       ├── Busy
        │       │       └── Closed
        │       └── Err
        │           └── RefreshInterfacesError
        │               └── IdentifiersExhausted
        └── ▸ SynchronizeDeviceInterfaces
            └── ◆ SynchronizeDeviceInterfacesOutcome
                ├── Started
                ├── Unchanged
                └── Unavailable
    "#]]
    .assert_eq(&interfaces.render(DiagramTarget::TextTree));
    Ok(())
}
