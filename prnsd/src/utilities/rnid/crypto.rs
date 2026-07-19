use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use personal_rns::crypto::{sealed_len, Ed25519Signature, X25519SecretKey, TOKEN_OVERHEAD};
use personal_rns::identity::{
    DecryptError, EncryptError, ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN, ENCRYPTION_IV_LEN,
};
use personal_rns::runtime::{fill_os_entropy, OsEntropyError};

use super::args::{CryptoOperation, RnidArgs};
use super::identity::{LocalIdentity, LocalIdentityError};
use super::io::{
    expand_user_path, read_file, read_stdin, IdentityIoError, OutputSensitivity, OutputSink,
    OverwritePolicy,
};

const ENCRYPTION_CHUNK_LEN: usize = 1024 * 1024 * 16;
const ENCRYPTED_CHUNK_LEN: usize =
    ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN + sealed_len(ENCRYPTION_CHUNK_LEN);
const DECRYPTED_CHUNK_BUFFER_LEN: usize = sealed_len(ENCRYPTION_CHUNK_LEN) - TOKEN_OVERHEAD;

#[derive(Debug)]
pub enum LocalCryptoError {
    Identity(LocalIdentityError),
    Io(IdentityIoError),
    Entropy(OsEntropyError),
    Encrypt(EncryptError),
    Decrypt(DecryptError),
    InvalidEncryptedFile(PathBuf),
    InvalidSignatureFile { path: PathBuf, length: usize },
    InvalidSignature { target: PathBuf, signature: PathBuf },
}

pub fn execute(
    args: &RnidArgs,
    identity: &LocalIdentity,
    operation: CryptoOperation<'_>,
) -> Result<(), LocalCryptoError> {
    match operation {
        CryptoOperation::Encrypt(paths) => encrypt(args, identity, paths),
        CryptoOperation::Decrypt(paths) => decrypt(args, identity, paths),
        CryptoOperation::Sign(paths) => sign(args, identity, paths),
        CryptoOperation::Validate(paths) => validate(identity, paths),
    }
}

fn sign(
    args: &RnidArgs,
    identity: &LocalIdentity,
    paths: &[PathBuf],
) -> Result<(), LocalCryptoError> {
    let identity = identity.private().map_err(LocalCryptoError::Identity)?;
    if args.stdin {
        let message = read_stdin().map_err(LocalCryptoError::Io)?;
        let signature = identity.sign(&message);
        let (mut output, output_path) = open_output(args, None)?;
        write_output(&mut output, &output_path, &signature.0)?;
        output.finish().map_err(LocalCryptoError::Io)?;
        return Ok(());
    }
    for path in paths {
        let message = read_file(path).map_err(LocalCryptoError::Io)?;
        let signature = identity.sign(&message);
        let default = append_suffix(path, ".rsg");
        let (mut output, output_path) = open_output(args, Some(&default))?;
        write_output(&mut output, &output_path, &signature.0)?;
        output.finish().map_err(LocalCryptoError::Io)?;
        print_completion(
            args,
            format_args!(
                "File {} signed with {} to {}",
                path.display(),
                super::identity::pretty_hash(identity.identity_hash()),
                output_path.display()
            ),
        );
    }
    Ok(())
}

fn validate(identity: &LocalIdentity, paths: &[PathBuf]) -> Result<(), LocalCryptoError> {
    let identity = identity.public().map_err(LocalCryptoError::Identity)?;
    for path in paths {
        let (target_path, signature_path) = signature_pair(path)?;
        let message = read_file(&target_path).map_err(LocalCryptoError::Io)?;
        let signature = read_file(&signature_path).map_err(LocalCryptoError::Io)?;
        let signature_bytes: [u8; Ed25519Signature::LEN] = signature
            .as_slice()
            .try_into()
            .map_err(|_| LocalCryptoError::InvalidSignatureFile {
                path: signature_path.clone(),
                length: signature.len(),
            })?;
        identity
            .verify(&message, &Ed25519Signature(signature_bytes))
            .map_err(|_| LocalCryptoError::InvalidSignature {
                target: target_path.clone(),
                signature: signature_path.clone(),
            })?;
        println!(
            "Signature is valid, the file {} was signed by {}",
            target_path.display(),
            super::identity::pretty_hash(identity.identity_hash())
        );
    }
    Ok(())
}

