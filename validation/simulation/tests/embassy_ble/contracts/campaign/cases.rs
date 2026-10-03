#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Expire,
    Cancel,
    Reconnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Profile {
    ShortDeadline,
    Fragmented,
}

impl Profile {
    pub(super) fn timeout_ms(self) -> u64 {
        match self {
            Self::ShortDeadline => 1,
            Self::Fragmented => 50,
        }
    }

    pub(super) fn payload(self, phase: usize) -> Vec<u8> {
        let len = match self {
            Self::ShortDeadline => 1,
            Self::Fragmented => crate::node::PAYLOAD_BYTES,
        };
        (0..len).map(|offset| (offset + phase) as u8).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Case {
    pub actions: [Action; 3],
    pub profile: Profile,
}

pub(super) fn cases() -> Vec<Case> {
    let mut cases = Vec::with_capacity(12);
    let actions = [Action::Expire, Action::Cancel, Action::Reconnect];
    for first in actions {
        for second in actions {
            if second == first {
                continue;
            }
            for third in actions {
                if third == first || third == second {
                    continue;
                }
                for profile in [Profile::ShortDeadline, Profile::Fragmented] {
                    cases.push(Case {
                        actions: [first, second, third],
                        profile,
                    });
                }
            }
        }
    }
    cases
}
