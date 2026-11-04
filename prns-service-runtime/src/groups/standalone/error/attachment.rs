use super::{network, AttachmentFailure, StoreFailure};
use crate::groups::{self, SchemaError};
use prns_identities::owned;
use std::convert::Infallible;

pub(super) fn classify(error: &network::DriverError) -> AttachmentFailure {
    use network::DriverError as E;
    match error {
        E::Local(error) => local(error),
        E::Runtime(reason) => identity_runtime(*reason),
        E::InvalidOptions | E::MissingOwner | E::WrongOwner => AttachmentFailure::Configuration,
        E::Failure(error) => retained(error),
        E::Announce(_) | E::Network(_) | E::Registration(_) => AttachmentFailure::Network,
        #[cfg(feature = "contact-card")]
        E::Contact(error) => AttachmentFailure::Identity { error: *error },
    }
}
fn retained(error: &network::Failure) -> AttachmentFailure {
    use network::Failure as E;
    match error {
        E::Group(error) => AttachmentFailure::Group {
            error: error.into(),
        },
        #[cfg(feature = "contact-card")]
        E::Contact(error) => AttachmentFailure::Identity { error: *error },
        E::Scope(_)
        | E::Service(_)
        | E::UnexpectedReceiveState
        | E::WrongDestination
        | E::TaskCancelled
        | E::TaskPanicked => AttachmentFailure::Network,
        #[cfg(feature = "contact-card")]
        E::AnnounceData(_) => AttachmentFailure::Network,
    }
}
fn identity_runtime(reason: prns_identities_runtime::Fault) -> AttachmentFailure {
    AttachmentFailure::Identity {
        error: owned::Error::<Infallible>::Runtime(reason).api_error(|never| match *never {}),
    }
}
fn local(error: &groups::Error) -> AttachmentFailure {
    use groups::Error as E;
    match error {
        E::Schema(error) => schema(error),
        E::Store(error) => store(error),
        E::Observation(error) => store(error),
        E::Runtime(reason) => identity_runtime(*reason),
        E::UnsupportedService(_) | E::Observing => AttachmentFailure::Configuration,
        E::NetworkUnavailable => AttachmentFailure::Network,
        E::Allocation => AttachmentFailure::Allocation,
        E::Capacity => AttachmentFailure::Capacity,
    }
}
fn store(error: &crate::stores::Error<SchemaError>) -> AttachmentFailure {
    match super::store_failure(error) {
        Ok(reason) => AttachmentFailure::Storage { reason },
        Err(error) => schema(error),
    }
}
fn schema(error: &SchemaError) -> AttachmentFailure {
    match error {
        SchemaError::Binding(error) => AttachmentFailure::Storage {
            reason: super::binding_failure(error),
        },
        SchemaError::Group(error) => AttachmentFailure::Group {
            error: error.into(),
        },
        SchemaError::Database(_) => AttachmentFailure::Storage {
            reason: StoreFailure::Database,
        },
        SchemaError::Entropy(_) => AttachmentFailure::Entropy,
    }
}

pub(super) fn detail(error: &impl std::fmt::Debug) -> String {
    use std::fmt::Write;
    struct Bounded(String);
    impl Write for Bounded {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            let mut end = value.len().min(1024 - self.0.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&value[..end]);
            if end == value.len() {
                Ok(())
            } else {
                Err(std::fmt::Error)
            }
        }
    }
    let mut output = Bounded(String::new());
    let _truncated = write!(&mut output, "{error:?}");
    output.0
}
