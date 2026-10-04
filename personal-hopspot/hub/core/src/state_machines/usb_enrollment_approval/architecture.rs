use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};
#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● UsbEnrollmentApproval
        └── ▸ ReviewUsbEnrollmentOffer
            └── ◆ Result
                ├── Ok
                │   └── ReviewUsbEnrollmentOfferOutcome
                │       ├── Approve
                │       ├── AlreadyApproved
                │       ├── InvitationRequired
                │       ├── WrongEndpoint
                │       ├── WrongController
                │       ├── InsufficientAuthority
                │       └── Expired
                └── Err
                    └── ReviewUsbEnrollmentOfferError
                        └── ClockWentBackwards
    "#]]
    .assert_eq(
        &architecture
            .state_machine("UsbEnrollmentApproval")?
            .render(DiagramTarget::TextTree),
    );
    Ok(())
}