fn encrypt(
    args: &RnidArgs,
    identity: &LocalIdentity,
    paths: &[PathBuf],
) -> Result<(), LocalCryptoError> {
    let identity = identity.public().map_err(LocalCryptoError::Identity)?;
    if args.stdin {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let (mut output, output_path) = open_output(args, None)?;
        encrypt_reader(
            &mut input,
            Path::new("<stdin>"),
            &mut output,
            &output_path,
            &identity,
        )?;
        output.finish().map_err(LocalCryptoError::Io)?;
        return Ok(());
    }
    for path in paths {
        let input_path = expand_user_path(path).map_err(LocalCryptoError::Io)?;
        let mut input = File::open(&input_path).map_err(|source| {
            LocalCryptoError::Io(IdentityIoError::Read {
                path: input_path.clone(),
                source,
            })
        })?;
        let default = append_suffix(path, ".rfe");
        let (mut output, output_path) = open_output(args, Some(&default))?;
        encrypt_reader(
            &mut input,
            &input_path,
            &mut output,
            &output_path,
            &identity,
        )?;
        output.finish().map_err(LocalCryptoError::Io)?;
        print_completion(
            args,
            format_args!(
                "File {} encrypted for {} to {}",
                input_path.display(),
                super::identity::pretty_hash(identity.identity_hash()),
                output_path.display()
            ),
        );
    }
    Ok(())
}

fn decrypt(
    args: &RnidArgs,
    identity: &LocalIdentity,
    paths: &[PathBuf],
) -> Result<(), LocalCryptoError> {
    let identity = identity.private().map_err(LocalCryptoError::Identity)?;
    if args.stdin {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let (mut output, output_path) = open_output(args, None)?;
        decrypt_reader(
            &mut input,
            Path::new("<stdin>"),
            &mut output,
            &output_path,
            identity,
        )?;
        output.finish().map_err(LocalCryptoError::Io)?;
        return Ok(());
    }
    for path in paths {
        let default = decrypted_path(path)?;
        let input_path = expand_user_path(path).map_err(LocalCryptoError::Io)?;
        let mut input = File::open(&input_path).map_err(|source| {
            LocalCryptoError::Io(IdentityIoError::Read {
                path: input_path.clone(),
                source,
            })
        })?;
        let (mut output, output_path) = open_output(args, Some(&default))?;
        decrypt_reader(&mut input, &input_path, &mut output, &output_path, identity)?;
        output.finish().map_err(LocalCryptoError::Io)?;
        print_completion(
            args,
            format_args!(
                "File {} decrypted with {} to {}",
                input_path.display(),
                super::identity::pretty_hash(identity.identity_hash()),
                output_path.display()
            ),
        );
    }
    Ok(())
}

fn encrypt_reader(
    input: &mut impl Read,
    input_path: &Path,
    output: &mut OutputSink,
    output_path: &Path,
    identity: &personal_rns::identity::PublicIdentityMaterial,
) -> Result<(), LocalCryptoError> {
    let mut plaintext = vec![0u8; ENCRYPTION_CHUNK_LEN];
    let mut encrypted = vec![0u8; ENCRYPTED_CHUNK_LEN];
    loop {
        let read = read_chunk(input, &mut plaintext).map_err(|source| {
            LocalCryptoError::Io(IdentityIoError::Read {
                path: input_path.to_owned(),
                source,
            })
        })?;
        if read == 0 {
            break;
        }
        let mut entropy = [0u8; X25519SecretKey::LEN + ENCRYPTION_IV_LEN];
        fill_os_entropy(&mut entropy).map_err(LocalCryptoError::Entropy)?;
        let mut secret = [0u8; X25519SecretKey::LEN];
        secret.copy_from_slice(&entropy[..X25519SecretKey::LEN]);
        let mut iv = [0u8; ENCRYPTION_IV_LEN];
        iv.copy_from_slice(&entropy[X25519SecretKey::LEN..]);
        let encrypted_len = identity
            .encrypt(
                &X25519SecretKey::new(secret),
                &iv,
                &plaintext[..read],
                &mut encrypted,
            )
            .map_err(LocalCryptoError::Encrypt)?;
        write_output(output, output_path, &encrypted[..encrypted_len])?;
    }
    Ok(())
}

