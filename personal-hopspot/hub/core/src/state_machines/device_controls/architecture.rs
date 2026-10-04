use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};

#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● DeviceControls
        ├── ▸ ReadDeviceControls
        │   └── ◆ DeviceControlsSnapshot
        ├── ▸ RequestDeviceControl
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── RequestDeviceControlOutcome
        │       │       ├── Requested
        │       │       ├── Busy
        │       │       └── NotConnected
        │       └── Err
        │           └── RequestDeviceControlError
        │               └── IdentifiersExhausted
        ├── ▸ SettleDeviceControl
        │   └── ◆ SettleDeviceControlOutcome
        │       ├── Settled
        │       └── StaleRequest
        └── ▸ SynchronizeDeviceControls
            └── ◆ Option
                ├── Some
                │   └── DeviceControlRequest
                └── None
    "#]]
    .assert_eq(
        &architecture
            .state_machine("DeviceControls")?
            .render(DiagramTarget::TextTree),
    );
    Ok(())
}
