//! Standalone native key image; aggregate images link the same adapter rlib.
#![forbid(unsafe_code)]

prns_identities_uniffi::uniffi_reexport_scaffolding!();

#[cfg(feature = "contact-card")]
prns_identities_uniffi::contact_card::uniffi_reexport_scaffolding!();
