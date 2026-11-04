//! Startup-only publication ownership. A fresh service object is the callback epoch.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RestoredProfile {
    Current,
    Legacy,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Attribute {
    Control,
    Data,
    ColumbaRx,
    ColumbaTx,
    ColumbaIdentity,
    Liveness,
}

#[derive(Default)]
pub(super) struct RestoredShape {
    seen: u8,
    invalid: bool,
}

impl RestoredShape {
    pub(super) fn observe(&mut self, attribute: Attribute, valid_properties: bool) {
        let bit = 1 << attribute as u8;
        self.invalid |= self.seen & bit != 0 || !valid_properties;
        self.seen |= bit;
    }

    pub(super) fn profile(&self) -> RestoredProfile {
        if self.invalid {
            return RestoredProfile::Invalid;
        }
        match self.seen {
            0b11_1111 => RestoredProfile::Current,
            0b01_1111 => RestoredProfile::Legacy,
            _ => RestoredProfile::Invalid,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Empty,
    Legacy,
    Adding,
    Published,
    Failed,
}

pub(super) struct ServicePublication<T> {
    phase: Phase,
    service: Option<T>,
}

impl<T> Default for ServicePublication<T> {
    fn default() -> Self {
        Self {
            phase: Phase::Empty,
            service: None,
        }
    }
}

impl<T> ServicePublication<T> {
    pub(super) fn is_empty(&self) -> bool {
        self.phase == Phase::Empty
    }

    pub(super) fn is_published(&self) -> bool {
        self.phase == Phase::Published
    }

    pub(super) fn restore(&mut self, service: T, profile: RestoredProfile) -> bool {
        if !self.is_empty() || profile == RestoredProfile::Invalid {
            return false;
        }
        self.phase = if profile == RestoredProfile::Current {
            Phase::Published
        } else {
            Phase::Legacy
        };
        self.service = Some(service);
        true
    }

    pub(super) fn can_publish(&self, powered: bool, owners_empty: bool) -> bool {
        powered && owners_empty && matches!(self.phase, Phase::Empty | Phase::Legacy)
    }

    /// The returned legacy service is the only permitted removal target. Allocate the new
    /// object before dropping this retained old object, so their identities cannot alias.
    pub(super) fn begin(
        &mut self,
        powered: bool,
        owners_empty: bool,
        service: T,
    ) -> Result<Option<T>, T> {
        if !self.can_publish(powered, owners_empty) {
            return Err(service);
        }
        self.phase = Phase::Adding;
        Ok(self.service.replace(service))
    }

    /// Match both success and failure before changing readiness. Failed attempts stay terminal;
    /// a duplicate or delayed callback can never publish a different service.
    pub(super) fn complete(&mut self, matches: impl FnOnce(&T) -> bool, success: bool) -> bool {
        if self.phase != Phase::Adding || !self.service.as_ref().is_some_and(matches) {
            return false;
        }
        self.phase = if success {
            Phase::Published
        } else {
            Phase::Failed
        };
        true
    }

    pub(super) fn fail_unrestored(&mut self) {
        if self.is_empty() {
            self.phase = Phase::Failed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(liveness: bool) -> RestoredShape {
        let mut shape = RestoredShape::default();
        for attribute in [
            Attribute::Control,
            Attribute::Data,
            Attribute::ColumbaRx,
            Attribute::ColumbaTx,
            Attribute::ColumbaIdentity,
        ] {
            shape.observe(attribute, true);
        }
        if liveness {
            shape.observe(Attribute::Liveness, true);
        }
        shape
    }

    #[test]
    fn restored_shapes_require_a_complete_unique_profile() {
        assert_eq!(shape(false).profile(), RestoredProfile::Legacy);
        assert_eq!(shape(true).profile(), RestoredProfile::Current);
        assert_eq!(RestoredShape::default().profile(), RestoredProfile::Invalid);
        let mut unreadable = shape(false);
        unreadable.observe(Attribute::Liveness, false);
        assert_eq!(unreadable.profile(), RestoredProfile::Invalid);
        let mut duplicate = shape(true);
        duplicate.observe(Attribute::Control, true);
        assert_eq!(duplicate.profile(), RestoredProfile::Invalid);
    }

    #[test]
    fn current_restoration_preserves_its_owner_without_republication() {
        let mut state = ServicePublication::default();
        assert!(state.restore(10, RestoredProfile::Current));
        assert!(state.is_published());
        assert!(!state.can_publish(true, true));
        assert_eq!(state.begin(true, true, 11), Err(11));
        assert!(!state.restore(12, RestoredProfile::Legacy));
        assert!(!state.complete(|id| *id == 10, false));
        assert!(state.is_published());
    }

    #[test]
    fn legacy_migration_waits_for_power_and_no_live_owners_then_runs_once() {
        let mut state = ServicePublication::default();
        assert!(state.restore(10, RestoredProfile::Legacy));
        assert!(!state.is_published());
        assert_eq!(state.begin(false, true, 11), Err(11));
        assert_eq!(state.begin(true, false, 11), Err(11));
        assert_eq!(state.begin(true, true, 11), Ok(Some(10)));
        assert!(!state.is_published());
        assert_eq!(state.begin(true, true, 12), Err(12));
        assert!(!state.complete(|id| *id == 10, true));
        assert!(!state.complete(|id| *id == 10, false));
        assert!(state.complete(|id| *id == 11, true));
        assert!(state.is_published());
        assert!(!state.complete(|id| *id == 11, false));
        assert!(state.is_published());
    }

    #[test]
    fn exact_publication_failure_cannot_be_revived_by_late_success() {
        let mut state = ServicePublication::default();
        assert_eq!(state.begin(true, true, 20), Ok(None));
        assert!(state.complete(|id| *id == 20, false));
        assert!(!state.complete(|id| *id == 20, true));
        assert!(!state.is_published());
        assert!(!state.can_publish(true, true));
        let mut malformed = ServicePublication::<u8>::default();
        malformed.fail_unrestored();
        assert!(!malformed.can_publish(true, true));
    }
}
