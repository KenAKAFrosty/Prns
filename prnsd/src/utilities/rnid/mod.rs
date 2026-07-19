mod args;
mod crypto;
mod encoding;
mod identity;
mod io;

use std::io::ErrorKind;

use args::IdentitySource;
pub use args::RnidArgs;
use crypto::LocalCryptoError;
use identity::{LocalIdentity, LocalIdentityError};
use io::IdentityIoError;

#[derive(Debug)]
pub enum RnidError {
    Arguments(args::RnidArgumentError),
    Identity(LocalIdentityError),
    Crypto(LocalCryptoError),
}

pub fn run(args: RnidArgs) -> Result<(), RnidError> {
    if args.version {
        println!(
            "prnsd id {} (RNS 1.3.8 compatibility)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    args.validate_local().map_err(RnidError::Arguments)?;
    let source = args.source();
    let identity = LocalIdentity::resolve(&args).map_err(RnidError::Identity)?;
    let operation = args.print_identity
        || args.export_public
        || args.export_private
        || args.hash.is_some()
        || args.crypto_operation().is_some()
        || args.write.is_some();
    if !operation && !matches!(source, IdentitySource::Generate(_)) {
        print!("{}", crate::cli::id_help());
        return Ok(());
    }
    let identity = identity.ok_or(RnidError::Identity(LocalIdentityError::Missing))?;
    if args.print_identity {
        identity
            .print_information(args.encoding(), args.print_private)
            .map_err(RnidError::Identity)?;
    }
    if args.export_public {
        identity
            .export_public(args.encoding())
            .map_err(RnidError::Identity)?;
    }
    if args.export_private {
        identity
            .export_private(args.encoding())
            .map_err(RnidError::Identity)?;
    }
    if let Some(aspects) = &args.hash {
        identity
            .print_destination_hash(aspects)
            .map_err(RnidError::Identity)?;
    }
    if let Some(operation) = args.crypto_operation() {
        crypto::execute(&args, &identity, operation).map_err(RnidError::Crypto)?;
    }
    identity.write_export(&args).map_err(RnidError::Identity)
}

impl RnidError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Arguments(_) => 250,
            Self::Identity(source) => identity_exit_code(source),
            Self::Crypto(source) => crypto_exit_code(source),
        }
    }
}

fn identity_exit_code(error: &LocalIdentityError) -> u8 {
    match error {
        LocalIdentityError::Missing => 2,
        LocalIdentityError::PublicRequired => 3,
        LocalIdentityError::PrivateRequired => 4,
        LocalIdentityError::DestinationName(_) => 9,
        LocalIdentityError::Io(source) => io_exit_code(source),
        LocalIdentityError::Encoding(_)
        | LocalIdentityError::Material(_)
        | LocalIdentityError::InvalidHash => 8,
        LocalIdentityError::Entropy(_) => 254,
    }
}

fn crypto_exit_code(error: &LocalCryptoError) -> u8 {
    match error {
        LocalCryptoError::Identity(source) => identity_exit_code(source),
        LocalCryptoError::Io(source) => io_exit_code(source),
        LocalCryptoError::InvalidEncryptedFile(_)
        | LocalCryptoError::InvalidSignatureFile { .. } => 7,
        LocalCryptoError::InvalidSignature { .. } => 10,
        LocalCryptoError::Decrypt(_) => 12,
        LocalCryptoError::Entropy(_) | LocalCryptoError::Encrypt(_) => 254,
    }
}

fn io_exit_code(error: &IdentityIoError) -> u8 {
    match error {
        IdentityIoError::AlreadyExists(_) => 11,
        IdentityIoError::Read { source, .. } if source.kind() == ErrorKind::NotFound => 6,
        IdentityIoError::Read { .. } => 252,
        IdentityIoError::Write { .. } | IdentityIoError::InvalidOutputPath(_) => 253,
        IdentityIoError::HomeUnavailable(_) => 254,
    }
}

impl std::fmt::Display for RnidError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Arguments(source) => source.fmt(formatter),
            Self::Identity(source) => source.fmt(formatter),
            Self::Crypto(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for RnidError {}
