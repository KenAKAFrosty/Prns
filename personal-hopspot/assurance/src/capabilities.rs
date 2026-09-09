use crate::contract::{
    ArchitectureId, Capability, CapabilityReason, ComponentId, IdentifierError, PlatformId,
    ProofKind, ScenarioId, Subject, SupportLevel,
};

pub fn canonical() -> Result<Vec<Capability>, IdentifierError> {
    Ok(vec![
        Capability {
            subject: Subject::Component(ComponentId::parse("sx126x")?),
            scenario: ScenarioId::parse("sx126x-state-machine")?,
            proof: ProofKind::Miri,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Component(ComponentId::parse("lr1110")?),
            scenario: ScenarioId::parse("lr1110-state-machine")?,
            proof: ProofKind::Miri,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Component(ComponentId::parse("embedded-persistence")?),
            scenario: ScenarioId::parse("flash-journal-state-machine")?,
            proof: ProofKind::Miri,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Architecture(ArchitectureId::parse("thumbv7em")?),
            scenario: ScenarioId::parse("shared-state-machines")?,
            proof: ProofKind::TargetIsa,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Architecture(ArchitectureId::parse("riscv32imac")?),
            scenario: ScenarioId::parse("shared-state-machines")?,
            proof: ProofKind::TargetIsa,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Architecture(ArchitectureId::parse("xtensa-esp32s3")?),
            scenario: ScenarioId::parse("shared-state-machines")?,
            proof: ProofKind::TargetIsa,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Platform(PlatformId::parse("nrf52840")?),
            scenario: ScenarioId::parse("platform-startup")?,
            proof: ProofKind::PlatformEmulation,
            support: SupportLevel::Pilot,
        },
        Capability {
            subject: Subject::Platform(PlatformId::parse("esp32s3")?),
            scenario: ScenarioId::parse("platform-startup")?,
            proof: ProofKind::PlatformEmulation,
            support: SupportLevel::Pilot,
        },
        Capability {
            subject: Subject::Platform(PlatformId::parse("esp32c6")?),
            scenario: ScenarioId::parse("platform-startup")?,
            proof: ProofKind::PlatformEmulation,
            support: SupportLevel::Unsupported(CapabilityReason::EmulatorDoesNotModelPlatform),
        },
    ])
}
