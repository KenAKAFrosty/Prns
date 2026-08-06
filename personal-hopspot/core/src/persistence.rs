#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PersistenceState {
    Durable,
    Deferred,
    Failed,
}

impl PersistenceState {
    #[must_use]
    pub const fn encode(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn decode(value: u8) -> Self {
        match value {
            value if value == Self::Deferred as u8 => Self::Deferred,
            value if value == Self::Failed as u8 => Self::Failed,
            _ => Self::Durable,
        }
    }
}
