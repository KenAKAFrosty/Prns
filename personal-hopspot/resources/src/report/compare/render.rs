use std::path::Path;

use super::model::{
    ArtifactComparison, ByteComparison, RamComparison, RamHeadroomComparison, ResourceComparison,
    SectionComparison, SettingDifference,
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
    metric(&mut output, "flash image", comparison.flash_image);
    metric(&mut output, "flash headroom", comparison.flash_headroom);
    for artifact in &comparison.artifacts {
        artifact_metric(&mut output, artifact);
    }
    for ram in &comparison.ram {
        ram_metrics(&mut output, ram);
    }
    for section in &comparison.sections {
        section_metrics(&mut output, section);
    }
    output
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
