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
        │   └── ◆ BeginConnectionOutcome
        │       ├── Connect
        │       ├── MissingDevice
        │       ├── NotPaired
        │       ├── AlreadyConnecting
        │       ├── AlreadyConnected
        │       └── IdentifiersExhausted
        ├── ▸ BeginEnrollment
        │   └── ◆ BeginEnrollmentOutcome
        │       ├── Started
        │       ├── MissingDevice
        │       ├── AlreadyPairing
        │       ├── AlreadyPaired
        │       └── IdentifiersExhausted
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
        │   └── ◆ CreateDeviceOutcome
        │       ├── Created
        │       ├── AtCapacity
        │       └── IdentifiersExhausted
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
        └── ▸ RenameDevice
            └── ◆ RenameDeviceOutcome
                ├── Renamed
                └── MissingDevice
    "#]]
    .assert_eq(&registry.render(DiagramTarget::TextTree));
    Ok(())
}
