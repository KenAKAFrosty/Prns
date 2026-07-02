pub use crate::runtime::{
    Diagnostic, Message, PreConfiguredDestination, PrnsApi, PrnsEvent, PrnsRecipe, RuntimeHealth,
    SendError,
};

#[cfg(feature = "tokio-host")]
pub use crate::runtime::{Fleet, Prns, TokioPrnsHandle};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use crate::runtime::{Fleet, Prns};

#[cfg(feature = "embassy-host")]
pub use crate::runtime::EmbassyPrnsHandle;
