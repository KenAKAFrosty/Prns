use crate::runtime::ManifoldMetricsSnapshot;

#[derive(Default)]
pub(super) struct TurnActivity {
    pub(super) completions: usize,
    pub(super) inbound_frames: usize,
    pub(super) commands: usize,
    pub(super) owed_work: usize,
}

#[derive(Default)]
pub(super) struct ManifoldMetrics {
    snapshot: ManifoldMetricsSnapshot,
}

impl ManifoldMetrics {
    pub(super) fn snapshot(&self) -> ManifoldMetricsSnapshot {
        self.snapshot
    }

    pub(super) fn record_turn(
        &mut self,
        started_at: std::time::Instant,
        activity: TurnActivity,
        budget_exhausted: bool,
    ) {
        self.snapshot.turns = self.snapshot.turns.saturating_add(1);
        self.snapshot.maximum_turn_micros = self
            .snapshot
            .maximum_turn_micros
            .max(elapsed_micros(started_at));
        if budget_exhausted {
            self.snapshot.budget_yields = self.snapshot.budget_yields.saturating_add(1);
        }
        self.snapshot.maximum_completion_batch = self
            .snapshot
            .maximum_completion_batch
            .max(bounded_u32(activity.completions));
        self.snapshot.maximum_inbound_batch = self
            .snapshot
            .maximum_inbound_batch
            .max(bounded_u32(activity.inbound_frames));
        self.snapshot.maximum_command_batch = self
            .snapshot
            .maximum_command_batch
            .max(bounded_u32(activity.commands));
        self.snapshot.maximum_owed_work_batch = self
            .snapshot
            .maximum_owed_work_batch
            .max(bounded_u32(activity.owed_work));
    }

    pub(super) fn record_inline_work(&mut self, started_at: std::time::Instant, jobs: usize) {
        self.snapshot.inline_jobs = self
            .snapshot
            .inline_jobs
            .saturating_add(u64::try_from(jobs).unwrap_or(u64::MAX));
        self.snapshot.maximum_inline_work_micros = self
            .snapshot
            .maximum_inline_work_micros
            .max(elapsed_micros(started_at));
    }

    pub(super) fn record_timer_lateness(&mut self, deadline: u64, observed: u64) {
        self.snapshot.maximum_timer_lateness_ms = self
            .snapshot
            .maximum_timer_lateness_ms
            .max(observed.saturating_sub(deadline));
    }

    pub(super) fn record_pacer_lateness(&mut self, deadline: u64, observed: u64) {
        self.snapshot.maximum_pacer_lateness_ms = self
            .snapshot
            .maximum_pacer_lateness_ms
            .max(observed.saturating_sub(deadline));
    }

    pub(super) fn record_resource_request_to_first_frame(&mut self, elapsed: std::time::Duration) {
        let micros = duration_micros(elapsed);
        self.snapshot.resource_request_to_first_frame_observations = self
            .snapshot
            .resource_request_to_first_frame_observations
            .saturating_add(1);
        self.snapshot.resource_request_to_first_frame_total_micros = self
            .snapshot
            .resource_request_to_first_frame_total_micros
            .saturating_add(micros);
        self.snapshot.maximum_resource_request_to_first_frame_micros = self
            .snapshot
            .maximum_resource_request_to_first_frame_micros
            .max(micros);
    }

    pub(super) fn record_resource_request_round_gap(&mut self, elapsed: std::time::Duration) {
        let micros = duration_micros(elapsed);
        self.snapshot.resource_request_round_gap_observations = self
            .snapshot
            .resource_request_round_gap_observations
            .saturating_add(1);
        self.snapshot.resource_request_round_gap_total_micros = self
            .snapshot
            .resource_request_round_gap_total_micros
            .saturating_add(micros);
        self.snapshot.maximum_resource_request_round_gap_micros = self
            .snapshot
            .maximum_resource_request_round_gap_micros
            .max(micros);
    }
}

fn bounded_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn elapsed_micros(started_at: std::time::Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn duration_micros(elapsed: std::time::Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_round_intervals_accumulate_totals_and_maxima() {
        let mut metrics = ManifoldMetrics::default();
        metrics.record_resource_request_to_first_frame(std::time::Duration::from_micros(7));
        metrics.record_resource_request_to_first_frame(std::time::Duration::from_micros(11));
        metrics.record_resource_request_round_gap(std::time::Duration::from_micros(13));
        metrics.record_resource_request_round_gap(std::time::Duration::from_micros(17));

        assert_eq!(
            metrics.snapshot(),
            ManifoldMetricsSnapshot {
                resource_request_to_first_frame_observations: 2,
                resource_request_to_first_frame_total_micros: 18,
                maximum_resource_request_to_first_frame_micros: 11,
                resource_request_round_gap_observations: 2,
                resource_request_round_gap_total_micros: 30,
                maximum_resource_request_round_gap_micros: 17,
                ..ManifoldMetricsSnapshot::default()
            }
        );
    }
}
