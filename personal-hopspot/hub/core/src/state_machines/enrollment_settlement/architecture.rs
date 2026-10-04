use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};

#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    let settlement = architecture.state_machine("EnrollmentSettlement")?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● EnrollmentSettlement
        ├── ▸ ObserveEnrollmentResult
        │   └── ◆ ObserveEnrollmentResultOutcome
        │       ├── Persist
        │       ├── Fail
        │       ├── UnrelatedAttempt
        │       ├── AwaitingPersistence
        │       └── Settled
        ├── ▸ ReadEnrollmentSettlement
        │   └── ◆ EnrollmentSettlementSnapshot
        ├── ▸ RecordEnrollmentPersistence
        │   └── ◆ RecordEnrollmentPersistenceOutcome
        │       ├── Recorded
        │       ├── AlreadyRecorded
        │       ├── AwaitingAuthorization
        │       └── Failed
        └── ▸ RetryEnrollmentPersistence
            └── ◆ RetryEnrollmentPersistenceOutcome
                ├── Persist
                ├── AwaitingAuthorization
                └── Settled
    "#]]
    .assert_eq(&settlement.render(DiagramTarget::TextTree));
    Ok(())
}
