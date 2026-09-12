use std::path::Path;

use super::model::{
    ArtifactComparison, AsyncMemoryComparison, AttributionCandidateBaseline,
    AttributionCandidateComparison, AttributionCategoryComparison, AttributionComparison,
    ByteComparison, CountComparison, EvidenceComparison, ExecutableComparison, FlashComparison,
    ModeledChainAssessmentComparison, NamedSizeComparison, OverflowComparison, OverflowState,
    RamComparison, RamHeadroomComparison, ResourceComparison, ScenarioFutureComparison,
    SectionComparison, SettingDifference, StackEvidenceComparison, StackReservationComparison,
    StatusComparison,
};

pub(super) fn render(comparison: &ResourceComparison, before: &Path, after: &Path) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "EMBEDDED_RESOURCE_COMPARISON: target={} before={} after={}\n",
        comparison.target,
        before.display(),
        after.display()
    ));
    for setting in &comparison.settings {
        match setting {
            SettingDifference::RequestedLto { before, after } => {
                output.push_str(&format!("setting requested-lto {before} -> {after}\n"));
            }
            SettingDifference::EffectiveLto { before, after } => {
                output.push_str(&format!("setting effective-lto {before} -> {after}\n"));
            }
        }
    }
    status(&mut output, &comparison.status);
    evidence(&mut output, "flash", &comparison.flash, flash_metrics);
    evidence(
        &mut output,
        "artifacts",
        &comparison.artifacts,
        |output, artifacts| {
            for artifact in artifacts {
                artifact_metric(output, artifact);
            }
        },
    );
    evidence(&mut output, "ram", &comparison.ram, |output, ram| {
        for usage in ram {
            ram_metrics(output, usage);
        }
    });
    evidence(
        &mut output,
        "sections",
        &comparison.sections,
        |output, sections| {
            for section in sections {
                section_metrics(output, section);
            }
        },
    );
    attribution(&mut output, &comparison.attribution);
    evidence(
        &mut output,
        "executable",
        &comparison.executable,
        executable,
    );
    stack(&mut output, &comparison.stack);
    evidence(
        &mut output,
        "async-memory",
        &comparison.async_memory,
        async_memory,
    );
    output
}

fn stack(output: &mut String, comparison: &StackEvidenceComparison) {
    let (before, after, value) = match comparison {
        StackEvidenceComparison::Comparable {
            before,
            after,
            value,
        } => (before, after, Some(value)),
        StackEvidenceComparison::NotComparable { before, after } => (before, after, None),
    };
    output.push_str(&format!("stack evidence {before} -> {after}\n"));
    let Some(value) = value else {
        return;
    };
    output.push_str(&format!("stack frame-source {}\n", value.frame_source));
    metric(output, "stack frame-source evidence", value.source_bytes);
    count_metric(output, "stack frames", value.frames);
    metric(
        output,
        "stack largest-modeled-direct-call-chain",
        value.modeled_chain,
    );
    for function in &value.changed_largest_frames {
        output.push_str(&format!("stack ranked-frame changed {function:?}\n"));
    }
    match &value.reservation {
        StackReservationComparison::Declared {
            reservation,
            bytes,
            assessment,
        } => {
            output.push_str(&format!("stack reservation {reservation:?} {bytes}\n"));
            match assessment {
                ModeledChainAssessmentComparison::WithinReservation { remaining } => {
                    metric(output, "stack modeled-chain advisory remaining", *remaining)
                }
                ModeledChainAssessmentComparison::OverReservation { excess } => {
                    metric(output, "stack modeled-chain advisory excess", *excess)
                }
                ModeledChainAssessmentComparison::Changed => {
                    output.push_str("stack modeled-chain advisory changed\n");
                }
            }
        }
        StackReservationComparison::Undeclared => output.push_str("stack reservation undeclared\n"),
        StackReservationComparison::Changed => output.push_str("stack reservation changed\n"),
    }
    for gap in &value.gaps {
        count_metric(output, &format!("stack gap {}", gap.name), gap.count);
    }
}

fn async_memory(output: &mut String, comparison: &AsyncMemoryComparison) {
    metric(output, "async task-pool total", comparison.task_pool_total);
    for pool in &comparison.task_pools {
        named_size(output, "async task-pool", pool);
    }
    match &comparison.scenario_futures {
        ScenarioFutureComparison::Measured(futures) => {
            for future in futures {
                named_size(output, "async scenario-future", future);
            }
        }
        ScenarioFutureComparison::Unavailable { before, after } => output.push_str(&format!(
            "async scenario-futures unavailable {before} -> {after}\n"
        )),
        ScenarioFutureComparison::AvailabilityChanged { before, after } => {
            output.push_str(&format!("async scenario-futures {before} -> {after}\n"))
        }
    }
}

fn named_size(output: &mut String, category: &str, comparison: &NamedSizeComparison) {
    match (comparison.before, comparison.after) {
        (Some(before), Some(after)) => metric(
            output,
            &format!("{category} {:?}", comparison.name),
            ByteComparison::new(before, after),
        ),
        (Some(before), None) => output.push_str(&format!(
            "{category} {:?} {before} -> not-present\n",
            comparison.name
        )),
        (None, Some(after)) => output.push_str(&format!(
            "{category} {:?} not-present -> {after}\n",
            comparison.name
        )),
        (None, None) => {}
    }
}

