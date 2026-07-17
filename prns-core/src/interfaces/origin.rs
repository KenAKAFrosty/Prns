#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceOriginKind {
    Configured,
    Discovered,
}

impl InterfaceOriginKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Discovered => "discovered",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_names_are_stable() {
        assert_eq!(InterfaceOriginKind::Configured.as_str(), "configured");
        assert_eq!(InterfaceOriginKind::Discovered.as_str(), "discovered");
    }
}
