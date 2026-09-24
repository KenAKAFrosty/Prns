use crate::contract::{
    ArchitectureId, Capability, CapabilityReason, ComponentId, IdentifierError, PlatformId,
    ProofFragment, ProofKind, RunnerId, ScenarioId, Subject, SupportLevel, ToolKind,
};
use personal_hopspot_memory::ProcessorArchitecture;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RunnerContractError {
    #[error("proof for {subject:?} uses runner {actual:?}, expected {expected:?}")]
    Mismatch {
        subject: Subject,
        actual: RunnerId,
        expected: String,
    },
    #[error("proof for {subject:?} uses emulator {actual:?}, expected {expected:?}")]
    EmulatorMismatch {
        subject: Subject,
        actual: ToolKind,
        expected: ToolKind,
    },
}

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
            subject: Subject::Architecture(ArchitectureId::parse(
                ProcessorArchitecture::ThumbV7em.id(),
            )?),
            scenario: ScenarioId::parse("shared-state-machines")?,
            proof: ProofKind::TargetIsa,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Architecture(ArchitectureId::parse(
                ProcessorArchitecture::RiscV32Imac.id(),
            )?),
            scenario: ScenarioId::parse("shared-state-machines")?,
            proof: ProofKind::TargetIsa,
            support: SupportLevel::Required,
        },
        Capability {
            subject: Subject::Architecture(ArchitectureId::parse(
                ProcessorArchitecture::XtensaEsp32S3.id(),
            )?),
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

pub fn validate_runner(
    capability: &Capability,
    proof: &ProofFragment,
) -> Result<(), RunnerContractError> {
    if matches!(capability.proof, ProofKind::Miri) {
        if matches!(proof.runner.as_str(), "miri-stacked" | "miri-stacked-tree") {
            return Ok(());
        }
        return Err(RunnerContractError::Mismatch {
            subject: capability.subject.clone(),
            actual: proof.runner.clone(),
            expected: "miri-stacked or miri-stacked-tree".to_string(),
        });
    }
    let expected = match &capability.subject {
        Subject::Architecture(architecture) => Some(format!("qemu-{architecture}")),
        Subject::Platform(platform) if platform.as_str() == "nrf52840" => {
            Some("renode-nrf52840".to_string())
        }
        Subject::Platform(platform) if platform.as_str() == "esp32s3" => {
            Some("qemu-esp32s3-startup".to_string())
        }
        Subject::Component(_) | Subject::Platform(_) | Subject::Target(_) => None,
    };
    if let Some(expected) = expected {
        if proof.runner.as_str() != expected {
            return Err(RunnerContractError::Mismatch {
                subject: capability.subject.clone(),
                actual: proof.runner.clone(),
                expected,
            });
        }
    }
    let expected_emulator = match &capability.subject {
        Subject::Platform(platform) if platform.as_str() == "nrf52840" => Some(ToolKind::Renode),
        Subject::Platform(platform) if platform.as_str() == "esp32s3" => Some(ToolKind::Qemu),
        Subject::Architecture(_)
        | Subject::Component(_)
        | Subject::Platform(_)
        | Subject::Target(_) => None,
    };
    if let Some(expected) = expected_emulator {
        if let Some(actual) = proof
            .tools
            .iter()
            .map(|tool| tool.kind)
            .find(|kind| matches!(kind, ToolKind::Qemu | ToolKind::Renode))
        {
            if actual != expected {
                return Err(RunnerContractError::EmulatorMismatch {
                    subject: capability.subject.clone(),
                    actual,
                    expected,
                });
            }
        }
    }
    Ok(())
}
