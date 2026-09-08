use super::model::MatrixSummary;

pub(super) fn markdown(summary: &MatrixSummary) -> String {
    let mut output = String::from(
        "# Embedded resource matrix\n\nExact byte counts; deltas are current minus the committed baseline.\n\n| Target | Architecture | Flash | Δ | Flash headroom (Δ) | Static RAM | Δ | Known RAM used | Δ | Known RAM headroom (Δ) | Toolchain |\n| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n",
    );
    for target in &summary.targets {
        output.push_str(&format!(
            "| {} (`{}`) | `{}` | {} | {} | {} ({}) | {} | {} | {} | {} | {} ({}) | {} |\n",
            cell(&target.display_name),
            cell(&target.id),
            cell(&target.rust_target),
            target.flash_image.current_bytes,
            target.flash_image.delta,
            target.flash_headroom.current_bytes,
            target.flash_headroom.delta,
            target.static_ram.current_bytes,
            target.static_ram.delta,
            target.known_ram_used.current_bytes,
            target.known_ram_used.delta,
            target.known_ram_headroom.current_bytes,
            target.known_ram_headroom.delta,
            target.toolchain.relation,
        ));
    }
    let runtime_detected = summary
        .targets
        .iter()
        .filter(|target| !target.runtime_detected_ram.is_empty())
        .collect::<Vec<_>>();
    if !runtime_detected.is_empty() {
        output.push_str("\n## Runtime-detected RAM\n\n");
        for target in runtime_detected {
            output.push_str(&format!(
                "- `{}`: {}\n",
                cell(&target.id),
                target
                    .runtime_detected_ram
                    .iter()
                    .map(|store| format!("`{}`", cell(store)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    output
}

fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}
