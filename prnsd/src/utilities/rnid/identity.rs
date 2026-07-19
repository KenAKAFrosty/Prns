use std::path::{Path, PathBuf};

use personal_rns::identity::{
    IdentityHash, IdentityMaterialLengthError, PrivateIdentityMaterial, PublicIdentityMaterial,
    IDENTITY_PUBLIC_KEY_LEN, IDENTITY_SECRET_KEY_LEN,
};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::runtime::{try_generate_identity_secret, OsEntropyError};

use super::args::{IdentityEncoding, IdentitySource, RnidArgs};
use super::encoding::{decode_identity, encode, IdentityEncodingError};
use super::io::{
    expand_user_path, read_file, IdentityIoError, OutputSensitivity, OutputSink, OverwritePolicy,
};

pub enum LocalIdentity {
    Private(PrivateIdentityMaterial),
    Public(PublicIdentityMaterial),
    Hash(IdentityHash),
}

#[derive(Debug)]
pub enum LocalIdentityError {
    Io(IdentityIoError),
    Encoding(IdentityEncodingError),
    Material(IdentityMaterialLengthError),
    Entropy(OsEntropyError),
    InvalidHash,
    Missing,
    PublicRequired,
    PrivateRequired,
    DestinationName(ExpandNameError),
}

impl LocalIdentity {
    pub fn resolve(args: &RnidArgs) -> Result<Option<Self>, LocalIdentityError> {
        match args.source() {
            IdentitySource::None => Ok(None),
            IdentitySource::Generate(path) => {
                let secret = try_generate_identity_secret().map_err(LocalIdentityError::Entropy)?;
                let identity = PrivateIdentityMaterial::from(secret);
                write_bytes(
                    path,
                    identity.as_bytes(),
                    args.force,
                    OutputSensitivity::Private,
                )?;
                print_status(
                    args,
                    format_args!(
                        "New identity {} written to {}",
                        pretty_hash(identity.identity_hash()),
                        path.display()
                    ),
                );
                Ok(Some(Self::Private(identity)))
            }
            IdentitySource::Identity(value) => {
                let path = expand_user_path(Path::new(value)).map_err(LocalIdentityError::Io)?;
                if path.is_file() {
                    let bytes = read_file(&path).map_err(LocalIdentityError::Io)?;
                    let identity = PrivateIdentityMaterial::from_slice(&bytes)
                        .map_err(LocalIdentityError::Material)?;
                    print_status(
                        args,
                        format_args!(
                            "Loaded Identity {} from {}",
                            pretty_hash(identity.identity_hash()),
                            path.display()
                        ),
                    );
                    Ok(Some(Self::Private(identity)))
                } else {
                    parse_hash(value).map(Self::Hash).map(Some)
                }
            }
            IdentitySource::ImportPublic(value) => {
                let bytes = import_bytes(value, args.explicit_encoding(), IDENTITY_PUBLIC_KEY_LEN)?;
                let identity = PublicIdentityMaterial::from_slice(&bytes)
                    .map_err(LocalIdentityError::Material)?;
                print_status(
                    args,
                    format_args!("Reticulum Identity imported as public identity"),
                );
                Ok(Some(Self::Public(identity)))
            }
            IdentitySource::ImportPrivate(value) => {
                let bytes = import_bytes(value, args.explicit_encoding(), IDENTITY_SECRET_KEY_LEN)?;
                let identity = PrivateIdentityMaterial::from_slice(&bytes)
                    .map_err(LocalIdentityError::Material)?;
                print_status(
                    args,
                    format_args!("Reticulum Identity imported as private identity"),
                );
                Ok(Some(Self::Private(identity)))
            }
        }
    }

    pub fn identity_hash(&self) -> IdentityHash {
        match self {
            Self::Private(identity) => identity.identity_hash(),
            Self::Public(identity) => identity.identity_hash(),
            Self::Hash(identity) => *identity,
        }
    }

    pub fn public(&self) -> Result<PublicIdentityMaterial, LocalIdentityError> {
        match self {
            Self::Private(identity) => Ok(identity.public()),
            Self::Public(identity) => Ok(*identity),
            Self::Hash(_) => Err(LocalIdentityError::PublicRequired),
        }
    }

    pub fn private(&self) -> Result<&PrivateIdentityMaterial, LocalIdentityError> {
        match self {
            Self::Private(identity) => Ok(identity),
            Self::Public(_) | Self::Hash(_) => Err(LocalIdentityError::PrivateRequired),
        }
    }

    pub fn print_information(
        &self,
        encoding: IdentityEncoding,
        reveal_private: bool,
    ) -> Result<(), LocalIdentityError> {
        let public = self.public()?;
        println!("Identity Hash : {}", pretty_hash(self.identity_hash()));
        println!("Public Key    : {}", encode(public.as_bytes(), encoding));
        if let Self::Private(private) = self {
            if reveal_private {
                println!("Private Key   : {}", encode(private.as_bytes(), encoding));
            } else {
                println!("Private Key   : Hidden");
            }
        }
        Ok(())
    }

