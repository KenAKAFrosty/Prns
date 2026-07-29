#[derive(Debug, PartialEq, Eq)]
pub enum EinkRefresh {
    Deferred,
    Full,
    Partial,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EinkRefreshUrgency {
    Immediate,
    Telemetry,
}

pub struct EinkRefreshPolicy {
    partial_refresh_limit: u32,
    full_refresh_max_age_ms: u64,
    telemetry_min_interval_ms: u64,
    partial_refreshes_since_full: u32,
    last_full_refresh_ms: Option<u64>,
    last_refresh_ms: Option<u64>,
    recovery_required: bool,
}

impl EinkRefreshPolicy {
    pub const fn new(
        partial_refresh_limit: u32,
        full_refresh_max_age_ms: u64,
        telemetry_min_interval_ms: u64,
    ) -> Self {
        Self {
            partial_refresh_limit,
            full_refresh_max_age_ms,
            telemetry_min_interval_ms,
            partial_refreshes_since_full: 0,
            last_full_refresh_ms: None,
            last_refresh_ms: None,
            recovery_required: false,
        }
    }

    pub fn for_changed_frame(&self, now_ms: u64, urgency: &EinkRefreshUrgency) -> EinkRefresh {
        let telemetry_deferred = *urgency == EinkRefreshUrgency::Telemetry
            && !self.recovery_required
            && self
                .last_refresh_ms
                .is_some_and(|last| now_ms.saturating_sub(last) < self.telemetry_min_interval_ms);
        if telemetry_deferred {
            return EinkRefresh::Deferred;
        }
        let full_refresh_expired = self
            .last_full_refresh_ms
            .is_some_and(|last| now_ms.saturating_sub(last) >= self.full_refresh_max_age_ms);
        if self.last_full_refresh_ms.is_none()
            || self.recovery_required
            || self.partial_refreshes_since_full >= self.partial_refresh_limit
            || full_refresh_expired
        {
            EinkRefresh::Full
        } else {
            EinkRefresh::Partial
        }
    }

    pub fn full_refresh_succeeded(&mut self, now_ms: u64) {
        self.partial_refreshes_since_full = 0;
        self.last_full_refresh_ms = Some(now_ms);
        self.last_refresh_ms = Some(now_ms);
        self.recovery_required = false;
    }

    pub fn partial_refresh_succeeded(&mut self, now_ms: u64) {
        self.partial_refreshes_since_full = self.partial_refreshes_since_full.saturating_add(1);
        self.last_refresh_ms = Some(now_ms);
    }

    pub const fn refresh_failed(&mut self) {
        self.recovery_required = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARTIAL_LIMIT: u32 = 3;
    const MAX_AGE_MS: u64 = 1_000;

    fn policy_after_initial_full() -> EinkRefreshPolicy {
        let mut policy = EinkRefreshPolicy::new(PARTIAL_LIMIT, MAX_AGE_MS, 100);
        policy.full_refresh_succeeded(100);
        policy
    }

    #[test]
    fn first_changed_frame_requires_a_full_refresh() {
        let policy = EinkRefreshPolicy::new(PARTIAL_LIMIT, MAX_AGE_MS, 100);
        assert_eq!(
            policy.for_changed_frame(0, &EinkRefreshUrgency::Telemetry),
            EinkRefresh::Full
        );
    }

    #[test]
    fn partial_limit_requires_cleanup_on_the_next_changed_frame() {
        let mut policy = policy_after_initial_full();
        for _ in 0..PARTIAL_LIMIT {
            assert_eq!(
                policy.for_changed_frame(200, &EinkRefreshUrgency::Immediate),
                EinkRefresh::Partial
            );
            policy.partial_refresh_succeeded(200);
        }
        assert_eq!(
            policy.for_changed_frame(200, &EinkRefreshUrgency::Immediate),
            EinkRefresh::Full
        );
    }

    #[test]
    fn elapsed_age_requires_cleanup_on_the_next_changed_frame() {
        let policy = policy_after_initial_full();
        assert_eq!(
            policy.for_changed_frame(1_099, &EinkRefreshUrgency::Immediate),
            EinkRefresh::Partial
        );
        assert_eq!(
            policy.for_changed_frame(1_100, &EinkRefreshUrgency::Immediate),
            EinkRefresh::Full
        );
    }

    #[test]
    fn any_failed_refresh_requires_full_recovery() {
        let mut policy = policy_after_initial_full();
        policy.refresh_failed();
        assert_eq!(
            policy.for_changed_frame(101, &EinkRefreshUrgency::Telemetry),
            EinkRefresh::Full
        );
        policy.full_refresh_succeeded(101);
        assert_eq!(
            policy.for_changed_frame(102, &EinkRefreshUrgency::Immediate),
            EinkRefresh::Partial
        );
    }

    #[test]
    fn successful_full_refresh_resets_partial_budget_and_age() {
        let mut policy = policy_after_initial_full();
        for _ in 0..PARTIAL_LIMIT {
            policy.partial_refresh_succeeded(200);
        }
        policy.full_refresh_succeeded(2_000);
        assert_eq!(
            policy.for_changed_frame(2_999, &EinkRefreshUrgency::Immediate),
            EinkRefresh::Partial
        );
    }

    #[test]
    fn telemetry_is_coalesced_but_immediate_frames_bypass_the_interval() {
        let policy = policy_after_initial_full();
        assert_eq!(
            policy.for_changed_frame(199, &EinkRefreshUrgency::Telemetry),
            EinkRefresh::Deferred
        );
        assert_eq!(
            policy.for_changed_frame(199, &EinkRefreshUrgency::Immediate),
            EinkRefresh::Partial
        );
        assert_eq!(
            policy.for_changed_frame(200, &EinkRefreshUrgency::Telemetry),
            EinkRefresh::Partial
        );
    }

    #[test]
    fn continuously_changing_telemetry_needs_twelve_full_refreshes_per_hour() {
        let mut policy = EinkRefreshPolicy::new(64, 30 * 60 * 1_000, 5_000);
        let mut deferred = 0;
        let mut full = 0;
        let mut partial = 0;
        for now_ms in (0u64..=60 * 60 * 1_000).step_by(1_000) {
            match policy.for_changed_frame(now_ms, &EinkRefreshUrgency::Telemetry) {
                EinkRefresh::Deferred => deferred += 1,
                EinkRefresh::Full => {
                    full += 1;
                    policy.full_refresh_succeeded(now_ms);
                }
                EinkRefresh::Partial => {
                    partial += 1;
                    policy.partial_refresh_succeeded(now_ms);
                }
            }
        }
        assert_eq!((deferred, full, partial), (2_880, 12, 709));
    }
}
