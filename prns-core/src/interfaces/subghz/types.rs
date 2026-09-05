use crate::interfaces::AirtimeDutyCycle;

const DUTY_ONE_PERCENT_PER_MILLE: u16 = 10;
const DUTY_QUEUE_BUDGET_MS: u32 = 4_000;
const DUTY_TEN_PERCENT_PER_MILLE: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frequency(u32);

impl Frequency {
    pub const fn new(hz: u32) -> Self {
        Self(hz)
    }

    pub const fn hz(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxPower(i8);

impl TxPower {
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    pub const fn dbm(self) -> i8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonotonicMicros(u64);

impl MonotonicMicros {
    pub const fn new(micros: u64) -> Self {
        Self(micros)
    }

    pub const fn micros(self) -> u64 {
        self.0
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Region {
        Us915,
        Au915,
        Eu433,
        Eu865,
        Eu868,
        Eu869,
        As923,
        In865,
        Cn470,
        Kr920,
        Jp920,
        Unlimited,
    }
}

impl Region {
    pub const fn band(self) -> (u32, u32) {
        match self {
            Self::Us915 => (902_000_000, 928_000_000),
            Self::Au915 => (915_000_000, 928_000_000),
            Self::Eu433 => (433_050_000, 434_790_000),
            Self::Eu865 => (865_000_000, 868_000_000),
            Self::Eu868 => (868_000_000, 868_600_000),
            Self::Eu869 => (869_400_000, 869_650_000),
            Self::As923 => (920_000_000, 925_000_000),
            Self::In865 => (865_000_000, 867_000_000),
            Self::Cn470 => (470_000_000, 510_000_000),
            Self::Kr920 => (920_000_000, 923_000_000),
            Self::Jp920 => (920_800_000, 927_800_000),
            Self::Unlimited => (150_000_000, 960_000_000),
        }
    }

    pub const fn default_frequency(self) -> Frequency {
        let hz = match self {
            Self::Us915 | Self::Au915 => 921_500_000,
            Self::Eu433 => 433_900_000,
            Self::Eu865 => 866_500_000,
            Self::Eu868 => 868_300_000,
            Self::Eu869 => 869_500_000,
            Self::As923 => 922_500_000,
            Self::In865 => 866_000_000,
            Self::Cn470 => 490_000_000,
            Self::Kr920 => 921_500_000,
            Self::Jp920 => 922_000_000,
            Self::Unlimited => 915_000_000,
        };
        Frequency::new(hz)
    }

    pub const fn max_tx_power(self) -> TxPower {
        let dbm = match self {
            Self::Us915 | Self::Au915 | Self::In865 | Self::Eu869 | Self::Unlimited => 22,
            Self::Cn470 => 19,
            Self::As923 | Self::Jp920 => 16,
            Self::Eu865 | Self::Eu868 | Self::Kr920 => 14,
            Self::Eu433 => 12,
        };
        TxPower::new(dbm)
    }

    pub const fn regulatory_duty_cycle(self) -> Option<AirtimeDutyCycle> {
        let limit_long_per_mille = match self {
            Self::Eu865 | Self::Eu868 => DUTY_ONE_PERCENT_PER_MILLE,
            Self::Eu433 | Self::Eu869 => DUTY_TEN_PERCENT_PER_MILLE,
            _ => return None,
        };
        Some(AirtimeDutyCycle {
            limit_short_per_mille: None,
            limit_long_per_mille: Some(limit_long_per_mille),
            max_queued_airtime_ms: DUTY_QUEUE_BUDGET_MS,
        })
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Us915 => "US915",
            Self::Au915 => "AU915",
            Self::Eu433 => "EU433",
            Self::Eu865 => "EU865",
            Self::Eu868 => "EU868",
            Self::Eu869 => "EU869",
            Self::As923 => "AS923",
            Self::In865 => "IN865",
            Self::Cn470 => "CN470",
            Self::Kr920 => "KR920",
            Self::Jp920 => "JP920",
            Self::Unlimited => "Custom",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Us915 => Self::Au915,
            Self::Au915 => Self::Eu433,
            Self::Eu433 => Self::Eu865,
            Self::Eu865 => Self::Eu868,
            Self::Eu868 => Self::Eu869,
            Self::Eu869 => Self::As923,
            Self::As923 => Self::In865,
            Self::In865 => Self::Cn470,
            Self::Cn470 => Self::Kr920,
            Self::Kr920 => Self::Jp920,
            Self::Jp920 => Self::Unlimited,
            Self::Unlimited => Self::Us915,
        }
    }
}
