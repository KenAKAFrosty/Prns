pub const REMOTE_CONTROL_WIFI_CONFIRMATION_WINDOW_SECONDS: u8 = 120;

macro_rules! desired_state {
    ($name:ident { $($variant:ident = $wire:expr),+ $(,)? }) => {
        prns_macros::iterable_enum! {
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            #[repr(u8)]
            pub enum $name {
                $($variant = $wire),+
            }
        }

        impl $name {
            #[must_use]
            pub const fn wire_value(self) -> u8 {
                self as u8
            }

            pub(crate) fn from_wire(value: u8) -> Option<Self> {
                Self::ALL
                    .into_iter()
                    .find(|candidate| candidate.wire_value() == value)
            }
        }
    };
}

desired_state!(RemoteControlSystemPower {
    Awake = 0x01,
    Asleep = 0x02,
});

desired_state!(RemoteControlGnssPower {
    Off = 0x00,
    On = 0x01,
});

desired_state!(RemoteControlDisplayVisibility {
    Hidden = 0x00,
    Visible = 0x01,
});

desired_state!(RemoteControlDisplayAutoOff {
    Disabled = 0x00,
    Enabled = 0x01,
});

desired_state!(RemoteControlStationUplink {
    Disabled = 0x00,
    Enabled = 0x01,
});

desired_state!(RemoteControlEspRadioMode {
    Bluetooth = 0x01,
    AccessPoint = 0x02,
});

desired_state!(RemoteControlApplyOutcome {
    Applied = 0x01,
    Unchanged = 0x02,
    Scheduled = 0x03,
});

impl RemoteControlApplyOutcome {
    pub const ENCODED_LEN: usize = 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RemoteControlWifiCredentialRevision(u32);

impl RemoteControlWifiCredentialRevision {
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    pub(crate) const fn from_wire(bytes: [u8; 4]) -> Option<Self> {
        Self::new(u32::from_be_bytes(bytes))
    }

    pub(crate) const fn wire_bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlWifiStageOutcome {
    Staged(RemoteControlWifiCredentialRevision),
    InvalidCredentials,
}

impl RemoteControlWifiStageOutcome {
    pub const MAX_ENCODED_LEN: usize = 5;

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::Staged(_) => Self::MAX_ENCODED_LEN,
            Self::InvalidCredentials => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlWifiConfirmationRemaining(u8);

impl RemoteControlWifiConfirmationRemaining {
    #[must_use]
    pub const fn new(seconds: u8) -> Option<Self> {
        if seconds <= REMOTE_CONTROL_WIFI_CONFIRMATION_WINDOW_SECONDS {
            Some(Self(seconds))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn seconds(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlWifiTransactionStatus {
    FactoryProvisioning,
    Confirmed {
        revision: RemoteControlWifiCredentialRevision,
    },
    Staged {
        revision: RemoteControlWifiCredentialRevision,
    },
    AwaitingConfirmation {
        revision: RemoteControlWifiCredentialRevision,
        remaining: RemoteControlWifiConfirmationRemaining,
    },
    RollingBack {
        rejected_revision: RemoteControlWifiCredentialRevision,
    },
}

impl RemoteControlWifiTransactionStatus {
    pub const MAX_ENCODED_LEN: usize = 6;

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::FactoryProvisioning => 1,
            Self::Confirmed { .. } | Self::Staged { .. } | Self::RollingBack { .. } => 5,
            Self::AwaitingConfirmation { .. } => Self::MAX_ENCODED_LEN,
        }
    }
}
