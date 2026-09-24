use super::*;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub version: u32,
    pub case: Case,
    pub runs: [Run; 2],
}
impl Artifact {
    pub fn write(&self, path: &Path) {
        let bytes = serde_json::to_vec_pretty(self).expect("versioned artifact");
        std::fs::write(path, bytes).expect("write qualification artifact");
    }
    pub fn passed(&self) -> bool {
        matches!(&self.runs, [Run::Passed(left), Run::Passed(right)] if left == right)
    }
    pub fn failure(&self) -> Option<Failure> {
        match &self.runs {
            [Run::Failed { failure, .. }, _] | [_, Run::Failed { failure, .. }] => {
                Some(failure.clone())
            }
            [Run::Passed(left), Run::Passed(right)] if left != right => Some(Failure {
                class: FailureClass::Replay,
                stage: 0,
                operation: None,
                detail: "fresh fixtures produced different byte or persistence traces".to_owned(),
            }),
            [Run::Passed(_), Run::Passed(_)] => None,
        }
    }
}
