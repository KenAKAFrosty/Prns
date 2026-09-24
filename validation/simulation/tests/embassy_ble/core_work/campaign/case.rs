use super::super::profile::{self, Execution, Profile};
use crate::remote_control::adapter::Runtime;
use serde::{Deserialize, Serialize};
pub const VERSION: u32 = 1;
pub const ROUTINE_SEEDS: [u64; 4] = [0, 1, 42, 0x5eed];
const MAX_ACTIONS: usize = 128;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    CompletionLifecycle,
    MixedPressure,
    RecoveryAttribution,
}
pub const FAMILIES: [Family; 3] = [
    Family::CompletionLifecycle,
    Family::MixedPressure,
    Family::RecoveryAttribution,
];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Overlap { marker: u8 },
    HeldWorker,
    CancelTransfer,
    RetireWorker,
    WindowPressure,
    RefusedResponse,
    LoseReply,
    CancelReply,
    RestartController,
    RestartTarget,
    Inventory,
    Watch,
    ByteStream { marker: u8 },
    PersistenceGate,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub version: u32,
    pub seed: u64,
    pub profile: Profile,
    pub family: Family,
    pub actions: Vec<Action>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum InvalidCase {
    Version,
    ActionBudget,
    UnsupportedExecution,
}
impl Case {
    pub fn validate(&self) -> Result<(), InvalidCase> {
        if self.version != VERSION {
            return Err(InvalidCase::Version);
        }
        if self.actions.is_empty() || self.actions.len() > MAX_ACTIONS {
            return Err(InvalidCase::ActionBudget);
        }
        if self.profile.runtimes == [Runtime::Embassy, Runtime::Embassy]
            && self.profile.execution != Execution::Inline
        {
            return Err(InvalidCase::UnsupportedExecution);
        }
        Ok(())
    }
    pub fn generated(seed: u64, profile: Profile, family: Family, budget: usize) -> Self {
        let mut state = seed ^ 0x9e3779b97f4a7c15;
        let mut actions = vec![Action::Inventory, Action::Overlap { marker: 0 }];
        for index in actions.len()..budget {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let draw = state.wrapping_mul(0x2545f4914f6cdd1d);
            let action = match family {
                Family::CompletionLifecycle => match draw % 7 {
                    0 => Action::HeldWorker,
                    1 => Action::RetireWorker,
                    2 => Action::CancelReply,
                    3 => Action::RestartController,
                    4 => Action::RestartTarget,
                    5 => Action::CancelTransfer,
                    _ => Action::Overlap {
                        marker: index as u8,
                    },
                },
                Family::MixedPressure => match draw % 7 {
                    0 => Action::WindowPressure,
                    1 => Action::RefusedResponse,
                    2 => Action::ByteStream {
                        marker: index as u8,
                    },
                    3 => Action::Watch,
                    4 => Action::PersistenceGate,
                    _ => Action::Overlap {
                        marker: index as u8,
                    },
                },
                Family::RecoveryAttribution => match draw % 7 {
                    0 => Action::LoseReply,
                    1 => Action::RefusedResponse,
                    2 => Action::CancelReply,
                    3 => Action::RestartTarget,
                    4 => Action::RestartController,
                    5 => Action::Inventory,
                    _ => Action::Overlap {
                        marker: index as u8,
                    },
                },
            };
            actions.push(action);
        }
        let case = Self {
            version: VERSION,
            seed,
            profile,
            family,
            actions,
        };
        case.validate().expect("bounded generated case");
        case
    }
}
fn matrix(seeds: impl IntoIterator<Item = u64>, budget: usize) -> Vec<Case> {
    seeds
        .into_iter()
        .flat_map(|seed| {
            profile::all().into_iter().flat_map(move |profile| {
                FAMILIES
                    .into_iter()
                    .map(move |family| Case::generated(seed, profile.clone(), family, budget))
            })
        })
        .collect()
}
pub fn routine() -> Vec<Case> {
    matrix(ROUTINE_SEEDS, 32)
}
pub fn extended() -> Vec<Case> {
    matrix(0..64, 64)
}
