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
}

fn bounded_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn elapsed_micros(started_at: std::time::Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX)
}
