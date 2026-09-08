use std::path::Path;

use super::model::{
    ArtifactComparison, AttributionCandidateBaseline, AttributionCandidateComparison,
    AttributionCategoryComparison, AttributionComparison, ByteComparison, EvidenceComparison,
    ExecutableComparison, FlashComparison, OverflowComparison, OverflowState, RamComparison,
    RamHeadroomComparison, ResourceComparison, SectionComparison, SettingDifference,
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
            SettingDifference::Lto { before, after } => {
                output.push_str(&format!("setting lto {before} -> {after}\n"));
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
    output
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

fn artifact_metric(output: &mut String, artifact: &ArtifactComparison) {
    metric(
        output,
        &format!("artifact {:?}", artifact.path),
        artifact.bytes,
    );
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
