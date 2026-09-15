use crate::contract::{
    AssuranceMatrix, CapabilityReason, CapabilityResult, EvidenceGap, FailureKind, MatrixStatus,
    SupportLevel, UnavailableReason, Verdict,
};

pub fn matrix(matrix: &AssuranceMatrix) -> String {
    let mut output = format!(
        "# Embedded assurance matrix\n\nStatus: **{}**\n\n## Canonical targets\n\n| Target | Architecture | Memory profile | Resource evidence | Details |\n| --- | --- | --- | --- | --- |\n",
        matrix_status_detail(&matrix.status)
    );
    for target in &matrix.targets {
        output.push_str(&format!(
            "| {} (`{}`) | `{}` | `{}` | {} | {} |\n",
            cell(&target.display_name),
            target.id,
            target.architecture,
            cell(&target.memory_profile),
            verdict(&target.resource),
            cell(&verdict_detail(&target.resource)),
        ));
    }
    output.push_str(
        "\n## Executable proofs\n\n| Subject | Scenario | Support | Verdict | Details |\n| --- | --- | --- | --- | --- |\n",
    );
    for result in &matrix.capabilities {
        let capability = result.capability();
        let (verdict, details) = match result {
            CapabilityResult::Observed { proof, .. } => {
                (verdict_name(&proof.verdict), verdict_detail(&proof.verdict))
            }
            CapabilityResult::Unavailable { reason, .. } => {
                ("unavailable", unavailable_reason(*reason).to_string())
            }
        };
        let details = match support_reason(&capability.support) {
            Some(reason) => format!("{details}; contract: {reason}"),
            None => details,
        };
        output.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            capability.subject,
            capability.scenario,
            support(&capability.support),
            verdict,
            cell(&details),
        ));
    }
    output
}

pub fn matrix_status(status: &MatrixStatus) -> &'static str {
    match status {
        MatrixStatus::Passed => "passed",
        MatrixStatus::Failed { .. } => "failed",
    }
}

pub fn verdict<T>(verdict: &Verdict<T>) -> &'static str {
    verdict_name(verdict)
}

fn matrix_status_detail(status: &MatrixStatus) -> String {
    match status {
        MatrixStatus::Passed => "passed".to_string(),
        MatrixStatus::Failed { required_failures } => {
            format!("failed ({required_failures} required failures)")
        }
    }
}

fn verdict_name<T>(verdict: &Verdict<T>) -> &'static str {
    match verdict {
        Verdict::Passed { .. } => "passed",
        Verdict::Failed { .. } => "failed",
        Verdict::Partial { .. } => "partial",
        Verdict::Unavailable { .. } => "unavailable",
    }
}

fn verdict_detail<T>(verdict: &Verdict<T>) -> String {
    match verdict {
        Verdict::Passed { .. } => "—".to_string(),
        Verdict::Failed { failure } => {
            format!("{}: {}", failure_kind(failure.kind), failure.diagnostic)
        }
        Verdict::Partial { gaps, .. } => format!(
            "gaps: {}",
            gaps.iter()
                .map(|gap| evidence_gap(*gap))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Verdict::Unavailable { reason } => unavailable_reason(*reason).to_string(),
    }
}

fn support(support: &SupportLevel) -> &'static str {
    match support {
        SupportLevel::Required => "required",
        SupportLevel::Pilot => "pilot",
        SupportLevel::Unsupported(_) => "unsupported",
        SupportLevel::NotApplicable(_) => "not applicable",
    }
}

const fn support_reason(support: &SupportLevel) -> Option<&'static str> {
    match support {
        SupportLevel::Unsupported(reason) | SupportLevel::NotApplicable(reason) => {
            Some(capability_reason(*reason))
        }
        SupportLevel::Required | SupportLevel::Pilot => None,
    }
}

const fn failure_kind(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Crash => "crash",
        FailureKind::MemoryOverflow => "memory-overflow",
        FailureKind::ScenarioMismatch => "scenario-mismatch",
        FailureKind::StructuralViolation => "structural-violation",
        FailureKind::Timeout => "timeout",
        FailureKind::ToolFailure => "tool-failure",
    }
}

const fn evidence_gap(gap: EvidenceGap) -> &'static str {
    match gap {
        EvidenceGap::Assembly => "assembly",
        EvidenceGap::IndirectCall => "indirect-call",
        EvidenceGap::InterruptNesting => "interrupt-nesting",
        EvidenceGap::MissingMetadata => "missing-metadata",
        EvidenceGap::MissingRoot => "missing-root",
        EvidenceGap::UnsupportedPeripheral => "unsupported-peripheral",
        EvidenceGap::VendorObject => "vendor-object",
    }
}

const fn unavailable_reason(reason: UnavailableReason) -> &'static str {
    match reason {
        UnavailableReason::EvidenceNotProduced => "evidence-not-produced",
        UnavailableReason::EmulatorUnavailable => "emulator-unavailable",
        UnavailableReason::RunnerNotInstalled => "runner-not-installed",
        UnavailableReason::ToolchainUnavailable => "toolchain-unavailable",
        UnavailableReason::UnsupportedByContract => "unsupported-by-contract",
    }
}

const fn capability_reason(reason: CapabilityReason) -> &'static str {
    match reason {
        CapabilityReason::ArchitectureDoesNotUseComponent => "architecture-does-not-use-component",
        CapabilityReason::EmulatorDoesNotModelPlatform => "emulator-does-not-model-platform",
        CapabilityReason::EmulatorDoesNotModelPeripheral => "emulator-does-not-model-peripheral",
        CapabilityReason::ScenarioOutsidePlatformScope => "scenario-outside-platform-scope",
    }
}

fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}
