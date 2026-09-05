#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessorArchitecture {
    ThumbV7em,
    RiscV32Imac,
    XtensaEsp32S3,
}

impl ProcessorArchitecture {
    #[must_use]
    pub const fn rust_target(self) -> &'static str {
        match self {
            Self::ThumbV7em => "thumbv7em-none-eabihf",
            Self::RiscV32Imac => "riscv32imac-unknown-none-elf",
            Self::XtensaEsp32S3 => "xtensa-esp32s3-none-elf",
        }
    }
}
