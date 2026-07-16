use crate::identity::IdentityHash;
use crate::units::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackholeExpiry {
    Indefinite,
    At(InstantMillis),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackholedIdentity<Reason> {
    pub identity: IdentityHash,
    pub source: IdentityHash,
    pub expiry: BlackholeExpiry,
    pub reason: Option<Reason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackholeIdentityOutcome {
    Added,
    AlreadyPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblackholeIdentityOutcome {
    Removed,
    NotFound,
}
