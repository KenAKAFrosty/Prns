use crate::InterfaceMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct InterfaceRoutingPolicy {
    pub mode: Option<InterfaceMode>,
    pub gravity: Option<i64>,
    pub recursive_path_requests: Option<bool>,
    pub announces_from_internal: Option<bool>,
    pub announces_to_internal: Option<bool>,
}
