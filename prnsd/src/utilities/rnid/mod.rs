mod args;
mod artifact;
mod crypto;
mod encoding;
mod identity;
mod io;
mod network;

use std::io::ErrorKind;
use std::path::PathBuf;

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
    Artifact(artifact::ArtifactError),
    Network(network::IdentityNetworkError),
}

pub async fn run(args: RnidArgs) -> Result<(), RnidError> {
    if args.version {
        println!(
            "prnsd id {} (RNS 1.4.0 compatibility)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    args.validate_local().map_err(RnidError::Arguments)?;
    let source = args.source();
    let identity = LocalIdentity::resolve(&args).map_err(RnidError::Identity)?;
    let identity = network::resolve(&args, identity)
        .await
        .map_err(RnidError::Network)?;
    let operation = args.print_identity
        || args.export_public
        || args.export_private
        || args.hash.is_some()
        || args.crypto_operation().is_some()
        || args.sign_message.is_some()
        || args.announce.is_some()
        || args.write.is_some();
    if !operation && !matches!(source, IdentitySource::Generate(_)) {
        print!("{}", crate::cli::id_help());
        return Ok(());
    }
    let identity_required = args.print_identity
        || args.export_public
        || args.export_private
        || args.hash.is_some()
        || args.encrypt.is_some()
        || args.decrypt.is_some()
        || args.sign.is_some()
        || args.sign_message.is_some()
        || args.announce.is_some()
        || (args.write.is_some() && args.sign_message.is_none());
    if identity_required && identity.is_none() {
        return Err(RnidError::Identity(LocalIdentityError::Missing));
    }
    if args.print_identity {
        identity
            .as_ref()
            .ok_or(RnidError::Identity(LocalIdentityError::Missing))?
            .print_information(args.encoding(), args.print_private)
            .map_err(RnidError::Identity)?;
    }
    if args.export_public {
        identity
            .as_ref()
            .ok_or(RnidError::Identity(LocalIdentityError::Missing))?
            .export_public(args.encoding())
            .map_err(RnidError::Identity)?;
    }
    if args.export_private {
        identity
            .as_ref()
            .ok_or(RnidError::Identity(LocalIdentityError::Missing))?
            .export_private(args.encoding())
            .map_err(RnidError::Identity)?;
    }
    if let Some(aspects) = &args.hash {
        identity
            .as_ref()
            .ok_or(RnidError::Identity(LocalIdentityError::Missing))?
            .print_destination_hash(aspects)
            .map_err(RnidError::Identity)?;
    }
    if let Some(aspects) = &args.announce {
        network::announce(
            &args,
            identity
                .as_ref()
                .ok_or(RnidError::Identity(LocalIdentityError::Missing))?,
            aspects,
        )
        .await
        .map_err(RnidError::Network)?;
    }
    if let Some(operation) = args.crypto_operation() {
        crypto::execute(&args, identity.as_ref(), operation).map_err(RnidError::Crypto)?;
    }
    if args.sign_message.is_some() {
        let identity = identity
            .as_ref()
            .ok_or(RnidError::Identity(LocalIdentityError::Missing))?;
        let signed =
            artifact::create_message_signature(&args, identity).map_err(RnidError::Artifact)?;
        if let Some(encoding) = args.explicit_encoding() {
            println!("\n{}\n", artifact::encoded_artifact(&signed, encoding));
            println!(
                "Message signed with {}",
                identity::pretty_hash(identity.identity_hash())
            );
        } else {
            let (mut output, path) = signed_message_output(&args).map_err(RnidError::Crypto)?;
            crypto::write_output(&mut output, &path, &signed).map_err(RnidError::Crypto)?;
            output
                .finish()
                .map_err(|source| RnidError::Crypto(LocalCryptoError::Io(source)))?;
            println!(
                "Message signed with {} saved to {}",
                identity::pretty_hash(identity.identity_hash()),
                path.display()
            );
        }
    }
    match identity {
        Some(identity) => identity.write_export(&args).map_err(RnidError::Identity),
        None => Ok(()),
    }
}

fn signed_message_output(args: &RnidArgs) -> Result<(io::OutputSink, PathBuf), LocalCryptoError> {
    if args.stdout {
        return crypto::open_output(args, None);
    }
    let path = args
        .write
        .as_deref()
        .ok_or_else(|| LocalCryptoError::Io(IdentityIoError::InvalidOutputPath(PathBuf::new())))?;
    let path = if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rsm"))
    {
        path.to_owned()
    } else {
        crypto::append_suffix(path, ".rsm")
    };
    let overwrite = if args.force {
        io::OverwritePolicy::Replace
    } else {
        io::OverwritePolicy::Refuse
    };
    let output = io::OutputSink::file(&path, overwrite, io::OutputSensitivity::Ordinary)
        .map_err(LocalCryptoError::Io)?;
    Ok((output, path))
}

impl RnidError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Arguments(_) => 250,
            Self::Identity(source) => identity_exit_code(source),
            Self::Crypto(source) => crypto_exit_code(source),
            Self::Artifact(source) => artifact_exit_code(source),
            Self::Network(source) => network_exit_code(source),
        }
    }
}

fn network_exit_code(error: &network::IdentityNetworkError) -> u8 {
    match error {
        network::IdentityNetworkError::LookupTimedOut { .. }
        | network::IdentityNetworkError::Identity(LocalIdentityError::Missing) => 2,
        network::IdentityNetworkError::Identity(source) => identity_exit_code(source),
        network::IdentityNetworkError::DestinationName(_)
        | network::IdentityNetworkError::InvalidAspects => 9,
        network::IdentityNetworkError::Configuration(_)
        | network::IdentityNetworkError::Session(_)
        | network::IdentityNetworkError::NodeStopped(_)
        | network::IdentityNetworkError::Announce(_) => 254,
    }
}

fn artifact_exit_code(error: &artifact::ArtifactError) -> u8 {
    match error {
        artifact::ArtifactError::Identity(source) => identity_exit_code(source),
        artifact::ArtifactError::Io(source) => io_exit_code(source),
        artifact::ArtifactError::Artifact(_) => 10,
        artifact::ArtifactError::MetadataConfig(_)
        | artifact::ArtifactError::MetadataSpecConfig(_)
        | artifact::ArtifactError::MetadataSpec(_)
        | artifact::ArtifactError::MessageEncoding(_) => 7,
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
        LocalCryptoError::Artifact(source) => artifact_exit_code(source),
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
            Self::Artifact(source) => source.fmt(formatter),
            Self::Network(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for RnidError {}
