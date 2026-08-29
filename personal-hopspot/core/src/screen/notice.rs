use super::display::{DisplayDuration, MonotonicMillis};
use super::UiNotice;

#[derive(Clone, Copy)]
struct PendingNotice {
    owner: UiNotice,
    duration: DisplayDuration,
}

#[derive(Clone, Copy)]
struct ArmedNotice {
    owner: UiNotice,
    expires_at: MonotonicMillis,
}

pub struct PresentedNoticeTimer {
    pending: Option<PendingNotice>,
    armed: Option<ArmedNotice>,
}

impl PresentedNoticeTimer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending: None,
            armed: None,
        }
    }

    pub fn stage(&mut self, owner: UiNotice, duration: DisplayDuration) {
        self.pending = Some(PendingNotice { owner, duration });
        self.armed = None;
    }

    pub fn reconcile(&mut self, visible: Option<UiNotice>) {
        if self
            .pending
            .is_some_and(|pending| Some(pending.owner) != visible)
        {
            self.pending = None;
        }
        if self.armed.is_some_and(|armed| Some(armed.owner) != visible) {
            self.armed = None;
        }
    }

    pub fn presentation_succeeded(
        &mut self,
        visible: Option<UiNotice>,
        completed_at: MonotonicMillis,
    ) -> Option<UiNotice> {
        self.reconcile(visible);
        let pending = self.pending.take()?;
        self.armed = Some(ArmedNotice {
            owner: pending.owner,
            expires_at: MonotonicMillis::new(
                completed_at
                    .as_millis()
                    .saturating_add(pending.duration.as_millis()),
            ),
        });
        Some(pending.owner)
    }

    pub fn expire(&mut self, now: MonotonicMillis) -> Option<UiNotice> {
        let armed = self.armed?;
        if now < armed.expires_at {
            return None;
        }
        self.armed = None;
        Some(armed.owner)
    }

    #[must_use]
    pub const fn deadline(&self) -> Option<MonotonicMillis> {
        match self.armed {
            Some(armed) => Some(armed.expires_at),
            None => None,
        }
    }
}

impl Default for PresentedNoticeTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: DisplayDuration = match DisplayDuration::from_millis(1_000) {
        Ok(duration) => duration,
        Err(_) => panic!("one second is nonzero"),
    };

    #[test]
    fn lifetime_starts_only_after_the_notice_is_presented() {
        let mut timer = PresentedNoticeTimer::new();
        timer.stage(UiNotice::Announcing, SECOND);

        assert_eq!(timer.deadline(), None);
        assert_eq!(timer.expire(MonotonicMillis::new(10_000)), None);
        assert_eq!(
            timer.presentation_succeeded(Some(UiNotice::Announcing), MonotonicMillis::new(10_000)),
            Some(UiNotice::Announcing)
        );
        assert_eq!(timer.deadline(), Some(MonotonicMillis::new(11_000)));
        assert_eq!(timer.expire(MonotonicMillis::new(10_999)), None);
        assert_eq!(
            timer.expire(MonotonicMillis::new(11_000)),
            Some(UiNotice::Announcing)
        );
    }

    #[test]
    fn replaced_notice_cannot_arm_or_expire_its_predecessor() {
        let mut timer = PresentedNoticeTimer::new();
        timer.stage(UiNotice::Announcing, SECOND);
        timer.reconcile(Some(UiNotice::Awake));

        assert_eq!(
            timer.presentation_succeeded(Some(UiNotice::Awake), MonotonicMillis::new(1_000)),
            None
        );
        assert_eq!(timer.deadline(), None);
    }
}
