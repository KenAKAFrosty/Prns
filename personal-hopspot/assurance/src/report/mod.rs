mod compare;
mod render;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildError, SourceCustody};
use thiserror::Error;

use crate::contract::MatrixStatus;
use crate::evidence::{self, AggregateError, DiscoveryError, MatrixValidationError};

pub use compare::{compare, ComparisonError};

const JSON_FILENAME: &str = "matrix.json";
const MARKDOWN_FILENAME: &str = "matrix.md";

#[derive(Debug, Error)]
pub enum SummaryError {
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    #[error(transparent)]
    Aggregate(#[from] AggregateError),
    #[error(transparent)]
    Validation(#[from] MatrixValidationError),
    #[error("could not serialize embedded assurance matrix: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not publish embedded assurance matrix: {0}")]
    Publish(#[from] BuildError),
}

pub struct SummaryOutcome {
    json: PathBuf,
    markdown: PathBuf,
    targets: usize,
    capabilities: usize,
    status: MatrixStatus,
}

impl SummaryOutcome {
    #[must_use]
    pub fn json(&self) -> &Path {
        &self.json
    }

    #[must_use]
    pub fn markdown(&self) -> &Path {
        &self.markdown
    }

    #[must_use]
    pub const fn targets(&self) -> usize {
        self.targets
    }

    #[must_use]
    pub const fn capabilities(&self) -> usize {
        self.capabilities
    }

    #[must_use]
    pub const fn status(&self) -> &MatrixStatus {
        &self.status
    }
}

pub fn summarize(
    resources: &Path,
    proofs: &Path,
    output: &Path,
    source: &SourceCustody,
) -> Result<SummaryOutcome, SummaryError> {
    let resources = evidence::discover_resources(resources)?;
    let proofs = evidence::discover_proofs(proofs)?;
    evidence::validate_documents_current(&resources, &proofs, source)?;
    let matrix = evidence::assemble(resources, proofs)?;
    evidence::validate_matrix(&matrix)?;
    evidence::validate_current(&matrix, source)?;
    let mut json = serde_json::to_vec_pretty(&matrix)?;
    json.push(b'\n');
    let markdown = render::matrix(&matrix);
    let json_path = output.join(JSON_FILENAME);
    let markdown_path = output.join(MARKDOWN_FILENAME);
    publish(&json_path, &json)?;
    publish(&markdown_path, markdown.as_bytes())?;
    Ok(SummaryOutcome {
        json: json_path,
        markdown: markdown_path,
        targets: matrix.targets.len(),
        capabilities: matrix.capabilities.len(),
        status: matrix.status,
    })
}
