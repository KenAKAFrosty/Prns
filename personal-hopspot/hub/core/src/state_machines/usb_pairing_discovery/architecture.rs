use expect_test::expect;
use pipecircuit_visualization::{ArchitectureError, CrateArchitecture, DiagramTarget};

#[test]
fn architecture_remains_reviewable() -> Result<(), ArchitectureError> {
    let architecture = CrateArchitecture::read(env!("CARGO_MANIFEST_DIR"))?;
    let discovery = architecture.state_machine("UsbPairingDiscovery")?;
    expect![[r#"
        ● state machine  ▸ step input  ◆ outcome

        ● UsbPairingDiscovery
        ├── ▸ AdvanceUsbPairingDiscovery
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── AdvanceUsbPairingDiscoveryOutcome
        │       │       └── Advanced
        │       └── Err
        │           └── AdvanceUsbPairingDiscoveryError
        │               └── TimeWentBackwards
        ├── ▸ ClearUsbPairingCandidates
        │   └── ◆ ClearUsbPairingCandidatesOutcome
        ├── ▸ ObserveUsbPairingAvailability
        │   └── ◆ Result
        │       ├── Ok
        │       │   └── ObserveUsbPairingAvailabilityOutcome
        │       │       ├── Added
        │       │       ├── Updated
        │       │       ├── StaleObservation
        │       │       ├── WrongInterface
        │       │       ├── Expired
        │       │       └── CapacityExceeded
        │       └── Err
        │           └── ObserveUsbPairingAvailabilityError
        │               └── ObservedInFuture
        ├── ▸ ReadUsbPairingCandidates
        │   └── ◆ UsbPairingCandidatesSnapshot
        └── ▸ SelectUsbPairingCandidate
            └── ◆ SelectUsbPairingCandidateOutcome
                ├── Selected
                └── Unavailable
    "#]]
    .assert_eq(&discovery.render(DiagramTarget::TextTree));
    Ok(())
}
