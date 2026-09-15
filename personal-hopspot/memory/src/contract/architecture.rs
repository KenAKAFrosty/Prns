#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessorArchitecture {
    ThumbV7em,
    RiscV32Imac,
    XtensaEsp32S3,
}

impl ProcessorArchitecture {
    pub const ALL: [Self; 3] = [Self::ThumbV7em, Self::RiscV32Imac, Self::XtensaEsp32S3];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ThumbV7em => "thumbv7em",
            Self::RiscV32Imac => "riscv32imac",
            Self::XtensaEsp32S3 => "xtensa-esp32s3",
        }
    }

    #[must_use]
    pub const fn rust_target(self) -> &'static str {
        match self {
            Self::ThumbV7em => "thumbv7em-none-eabihf",
            Self::RiscV32Imac => "riscv32imac-unknown-none-elf",
            Self::XtensaEsp32S3 => "xtensa-esp32s3-none-elf",
        }
    }

    #[must_use]
    pub fn from_rust_target(target: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|architecture| architecture.rust_target() == target)
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessorArchitecture;

    #[test]
    fn stable_ids_and_rust_targets_round_trip() {
        for architecture in ProcessorArchitecture::ALL {
            assert_eq!(
                ProcessorArchitecture::from_rust_target(architecture.rust_target()),
                Some(architecture)
            );
            assert!(!architecture.id().is_empty());
        }
        assert_eq!(ProcessorArchitecture::from_rust_target("unknown"), None);
    }
}
