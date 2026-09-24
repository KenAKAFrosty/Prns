use super::super::adapter::{Runtime, PAIRS};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const ROUTINE_SEEDS: [u64; 6] = [0, 1, 7, 42, 0x5eed, u64::MAX];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Authority,
    Requests,
    Pairing,
    Inventory,
}
pub const FAMILIES: [Family; 4] = [
    Family::Authority,
    Family::Requests,
    Family::Pairing,
    Family::Inventory,
];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Full,
    DescribeOnly,
    Absent,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Accept,
    RejectController,
    RejectTarget,
    Expire,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Permissions(Permission),
    Describe,
    Build,
    Echo { marker: u8, len: usize },
    AppReject,
    Malformed,
    Oversized,
    LoseResponse,
    CancelCaller,
    Reconnect,
    RestartController,
    RestartTarget,
    Pair(Approval),
    Inventory,
    WatchInventory,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub version: u32,
    pub seed: u64,
    pub runtimes: [Runtime; 2],
    pub family: Family,
    pub actions: Vec<Action>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum InvalidCase {
    Version,
    ActionBudget,
    PayloadBudget,
}
impl Case {
    pub fn validate(&self) -> Result<(), InvalidCase> {
        if self.version != VERSION {
            return Err(InvalidCase::Version);
        }
        if self.actions.is_empty() || self.actions.len() > 128 {
            return Err(InvalidCase::ActionBudget);
        }
        if self
            .actions
            .iter()
            .any(|action| matches!(action, Action::Echo { len, .. } if *len > 96))
        {
            return Err(InvalidCase::PayloadBudget);
        }
        Ok(())
    }
    pub fn generated(
        seed: u64,
        runtimes: [Runtime; 2],
        family: Family,
        action_budget: usize,
    ) -> Self {
        let mut state = seed ^ 0x9e3779b97f4a7c15;
        let mut actions = vec![Action::Describe, Action::Build];
        for index in 2..action_budget {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let draw = state.wrapping_mul(0x2545f4914f6cdd1d);
            let action = match family {
                Family::Authority => match draw % 8 {
                    0 => Action::Permissions(Permission::Full),
                    1 => Action::Permissions(Permission::DescribeOnly),
                    2 => Action::Permissions(Permission::Absent),
                    3 => Action::RestartTarget,
                    4 => Action::RestartController,
                    5 => Action::Describe,
                    6 => Action::Build,
                    _ => Action::Echo {
                        marker: index as u8,
                        len: (draw % 97) as usize,
                    },
                },
                Family::Requests => match draw % 9 {
                    0 => Action::Malformed,
                    1 => Action::Oversized,
                    2 => Action::AppReject,
                    3 => Action::LoseResponse,
                    4 => Action::CancelCaller,
                    5 => Action::Reconnect,
                    _ => Action::Echo {
                        marker: index as u8,
                        len: (draw % 97) as usize,
                    },
                },
                Family::Pairing => match draw % 9 {
                    0 => Action::Pair(Approval::Accept),
                    1 => Action::Pair(Approval::RejectController),
                    2 => Action::Pair(Approval::RejectTarget),
                    3 => Action::Pair(Approval::Expire),
                    4 => Action::Permissions(Permission::DescribeOnly),
                    5 => Action::Permissions(Permission::Absent),
                    6 => Action::RestartTarget,
                    7 => Action::RestartController,
                    _ => Action::Build,
                },
                Family::Inventory => match draw % 6 {
                    0 => Action::Reconnect,
                    1 => Action::RestartTarget,
                    2 => Action::RestartController,
                    3 => Action::WatchInventory,
                    _ => Action::Inventory,
                },
            };
            actions.push(action);
        }
        let case = Self {
            version: VERSION,
            seed,
            runtimes,
            family,
            actions,
        };
        case.validate().expect("generated valid actions");
        case
    }
}
pub fn routine() -> Vec<Case> {
    ROUTINE_SEEDS
        .into_iter()
        .flat_map(|seed| {
            PAIRS.into_iter().flat_map(move |runtimes| {
                FAMILIES
                    .into_iter()
                    .map(move |family| Case::generated(seed, runtimes.clone(), family, 32))
            })
        })
        .collect()
}
pub fn extended() -> Vec<Case> {
    (0..256)
        .flat_map(|seed| {
            PAIRS.into_iter().enumerate().map(move |(pair, runtimes)| {
                Case::generated(
                    seed,
                    runtimes,
                    FAMILIES[(seed as usize + pair) % 4].clone(),
                    128,
                )
            })
        })
        .collect()
}
