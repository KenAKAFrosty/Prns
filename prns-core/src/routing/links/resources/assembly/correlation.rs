use crate::routing::links::request::RequestId;
use crate::routing::links::resources::ResourceCorrelation;

/// A split chain's wire association, without outgoing request policy. Every
/// segment must preserve both the request ID and its request/response role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyCorrelation {
    Unsolicited,
    Request(RequestId),
    Response(RequestId),
}

impl From<ResourceCorrelation> for AssemblyCorrelation {
    fn from(correlation: ResourceCorrelation) -> Self {
        match correlation {
            ResourceCorrelation::Unsolicited => Self::Unsolicited,
            ResourceCorrelation::Request { id, .. } => Self::Request(id),
            ResourceCorrelation::Response(id) => Self::Response(id),
        }
    }
}
