mod artifact;
mod case;
pub(super) mod execute;
mod reduction;
mod subprocess;
mod tests;
use case::{Action, Case};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    Runtime,
    Replay,
    InvalidInput,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub class: FailureClass,
    pub action: Option<Action>,
    pub stage: usize,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metrics {
    Unavailable,
    Available(Vec<(String, String, u64)>),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub action: Action,
    pub generations: [u64; 3],
    pub app_calls: usize,
    pub metrics: [Metrics; 3],
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub observations: Vec<Observation>,
    pub trace: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Run {
    Passed(Report),
    Failed {
        failure: Failure,
        report: Option<Report>,
    },
}
