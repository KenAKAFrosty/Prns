#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LtoMode {
    #[default]
    Configured,
    Fat,
    Thin,
}

impl LtoMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Fat => "fat",
            Self::Thin => "thin",
        }
    }

    pub(crate) const fn cargo_value(self) -> Option<&'static str> {
        match self {
            Self::Configured => None,
            Self::Fat => Some("fat"),
            Self::Thin => Some("thin"),
        }
    }

    #[must_use]
    pub const fn resolve(self, configured: Self) -> Self {
        match self {
            Self::Configured => configured,
            Self::Fat => Self::Fat,
            Self::Thin => Self::Thin,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BuildIntent {
    #[default]
    Firmware,
    ResourceReport {
        lto: LtoMode,
    },
}

impl BuildIntent {
    #[must_use]
    pub const fn lto(self) -> LtoMode {
        match self {
            Self::Firmware => LtoMode::Configured,
            Self::ResourceReport { lto } => lto,
        }
    }

    pub(crate) const fn is_resource_report(self) -> bool {
        matches!(self, Self::ResourceReport { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_intent_preserves_cargo_release_settings() {
        let intent = BuildIntent::default();
        assert_eq!(intent.lto(), LtoMode::Configured);
        assert!(!intent.is_resource_report());
        assert_eq!(intent.lto().cargo_value(), None);
    }

    #[test]
    fn explicit_lto_modes_have_stable_cargo_values() {
        assert_eq!(LtoMode::Fat.cargo_value(), Some("fat"));
        assert_eq!(LtoMode::Thin.cargo_value(), Some("thin"));
        assert_eq!(LtoMode::Configured.resolve(LtoMode::Thin), LtoMode::Thin);
        assert_eq!(LtoMode::Fat.resolve(LtoMode::Thin), LtoMode::Fat);
        assert!(BuildIntent::ResourceReport { lto: LtoMode::Thin }.is_resource_report());
    }
}
