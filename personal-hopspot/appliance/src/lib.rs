//! Signed Linux application slots; vendor firmware and node state are separate owners.
#![forbid(unsafe_code)]

mod activation;
mod filesystem;
mod package;
mod radio;
mod storage;

pub use activation::{
    Activation, CandidateId, ExecutableDigest, Fallback, LaunchBudget, Slot, Status,
};
pub use package::{Board, Budgets, PackageError, VerifiedPackage};
pub use radio::transaction::{
    BootId, BootTime, FileMode, ObserveRadioWrites, RadioCandidateId, RadioCheckpoint, RadioDigest,
    RadioFile, RadioFileImage, RadioLease, RadioPhase, RadioPlatform, RadioPreparation,
    RadioRecoveryProgress, RadioSnapshot, RadioTransaction, RadioTransactionError,
    RadioTransactionState, RadioTransactionStatus, RadioVendorOperation, RecoveryWindow,
    RestoredRadioReadiness, TrialBoots,
};
pub use radio::{
    MeshId, MeshPathSetup, RadioBinding, RadioDevice, RadioPlan, RadioPreset, RadioProfile,
    RadioProfileError, RegionalChannel, UciSection,
};
pub use storage::{Appliance, Checkpoint, Error, ObserveWrites, SpaceBudget, UnobservedWrites};
