#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlanErrorKind {
    UnsupportedKind,
    MissingRequiredField { key: &'static str },
    InvalidSetting { key: &'static str },
}

impl From<SettingRepresentationError> for PlanErrorKind {
    fn from(error: SettingRepresentationError) -> Self {
        Self::InvalidSetting { key: error.key }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SettingRepresentationError {
    pub(super) key: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GlobalPlanError {
    pub(super) key: &'static str,
}

impl From<SettingRepresentationError> for GlobalPlanError {
    fn from(error: SettingRepresentationError) -> Self {
        Self { key: error.key }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlanError {
    pub(super) interface_name: String,
    pub(super) interface_type: String,
    pub(super) subinterface_name: Option<String>,
    pub(super) kind: PlanErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PlanningError {
    Global(GlobalPlanError),
    Interface(PlanError),
}
