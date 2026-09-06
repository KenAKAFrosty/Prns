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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildConfiguration {
    lto: LtoMode,
    linker_map: bool,
}

impl BuildConfiguration {
    #[must_use]
    pub const fn new(lto: LtoMode, linker_map: bool) -> Self {
        Self { lto, linker_map }
    }

    #[must_use]
    pub const fn lto(self) -> LtoMode {
        self.lto
    }

    #[must_use]
    pub const fn captures_linker_map(self) -> bool {
        self.linker_map
    }

    pub(crate) const fn isolates_artifacts(self) -> bool {
        self.linker_map || !matches!(self.lto, LtoMode::Configured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_configuration_preserves_cargo_release_settings() {
        let configuration = BuildConfiguration::default();
        assert_eq!(configuration.lto(), LtoMode::Configured);
        assert!(!configuration.captures_linker_map());
        assert_eq!(configuration.lto().cargo_value(), None);
    }

    #[test]
    fn explicit_lto_modes_have_stable_cargo_values() {
        assert_eq!(LtoMode::Fat.cargo_value(), Some("fat"));
        assert_eq!(LtoMode::Thin.cargo_value(), Some("thin"));
        assert!(BuildConfiguration::new(LtoMode::Thin, false).isolates_artifacts());
    }
}
