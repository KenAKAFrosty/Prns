//! Test constructors keep execution evidence separate from retained input.
#![allow(clippy::unwrap_used)]
use crate::*;
pub(crate) fn text(
    group: prns_groups_core::GroupId,
    scope: OriginalScope,
    id: [u8; 32],
    text: String,
    clock: prns_groups_core::ClockReading,
) -> (Request, ExecutionContext) {
    (
        Request {
            id: RequestId(id),
            original_scope: Some(scope),
            intent: Intent::Publish(PublicationIntent::text(group, &text).unwrap()),
        },
        ExecutionContext { clock },
    )
}
pub(crate) fn command((request, context): (Request, ExecutionContext)) -> crate::client::Command {
    crate::client::Command::Execute { request, context }
}