    pub fn export_public(&self, encoding: IdentityEncoding) -> Result<(), LocalIdentityError> {
        let public = self.public()?;
        println!(
            "Public Identity Keys  : {}",
            encode(public.as_bytes(), encoding)
        );
        Ok(())
    }

    pub fn export_private(&self, encoding: IdentityEncoding) -> Result<(), LocalIdentityError> {
        let private = self.private()?;
        println!(
            "Private Identity Keys : {}",
            encode(private.as_bytes(), encoding)
        );
        Ok(())
    }

    pub fn print_destination_hash(&self, full_name: &str) -> Result<(), LocalIdentityError> {
        let mut components = full_name.split('.');
        let app_name = components.next().unwrap_or_default();
        let aspects: Vec<_> = components.collect();
        let destination = derive_single_destination_hash(&self.identity_hash(), app_name, &aspects)
            .map_err(LocalIdentityError::DestinationName)?;
        println!(
            "The {full_name} destination for this Identity is {}",
            pretty_destination(destination.as_bytes())
        );
        if !matches!(self, Self::Hash(_)) {
            println!(
                "The full destination specifier is <{full_name}.{}:{}>",
                hex(self.identity_hash().as_bytes()),
                hex(destination.as_bytes())
            );
        }
        Ok(())
    }

    pub fn write_export(&self, args: &RnidArgs) -> Result<(), LocalIdentityError> {
        if args.crypto_operation().is_some() {
            return Ok(());
        }
        let Some(path) = &args.write else {
            return Ok(());
        };
        if args.export_private {
            let private = self.private()?;
            write_bytes(
                path,
                private.as_bytes(),
                args.force,
                OutputSensitivity::Private,
            )?;
            println!("Wrote private identity to {}", path.display());
        } else {
            let public = self.public()?;
            let path = public_path(path);
            write_bytes(
                &path,
                public.as_bytes(),
                args.force,
                OutputSensitivity::Ordinary,
            )?;
            println!("Wrote public identity to {}", path.display());
        }
        Ok(())
    }
}

fn import_bytes(
    value: &str,
    encoding: Option<IdentityEncoding>,
    expected: usize,
) -> Result<Vec<u8>, LocalIdentityError> {
    let path = expand_user_path(Path::new(value)).map_err(LocalIdentityError::Io)?;
    if path.is_file() {
        let bytes = read_file(&path).map_err(LocalIdentityError::Io)?;
        if bytes.len() != expected {
            return Err(LocalIdentityError::Material(IdentityMaterialLengthError {
                expected,
                found: bytes.len(),
            }));
        }
        return Ok(bytes);
    }
    decode_identity(value, encoding, expected).map_err(LocalIdentityError::Encoding)
}

fn write_bytes(
    path: &Path,
    bytes: &[u8],
    force: bool,
    sensitivity: OutputSensitivity,
) -> Result<(), LocalIdentityError> {
    use std::io::Write;

    let overwrite = if force {
        OverwritePolicy::Replace
    } else {
        OverwritePolicy::Refuse
    };
    let mut output =
        OutputSink::file(path, overwrite, sensitivity).map_err(LocalIdentityError::Io)?;
    output.write_all(bytes).map_err(|source| {
        LocalIdentityError::Io(IdentityIoError::Write {
            path: path.to_owned(),
            source,
        })
    })?;
    output.finish().map_err(LocalIdentityError::Io)
}

fn parse_hash(value: &str) -> Result<IdentityHash, LocalIdentityError> {
    let bytes = decode_identity(value, Some(IdentityEncoding::Hex), 16)
        .map_err(|_| LocalIdentityError::InvalidHash)?;
    let bytes: [u8; 16] = bytes
        .try_into()
        .map_err(|_| LocalIdentityError::InvalidHash)?;
    Ok(IdentityHash::new(bytes))
}

fn public_path(path: &Path) -> PathBuf {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pub"))
    {
        path.to_owned()
    } else {
        let mut output = path.as_os_str().to_owned();
        output.push(".pub");
        output.into()
    }
}

pub fn pretty_hash(hash: IdentityHash) -> String {
    pretty_destination(hash.as_bytes())
}

fn pretty_destination(bytes: &[u8]) -> String {
    format!("<{}>", hex(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn print_status(args: &RnidArgs, message: std::fmt::Arguments<'_>) {
    if args.stdout {
        eprintln!("{message}");
    } else {
        println!("{message}");
    }
}

impl std::fmt::Display for LocalIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(source) => source.fmt(formatter),
            Self::Encoding(source) => source.fmt(formatter),
            Self::Material(source) => source.fmt(formatter),
            Self::Entropy(source) => source.fmt(formatter),
            Self::InvalidHash => formatter.write_str("invalid hexadecimal identity hash"),
            Self::Missing => formatter.write_str("could not get working identity"),
            Self::PublicRequired => formatter.write_str("identity does not hold a public key"),
            Self::PrivateRequired => formatter.write_str("identity does not hold a private key"),
            Self::DestinationName(source) => {
                write!(formatter, "invalid destination aspects: {source:?}")
            }
        }
    }
}

impl std::error::Error for LocalIdentityError {}