fn decrypt_reader(
    input: &mut impl Read,
    input_path: &Path,
    output: &mut OutputSink,
    output_path: &Path,
    identity: &personal_rns::identity::PrivateIdentityMaterial,
) -> Result<(), LocalCryptoError> {
    let mut encrypted = vec![0u8; ENCRYPTED_CHUNK_LEN];
    let mut plaintext = vec![0u8; DECRYPTED_CHUNK_BUFFER_LEN];
    loop {
        let read = read_chunk(input, &mut encrypted).map_err(|source| {
            LocalCryptoError::Io(IdentityIoError::Read {
                path: input_path.to_owned(),
                source,
            })
        })?;
        if read == 0 {
            break;
        }
        let plaintext_len = identity
            .decrypt(&encrypted[..read], &mut plaintext)
            .map_err(LocalCryptoError::Decrypt)?;
        write_output(output, output_path, &plaintext[..plaintext_len])?;
    }
    Ok(())
}

fn read_chunk(input: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match input.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

fn open_output(
    args: &RnidArgs,
    default: Option<&Path>,
) -> Result<(OutputSink, PathBuf), LocalCryptoError> {
    if args.stdout {
        return Ok((OutputSink::stdout(), PathBuf::from("<stdout>")));
    }
    let path =
        args.write.as_deref().or(default).ok_or_else(|| {
            LocalCryptoError::Io(IdentityIoError::InvalidOutputPath(PathBuf::new()))
        })?;
    let path = expand_user_path(path).map_err(LocalCryptoError::Io)?;
    let overwrite = if args.force {
        OverwritePolicy::Replace
    } else {
        OverwritePolicy::Refuse
    };
    let output = OutputSink::file(&path, overwrite, OutputSensitivity::Ordinary)
        .map_err(LocalCryptoError::Io)?;
    Ok((output, path))
}

fn write_output(
    output: &mut OutputSink,
    output_path: &Path,
    bytes: &[u8],
) -> Result<(), LocalCryptoError> {
    output.write_all(bytes).map_err(|source| {
        LocalCryptoError::Io(IdentityIoError::Write {
            path: output_path.to_owned(),
            source,
        })
    })
}

fn signature_pair(path: &Path) -> Result<(PathBuf, PathBuf), LocalCryptoError> {
    let signature = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rsg"));
    if signature {
        let target = path.with_extension("");
        if target.as_os_str().is_empty() {
            return Err(LocalCryptoError::InvalidSignatureFile {
                path: path.to_owned(),
                length: 0,
            });
        }
        Ok((target, path.to_owned()))
    } else {
        Ok((path.to_owned(), append_suffix(path, ".rsg")))
    }
}

fn decrypted_path(path: &Path) -> Result<PathBuf, LocalCryptoError> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("rfe") {
        return Err(LocalCryptoError::InvalidEncryptedFile(path.to_owned()));
    }
    let output = path.with_extension("");
    if output.as_os_str().is_empty() {
        Err(LocalCryptoError::InvalidEncryptedFile(path.to_owned()))
    } else {
        Ok(output)
    }
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut output = path.as_os_str().to_owned();
    output.push(suffix);
    output.into()
}

fn print_completion(args: &RnidArgs, message: std::fmt::Arguments<'_>) {
    if args.stdout {
        eprintln!("{message}");
    } else {
        println!("{message}");
    }
}

impl std::fmt::Display for LocalCryptoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(source) => source.fmt(formatter),
            Self::Io(source) => source.fmt(formatter),
            Self::Entropy(source) => source.fmt(formatter),
            Self::Encrypt(source) => write!(formatter, "identity encryption failed: {source:?}"),
            Self::Decrypt(source) => write!(formatter, "identity decryption failed: {source:?}"),
            Self::InvalidEncryptedFile(path) => write!(
                formatter,
                "{} does not appear to be a Reticulum encrypted file",
                path.display()
            ),
            Self::InvalidSignatureFile { path, length } => write!(
                formatter,
                "{} holds {length} bytes, not a 64-byte raw RNS signature",
                path.display()
            ),
            Self::InvalidSignature { target, signature } => write!(
                formatter,
                "invalid signature {} for file {}",
                signature.display(),
                target.display()
            ),
        }
    }
}

impl std::error::Error for LocalCryptoError {}
