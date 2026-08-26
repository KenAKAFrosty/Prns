#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservedAuthentication {
    Unknown,
    Open,
    Legacy,
    Wpa2,
    Wpa2Wpa3,
    Wpa3,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StationSecurity {
    Open,
    Wpa2,
    Wpa2Wpa3,
    Wpa3,
}

impl ObservedAuthentication {
    pub(crate) const fn compatible_station_security(
        self,
        password_is_empty: bool,
    ) -> Option<StationSecurity> {
        match (self, password_is_empty) {
            (Self::Open, true) => Some(StationSecurity::Open),
            (Self::Wpa2, false) => Some(StationSecurity::Wpa2),
            (Self::Wpa2Wpa3, false) => Some(StationSecurity::Wpa2Wpa3),
            (Self::Wpa3, false) => Some(StationSecurity::Wpa3),
            (
                Self::Unknown
                | Self::Open
                | Self::Legacy
                | Self::Wpa2
                | Self::Wpa2Wpa3
                | Self::Wpa3
                | Self::Unsupported,
                _,
            ) => None,
        }
    }
}

impl StationSecurity {
    pub(crate) const fn requires_pmf(self) -> bool {
        match self {
            Self::Wpa3 => true,
            Self::Open | Self::Wpa2 | Self::Wpa2Wpa3 => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_authentication_enforces_the_provisioned_security_floor() {
        let cases = [
            (ObservedAuthentication::Unknown, true, None),
            (ObservedAuthentication::Unknown, false, None),
            (
                ObservedAuthentication::Open,
                true,
                Some(StationSecurity::Open),
            ),
            (ObservedAuthentication::Open, false, None),
            (ObservedAuthentication::Legacy, true, None),
            (ObservedAuthentication::Legacy, false, None),
            (ObservedAuthentication::Wpa2, true, None),
            (
                ObservedAuthentication::Wpa2,
                false,
                Some(StationSecurity::Wpa2),
            ),
            (ObservedAuthentication::Wpa2Wpa3, true, None),
            (
                ObservedAuthentication::Wpa2Wpa3,
                false,
                Some(StationSecurity::Wpa2Wpa3),
            ),
            (ObservedAuthentication::Wpa3, true, None),
            (
                ObservedAuthentication::Wpa3,
                false,
                Some(StationSecurity::Wpa3),
            ),
            (ObservedAuthentication::Unsupported, true, None),
            (ObservedAuthentication::Unsupported, false, None),
        ];

        for (observed, password_is_empty, expected) in cases {
            assert_eq!(
                observed.compatible_station_security(password_is_empty),
                expected,
                "observed={observed:?} password_is_empty={password_is_empty}"
            );
        }
    }

    #[test]
    fn only_pure_wpa3_requires_pmf() {
        assert!(!StationSecurity::Open.requires_pmf());
        assert!(!StationSecurity::Wpa2.requires_pmf());
        assert!(!StationSecurity::Wpa2Wpa3.requires_pmf());
        assert!(StationSecurity::Wpa3.requires_pmf());
    }
}
