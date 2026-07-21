use core::time::Duration;

use prns_config::StationIdentificationPlan;
use prns_core::interfaces::kiss::{StationIdInterval, StationIdWireFormat, StationIdentification};

use super::PlanFailure;

pub(super) fn runtime(
    planned: &Option<StationIdentificationPlan>,
    wire_format: StationIdWireFormat,
) -> Result<Option<StationIdentification>, PlanFailure> {
    planned
        .as_ref()
        .map(|planned| {
            StationIdentification::new(
                planned.callsign().as_bytes(),
                StationIdInterval::new(Duration::from_secs(planned.interval_seconds())),
                wire_format,
            )
            .map_err(PlanFailure::from)
        })
        .transpose()
}
