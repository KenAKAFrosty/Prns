mod artifact;
mod case;
mod execute;
mod inventory_action;
mod pairing_action;
mod reduction;
mod subprocess;
mod tests;

use super::*;
use case::{Action, Approval, Case, Permission};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FailureClass {
    Expectation,
    Runtime,
    Replay,
    InvalidInput,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Failure {
    class: FailureClass,
    stage: usize,
    operation: Option<Action>,
    detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Observation {
    stage: usize,
    action: Action,
    outcome: String,
    permission: Permission,
    generations: [u64; 2],
    app_calls: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Report {
    observations: Vec<Observation>,
    wire: Vec<WireValue>,
    persistence: [serde_json::Value; 2],
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Run {
    Passed(Report),
    Failed {
        failure: Failure,
        report: Option<Report>,
    },
}
