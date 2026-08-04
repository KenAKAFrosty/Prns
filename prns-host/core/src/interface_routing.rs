use crate::InterfaceMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterfaceRoutingPolicy {
    pub mode: Option<InterfaceMode>,
    pub gravity: Option<i64>,
    pub recursive_path_requests: Option<bool>,
    pub announces_from_internal: Option<bool>,
    pub announces_to_internal: Option<bool>,
}
