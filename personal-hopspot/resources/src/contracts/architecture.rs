use std::fmt::Write;
use std::path::Path;

use personal_hopspot_memory::ProcessorArchitecture;
use thiserror::Error;

use super::RenderedArtifact;

pub(super) const RELATIVE_PATH: &str = "validation/hardening/embedded_architectures.py";

#[derive(Debug, Error)]
pub(crate) enum ArchitectureContractError {
    #[error("the generated Python architecture contract could not be formatted")]
    Format(#[from] std::fmt::Error),
    #[error("an architecture value could not be encoded as a Python string: {0}")]
    StringEncoding(#[from] serde_json::Error),
}

pub(super) fn render(root: &Path) -> Result<RenderedArtifact, ArchitectureContractError> {
    let mut contents = String::new();
    writeln!(contents, "ARCHITECTURES = {{")?;
    for architecture in ProcessorArchitecture::ALL {
        writeln!(
            contents,
            "    {}: {},",
            serde_json::to_string(architecture.id())?,
            serde_json::to_string(architecture.rust_target())?
        )?;
    }
    writeln!(contents, "}}")?;
    Ok(RenderedArtifact {
        path: root.join(RELATIVE_PATH),
        contents,
    })
}
