use crate::contract::{AssuranceMatrix, CapabilityResult, MatrixStatus, SupportLevel, Verdict};

pub fn matrix(matrix: &AssuranceMatrix) -> String {
    let mut output = format!(
        "# Embedded assurance matrix\n\nStatus: **{}**\n\n## Canonical targets\n\n| Target | Architecture | Memory profile | Resource evidence |\n| --- | --- | --- | --- |\n",
        matrix_status(&matrix.status)
    );
    for target in &matrix.targets {
        output.push_str(&format!(
            "| {} (`{}`) | `{}` | `{}` | {} |\n",
            cell(&target.display_name),
            target.id,
            target.architecture,
            cell(&target.memory_profile),
            verdict(&target.resource),
        ));
    }
    output.push_str(
        "\n## Executable proofs\n\n| Subject | Scenario | Support | Verdict |\n| --- | --- | --- | --- |\n",
    );
    for result in &matrix.capabilities {
        let capability = result.capability();
        let verdict = match result {
            CapabilityResult::Observed { proof, .. } => verdict_name(&proof.verdict),
            CapabilityResult::Unavailable { .. } => "unavailable",
        };
        output.push_str(&format!(
            "| `{}` | `{}` | {} | {} |\n",
            capability.subject,
            capability.scenario,
            support(&capability.support),
            verdict,
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

fn verdict_name<T>(verdict: &Verdict<T>) -> &'static str {
    match verdict {
        Verdict::Passed { .. } => "passed",
        Verdict::Failed { .. } => "failed",
        Verdict::Partial { .. } => "partial",
        Verdict::Unavailable { .. } => "unavailable",
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

fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}
