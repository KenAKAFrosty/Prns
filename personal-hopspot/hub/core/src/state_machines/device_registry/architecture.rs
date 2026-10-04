use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};

#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    let registry = architecture.state_machine("DeviceRegistry")?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● DeviceRegistry
        ├── ▸ BeginConnection
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── BeginConnectionOutcome
        │       │       ├── Connect
        │       │       ├── MissingDevice
        │       │       ├── NotPaired
        │       │       ├── AlreadyConnecting
        │       │       └── AlreadyConnected
        │       └── Err
        │           └── BeginConnectionError
        │               └── IdentifiersExhausted
        ├── ▸ BeginEnrollment
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── BeginEnrollmentOutcome
        │       │       ├── Started
        │       │       ├── MissingDevice
        │       │       ├── AlreadyPairing
        │       │       └── AlreadyPaired
        │       └── Err
        │           └── BeginEnrollmentError
        │               └── IdentifiersExhausted
        ├── ▸ CancelEnrollment
        │   └── ◆ CancelEnrollmentOutcome
        │       ├── Cancelled
        │       ├── MissingDevice
        │       └── StaleEnrollment
        ├── ▸ CompleteEnrollment
        │   └── ◆ CompleteEnrollmentOutcome
        │       ├── Recorded
        │       ├── MissingDevice
        │       ├── StaleEnrollment
        │       ├── TargetMismatch
        │       └── TargetAlreadyPaired
        ├── ▸ ConfirmConnection
        │   └── ◆ ConfirmConnectionOutcome
        │       ├── Connected
        │       ├── MissingDevice
        │       ├── StaleConnection
        │       └── TargetMismatch
        ├── ▸ CreateDevice
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── CreateDeviceOutcome
        │       │       ├── Created
        │       │       └── AtCapacity
        │       └── Err
        │           └── CreateDeviceError
        │               └── IdentifiersExhausted
        ├── ▸ EndConnection
        │   └── ◆ EndConnectionOutcome
        │       ├── AttemptEnded
        │       ├── SessionEnded
        │       ├── MissingDevice
        │       └── StaleConnection
        ├── ▸ FailEnrollment
        │   └── ◆ FailEnrollmentOutcome
        │       ├── Failed
        │       ├── MissingDevice
        │       └── StaleEnrollment
        ├── ▸ ForgetDevice
        │   └── ◆ ForgetDeviceOutcome
        │       ├── Forgotten
        │       └── MissingDevice
        ├── ▸ ListDevices
        │   └── ◆ ListDevicesOutcome
        │       ├── Listed
        │       └── InsufficientCapacity
        ├── ▸ ReadDevice
        │   └── ◆ ReadDeviceOutcome
        │       ├── Found
        │       └── MissingDevice
        ├── ▸ ReadRememberedDevices
        │   └── ◆ ReadRememberedDevicesOutcome
        │       ├── Read
        │       └── InsufficientCapacity
        ├── ▸ RenameDevice
        │   └── ◆ RenameDeviceOutcome
        │       ├── Renamed
        │       └── MissingDevice
        └── ▸ RestoreRememberedDevice
            └── ◆ Result
                ├── Ok
                │   └── RestoreRememberedDeviceOutcome
                │       ├── Restored
                │       ├── TargetAlreadyPaired
                │       └── AtCapacity
                └── Err
                    └── RestoreRememberedDeviceError
                        └── IdentifiersExhausted
    "#]]
    .assert_eq(&registry.render(DiagramTarget::TextTree));
    Ok(())
}