fn executable(output: &mut String, comparison: &ExecutableComparison) {
    output.push_str(&format!("machine entry-point {}\n", comparison.entry_point));
    metric(output, "machine executable", comparison.section_bytes);
    output.push_str(&format!(
        "machine sections changed {}\n",
        comparison.changed_sections.len()
    ));
    for section in &comparison.changed_sections {
        output.push_str(&format!("machine section changed {section:?}\n"));
    }
    output.push_str(&format!(
        "machine function-boundaries {}\n",
        comparison.function_boundaries
    ));
    output.push_str(&format!(
        "machine functions {} -> {}\n",
        comparison.functions_before, comparison.functions_after
    ));
    for function in &comparison.changed_ranked_functions {
        output.push_str(&format!("machine ranked-function changed {function:?}\n"));
    }
    metric(output, "machine decoded", comparison.decoded_bytes);
    metric(output, "machine undecoded", comparison.undecoded_bytes);
}

fn attribution(output: &mut String, comparison: &AttributionComparison) {
    let (before, after, categories) = match comparison {
        AttributionComparison::Comparable {
            before,
            after,
            categories,
        } => (before, after, Some(categories)),
        AttributionComparison::NotComparable { before, after } => (before, after, None),
    };
    output.push_str(&format!("attribution evidence {before} -> {after}\n"));
    if let Some(categories) = categories {
        attribution_category(output, "crates", &categories.crates);
        attribution_category(output, "symbols", &categories.symbols);
    }
}

fn attribution_category(
    output: &mut String,
    name: &str,
    comparison: &AttributionCategoryComparison,
) {
    metric(
        output,
        &format!("attribution {name} analyzed"),
        comparison.coverage.analyzed,
    );
    metric(
        output,
        &format!("attribution {name} attributed"),
        comparison.coverage.attributed,
    );
    metric(
        output,
        &format!("attribution {name} unclassified"),
        comparison.coverage.unclassified,
    );
    for candidate in &comparison.candidates {
        attribution_candidate(output, name, candidate);
    }
}

fn attribution_candidate(
    output: &mut String,
    category: &str,
    comparison: &AttributionCandidateComparison,
) {
    let (before, delta) = match comparison.before {
        AttributionCandidateBaseline::Ranked(before) => (
            before.to_string(),
            format!(
                " ({})",
                ByteComparison::new(before, comparison.after_bytes).delta
            ),
        ),
        AttributionCandidateBaseline::NotRanked => ("not-ranked".to_string(), String::new()),
    };
    output.push_str(&format!(
        "attribution {category} candidate {} {before} -> {}{delta} {:?}\n",
        comparison.rank, comparison.after_bytes, comparison.name
    ));
}

fn status(output: &mut String, comparison: &StatusComparison) {
    output.push_str(&format!(
        "status {} -> {}\n",
        comparison.before, comparison.after
    ));
    for overflow in &comparison.overflows {
        overflow_metric(output, overflow);
    }
}

fn overflow_metric(output: &mut String, comparison: &OverflowComparison) {
    match (comparison.before, comparison.after) {
        (OverflowState::Overflow(before), OverflowState::Overflow(after)) => metric(
            output,
            &format!("overflow {:?}", comparison.linker_region),
            ByteComparison::new(before, after),
        ),
        (before, after) => output.push_str(&format!(
            "overflow {:?} {} -> {}\n",
            comparison.linker_region, before, after
        )),
    }
}

fn evidence<T>(
    output: &mut String,
    name: &str,
    comparison: &EvidenceComparison<T>,
    render: impl FnOnce(&mut String, &T),
) {
    match comparison {
        EvidenceComparison::Comparable(comparison) => render(output, comparison),
        EvidenceComparison::NotComparable { before, after } => {
            output.push_str(&format!("{name} evidence {before} -> {after}\n"));
        }
    }
}

fn flash_metrics(output: &mut String, flash: &FlashComparison) {
    metric(output, "flash image", flash.image);
    metric(output, "flash headroom", flash.headroom);
}

fn metric(output: &mut String, name: &str, comparison: ByteComparison) {
    output.push_str(&format!(
        "{name} {} -> {} ({})",
        comparison.before, comparison.after, comparison.delta
    ));
    output.push('\n');
}

fn count_metric(output: &mut String, name: &str, comparison: CountComparison) {
    output.push_str(&format!(
        "{name} {} -> {} ({})\n",
        comparison.before, comparison.after, comparison.delta
    ));
}

fn artifact_metric(output: &mut String, artifact: &ArtifactComparison) {
    metric(
        output,
        &format!("artifact {:?}", artifact.path),
        artifact.bytes,
    );
    output.push_str(&format!(
        "artifact {:?} fingerprint {}\n",
        artifact.path, artifact.fingerprint
    ));
}

fn ram_metrics(output: &mut String, ram: &RamComparison) {
    metric(
        output,
        &format!("ram {} static", ram.backing_store),
        ram.static_sections,
    );
    metric(
        output,
        &format!("ram {} padding", ram.backing_store),
        ram.linker_padding,
    );
    match ram.headroom {
        RamHeadroomComparison::Known(headroom) => metric(
            output,
            &format!("ram {} headroom", ram.backing_store),
            headroom,
        ),
        RamHeadroomComparison::RuntimeDetected => {
            output.push_str(&format!(
                "ram {} headroom runtime-detected\n",
                ram.backing_store
            ));
        }
    }
}

fn section_metrics(output: &mut String, section: &SectionComparison) {
    metric(
        output,
        &format!("section {} run", section.kind),
        section.run_bytes,
    );
    metric(
        output,
        &format!("section {} load", section.kind),
        section.load_bytes,
    );
}
