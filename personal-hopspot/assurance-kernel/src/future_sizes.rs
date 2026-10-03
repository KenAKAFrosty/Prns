pub struct FutureSizeScenario {
    identifier: &'static str,
    rustc_type: &'static str,
}

impl FutureSizeScenario {
    const fn new(identifier: &'static str, rustc_type: &'static str) -> Self {
        Self {
            identifier,
            rustc_type,
        }
    }

    #[must_use]
    pub const fn identifier(&self) -> &'static str {
        self.identifier
    }

    #[must_use]
    pub const fn rustc_type(&self) -> &'static str {
        self.rustc_type
    }
}

pub const SCENARIOS: [FutureSizeScenario; 2] = [
    FutureSizeScenario::new("sx126x", "{async fn body of sx126x::run()}"),
    FutureSizeScenario::new("lr1110", "{async fn body of lr1110::run()}"),
];
