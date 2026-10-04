use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};

#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    let owner = architecture.state_machine("UsbFirstOwner")?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● UsbFirstOwner
        ├── ▸ ApproveUsbFirstOwner
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── ApproveUsbFirstOwnerOutcome
        │       │       ├── Approve
        │       │       ├── Unavailable
        │       │       ├── WrongEndpoint
        │       │       └── Expired
        │       └── Err
        │           └── UsbFirstOwnerClockError
        │               └── WentBackwards
        ├── ▸ BindUsbFirstOwnerWindow
        │   └── ◆ BindUsbFirstOwnerWindowOutcome
        │       ├── Bound
        │       └── Unavailable
        ├── ▸ PrepareUsbFirstOwnerWindow
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── PrepareUsbFirstOwnerWindowOutcome
        │       │       ├── Open
        │       │       ├── WrongInterface
        │       │       ├── Unavailable
        │       │       └── Expired
        │       └── Err
        │           └── UsbFirstOwnerClockError
        │               └── WentBackwards
        └── ▸ ReadUsbFirstOwner
            └── ◆ UsbFirstOwnerSnapshot
    "#]]
    .assert_eq(&owner.render(DiagramTarget::TextTree));
    Ok(())
}
