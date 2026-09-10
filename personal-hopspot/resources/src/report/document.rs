use std::path::Path;

use personal_hopspot_builder::SourceCustody;
use prns_flash_manifest::{sha256_hex, Sha256Digest};

use super::compare::{load_report, ComparisonError};
use super::model::{BuildStatus, ResourceReport};
use super::Fingerprint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildOutcome {
    Success,
    MemoryOverflow,
}

#[derive(Debug)]
pub struct Document {
    report: ResourceReport,
    fingerprint: Sha256Digest,
}

impl Document {
    pub fn load(path: &Path) -> Result<Self, ComparisonError> {
        let report = load_report(path)?;
        let encoded = serde_json::to_vec(&report).map_err(|source| ComparisonError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let fingerprint = Sha256Digest::parse(sha256_hex(&encoded)).map_err(|_| {
            ComparisonError::InvalidReportFingerprint {
                path: path.to_path_buf(),
            }
        })?;
        Ok(Self {
            report,
            fingerprint,
        })
    }

    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.report.schema_version
    }

    #[must_use]
    pub const fn source_custody(&self) -> &SourceCustody {
        &self.report.source
    }

    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.report.target.id
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.report.target.display_name
    }

    #[must_use]
    pub fn memory_profile(&self) -> &str {
        &self.report.target.memory_profile
    }

    #[must_use]
    pub fn rust_target(&self) -> &str {
        &self.report.architecture.rust_target
    }

    #[must_use]
    pub fn architecture_adapter(&self) -> &str {
        &self.report.architecture.adapter
    }

    #[must_use]
    pub fn build_fingerprint(&self) -> &Fingerprint {
        &self.report.build.fingerprint
    }

    #[must_use]
    pub fn toolchain_fingerprint(&self) -> &Fingerprint {
        &self.report.toolchain.fingerprint
    }

    #[must_use]
    pub fn memory_contract_fingerprint(&self) -> &Fingerprint {
        &self.report.memory_contract.fingerprint
    }

    #[must_use]
    pub fn fingerprint(&self) -> &Sha256Digest {
        &self.fingerprint
    }

    #[must_use]
    pub const fn build_outcome(&self) -> BuildOutcome {
        match self.report.status {
            BuildStatus::Success => BuildOutcome::Success,
            BuildStatus::MemoryOverflow { .. } => BuildOutcome::MemoryOverflow,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{BuildOutcome, Document};

    #[test]
    fn validated_document_exposes_only_composition_identity(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("experiments/lto/t-echo-s140-v6-fat.json");
        let document = Document::load(&path)?;
        assert_eq!(document.schema_version(), 8);
        assert_eq!(document.target_id(), "t-echo-s140-v6");
        assert_eq!(document.memory_profile(), "t-echo-s140-v6");
        assert_eq!(document.rust_target(), "thumbv7em-none-eabihf");
        assert_eq!(document.build_outcome(), BuildOutcome::Success);
        assert_eq!(document.fingerprint().as_str().len(), 64);
        Ok(())
    }
}
