use super::*;
use std::path::Path;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Evidence {
    Compact {
        observations: Vec<Observation>,
        trace_digest: [u8; 32],
    },
    Full(Report),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordedRun {
    Passed(Evidence),
    Failed {
        failure: Failure,
        report: Option<Report>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub version: u32,
    pub case: Case,
    pub runs: [RecordedRun; 2],
}
pub enum Retention {
    CompactPassing,
    Full,
}
impl Artifact {
    pub fn from_runs(case: Case, runs: [Run; 2], retention: Retention) -> Self {
        let matched = matches!(&runs, [Run::Passed(left), Run::Passed(right)] if left == right);
        let replay = matches!(&runs, [Run::Passed(_), Run::Passed(_)]) && !matched;
        let runs = runs.map(|run| match run {
            Run::Passed(report) if matched && matches!(retention, Retention::CompactPassing) => {
                RecordedRun::Passed(Evidence::Compact {
                    observations: report.observations.clone(),
                    trace_digest: personal_rns::crypto::sha256(
                        &serde_json::to_vec(&report.trace).expect("trace encoding"),
                    ),
                })
            }
            Run::Passed(report) if replay => RecordedRun::Failed {
                failure: Failure {
                    class: FailureClass::Replay,
                    action: None,
                    stage: 0,
                    detail: "complete traces differ between fresh fixtures".to_owned(),
                },
                report: Some(report),
            },
            Run::Passed(report) => RecordedRun::Passed(Evidence::Full(report)),
            Run::Failed { failure, report } => RecordedRun::Failed { failure, report },
        });
        Self {
            version: case::VERSION,
            case,
            runs,
        }
    }
    pub fn write(&self, path: &Path) {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(self).expect("artifact encoding"),
        )
        .expect("artifact publication");
    }
    pub fn passed(&self) -> bool {
        matches!(&self.runs, [RecordedRun::Passed(left), RecordedRun::Passed(right)] if left == right)
    }
    pub fn failure(&self) -> Option<&Failure> {
        self.runs.iter().find_map(|run| match run {
            RecordedRun::Failed { failure, .. } => Some(failure),
            RecordedRun::Passed(_) => None,
        })
    }
}
