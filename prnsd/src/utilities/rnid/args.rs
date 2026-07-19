use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{ArgGroup, Args};

#[derive(Clone, Debug, PartialEq, Eq, Args)]
#[command(group(
    ArgGroup::new("identity_source")
        .args(["identity", "generate", "import_public", "import_private"])
        .multiple(false)
), group(
    ArgGroup::new("crypto_operation")
        .args(["encrypt", "decrypt", "validate", "sign", "sign_message"])
        .multiple(false)
), group(
    ArgGroup::new("encoding")
        .args(["base64", "base32", "base256", "hex"])
        .multiple(false)
))]
pub struct RnidArgs {
    #[arg(long, value_name = "DIR")]
    pub config: Option<PathBuf>,

    #[arg(short = 'i', long, value_name = "RID_OR_HASH")]
    pub identity: Option<String>,

    #[arg(short = 'g', long, value_name = "PATH")]
    pub generate: Option<PathBuf>,

    #[arg(short = 'm', long = "import-pub", value_name = "RID")]
    pub import_public: Option<String>,

    #[arg(short = 'M', long = "import-prv", value_name = "RID")]
    pub import_private: Option<String>,

    #[arg(short = 'x', long = "export-pub")]
    pub export_public: bool,

    #[arg(short = 'X', long = "export-prv")]
    pub export_private: bool,

    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[arg(short = 'q', long, action = clap::ArgAction::Count)]
    pub quiet: u8,

    #[arg(short = 'H', long = "hash", value_name = "ASPECTS")]
    pub hash: Option<String>,

    #[arg(short = 'a', long, value_name = "ASPECTS", num_args = 0..=1, default_missing_value = "rns.id")]
    pub announce: Option<String>,

    #[arg(short = 'd', long, value_name = "FILE", num_args = 0..)]
    pub decrypt: Option<Vec<PathBuf>>,

    #[arg(short = 'e', long, value_name = "FILE", num_args = 0..)]
    pub encrypt: Option<Vec<PathBuf>>,

    #[arg(short = 'V', long, value_name = "PATH", num_args = 1..)]
    pub validate: Option<Vec<PathBuf>>,

    #[arg(short = 's', long, value_name = "PATH", num_args = 0..)]
    pub sign: Option<Vec<PathBuf>>,

    #[arg(short = 'S', long = "sign-message", value_name = "TEXT", num_args = 0..=1, default_missing_value = "")]
    pub sign_message: Option<String>,

    #[arg(short = 'E', long = "embed-meta", value_name = "PATH")]
    pub embed_meta: Option<PathBuf>,

    #[arg(long = "meta-spec", value_name = "PATH")]
    pub meta_spec: Option<PathBuf>,

    #[arg(long)]
    pub raw: bool,

    #[arg(short = 'w', long, value_name = "PATH")]
    pub write: Option<PathBuf>,

    #[arg(short = 'r', long, value_name = "PATH")]
    pub read: Option<PathBuf>,

    #[arg(short = 'f', long)]
    pub force: bool,

    #[arg(short = 'I', long = "stdin", hide = true)]
    pub stdin: bool,

    #[arg(short = 'O', long = "stdout", hide = true)]
    pub stdout: bool,

    #[arg(short = 'p', long = "print-identity")]
    pub print_identity: bool,

    #[arg(short = 'P', long = "print-private")]
    pub print_private: bool,

    #[arg(short = 'R', long)]
    pub request: bool,

    #[arg(short = 'N', long = "no-cache")]
    pub no_cache: bool,

    #[arg(short = 't', value_name = "SECONDS", default_value = "15")]
    pub timeout: IdentityRequestTimeout,

    #[arg(short = 'B', long)]
    pub base32: bool,

    #[arg(short = 'b', long)]
    pub base64: bool,

    #[arg(short = 'U', long)]
    pub base256: bool,

    #[arg(short = 'F', long = "hex")]
    pub hex: bool,

    #[arg(long)]
    pub meta: bool,

    #[arg(long)]
    pub version: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityEncoding {
    Hex,
    Base32,
    Base64,
    Base256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource<'a> {
    None,
    Identity(&'a str),
    Generate(&'a Path),
    ImportPublic(&'a str),
    ImportPrivate(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoOperation<'a> {
    Encrypt(&'a [PathBuf]),
    Decrypt(&'a [PathBuf]),
    Sign(&'a [PathBuf]),
    Validate(&'a [PathBuf]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnidArgumentError {
    RawWithoutSign,
    MissingInput,
    StdinWithPaths,
    StdinValidationUnsupported,
    StdoutWithoutCryptoOperation,
    StdoutValidationUnsupported,
    StdoutWithMultipleInputs,
    StdinWithoutOutput,
    WriteWithMultipleInputs,
    WriteOutputConflict,
    BinaryStdoutWithTextOutput,
    EncodingWithoutTextOutput,
    PrivatePrintWithoutIdentityPrint,
    MetadataWithoutMessage,
    MetadataSpecWithoutMetadata,
    ReadWithoutMessage,
    ConflictingMessageInput,
    MissingMessage,
    MessageOutputRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityRequestTimeout(Duration);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityRequestTimeoutError;

impl IdentityRequestTimeout {
    pub const fn duration(self) -> Duration {
        self.0
    }
}

impl std::str::FromStr for IdentityRequestTimeout {
    type Err = IdentityRequestTimeoutError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let seconds = value
            .parse::<f64>()
            .map_err(|_| IdentityRequestTimeoutError)?;
        if !seconds.is_finite() || seconds <= 0.0 {
            return Err(IdentityRequestTimeoutError);
        }
        Ok(Self(Duration::from_secs_f64(seconds)))
    }
}

impl std::fmt::Display for IdentityRequestTimeoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("identity request timeout must be a positive finite number")
    }
}

impl std::error::Error for IdentityRequestTimeoutError {}

impl RnidArgs {
    pub const fn encoding(&self) -> IdentityEncoding {
        if self.base32 {
            IdentityEncoding::Base32
        } else if self.base64 {
            IdentityEncoding::Base64
        } else if self.base256 {
            IdentityEncoding::Base256
        } else {
            IdentityEncoding::Hex
        }
    }

    pub const fn explicit_encoding(&self) -> Option<IdentityEncoding> {
        if self.base32 || self.base64 || self.base256 || self.hex {
            Some(self.encoding())
        } else {
            None
        }
    }

    pub fn source(&self) -> IdentitySource<'_> {
        if let Some(identity) = &self.identity {
            IdentitySource::Identity(identity)
        } else if let Some(path) = &self.generate {
            IdentitySource::Generate(path)
        } else if let Some(identity) = &self.import_public {
            IdentitySource::ImportPublic(identity)
        } else if let Some(identity) = &self.import_private {
            IdentitySource::ImportPrivate(identity)
        } else {
            IdentitySource::None
        }
    }

    pub fn crypto_operation(&self) -> Option<CryptoOperation<'_>> {
        if let Some(paths) = &self.encrypt {
            Some(CryptoOperation::Encrypt(paths))
        } else if let Some(paths) = &self.decrypt {
            Some(CryptoOperation::Decrypt(paths))
        } else if let Some(paths) = &self.sign {
            Some(CryptoOperation::Sign(paths))
        } else {
            self.validate.as_deref().map(CryptoOperation::Validate)
        }
    }

    pub fn validate_local(&self) -> Result<(), RnidArgumentError> {
        let operation = self.crypto_operation();
        if self.raw && !matches!(operation, Some(CryptoOperation::Sign(_))) {
            return Err(RnidArgumentError::RawWithoutSign);
        }
        if self.stdin && self.sign_message.is_none() {
            match operation {
                Some(CryptoOperation::Validate(_)) => {
                    return Err(RnidArgumentError::StdinValidationUnsupported);
                }
                Some(
                    CryptoOperation::Encrypt(paths)
                    | CryptoOperation::Decrypt(paths)
                    | CryptoOperation::Sign(paths),
                ) if !paths.is_empty() => {
                    return Err(RnidArgumentError::StdinWithPaths);
                }
                Some(_) => {}
                None => return Err(RnidArgumentError::MissingInput),
            }
        } else if operation.is_some_and(|operation| operation.paths().is_empty()) {
            return Err(RnidArgumentError::MissingInput);
        }
        if self.stdout && operation.is_none() && self.sign_message.is_none() {
            return Err(RnidArgumentError::StdoutWithoutCryptoOperation);
        }
        if self.stdout && matches!(operation, Some(CryptoOperation::Validate(_))) {
            return Err(RnidArgumentError::StdoutValidationUnsupported);
        }
        if self.stdout && operation.is_some_and(|operation| operation.paths().len() > 1) {
            return Err(RnidArgumentError::StdoutWithMultipleInputs);
        }
        if self.stdin && self.write.is_none() && !self.stdout && self.sign_message.is_none() {
            return Err(RnidArgumentError::StdinWithoutOutput);
        }
        if self.write.is_some() && operation.is_some_and(|operation| operation.paths().len() > 1) {
            return Err(RnidArgumentError::WriteWithMultipleInputs);
        }
        let identity_write = self.export_public || self.export_private;
        if self.write.is_some() && operation.is_some() && identity_write {
            return Err(RnidArgumentError::WriteOutputConflict);
        }
        let text_output = self.print_identity
            || self.export_public
            || self.export_private
            || self.hash.is_some()
            || self.announce.is_some();
        if self.stdout && text_output {
            return Err(RnidArgumentError::BinaryStdoutWithTextOutput);
        }
        let encoded_output = self.print_identity
            || self.export_public
            || self.export_private
            || matches!(operation, Some(CryptoOperation::Sign(_)))
            || self.sign_message.is_some();
        if self.explicit_encoding().is_some() && !encoded_output {
            return Err(RnidArgumentError::EncodingWithoutTextOutput);
        }
        if self.print_private && !self.print_identity {
            return Err(RnidArgumentError::PrivatePrintWithoutIdentityPrint);
        }
        if self.embed_meta.is_some() && self.sign_message.is_none() {
            return Err(RnidArgumentError::MetadataWithoutMessage);
        }
        if self.meta_spec.is_some() && self.embed_meta.is_none() {
            return Err(RnidArgumentError::MetadataSpecWithoutMetadata);
        }
        if self.read.is_some() && self.sign_message.is_none() {
            return Err(RnidArgumentError::ReadWithoutMessage);
        }
        if self.sign_message.is_some() {
            let direct = self
                .sign_message
                .as_ref()
                .is_some_and(|message| !message.is_empty());
            let inputs =
                usize::from(direct) + usize::from(self.read.is_some()) + usize::from(self.stdin);
            if inputs > 1 {
                return Err(RnidArgumentError::ConflictingMessageInput);
            }
            if inputs == 0 {
                return Err(RnidArgumentError::MissingMessage);
            }
            if self.explicit_encoding().is_none() && self.write.is_none() && !self.stdout {
                return Err(RnidArgumentError::MessageOutputRequired);
            }
        }
        Ok(())
    }
}

impl<'a> CryptoOperation<'a> {
    pub fn paths(self) -> &'a [PathBuf] {
        match self {
            Self::Encrypt(paths)
            | Self::Decrypt(paths)
            | Self::Sign(paths)
            | Self::Validate(paths) => paths,
        }
    }
}

impl std::fmt::Display for RnidArgumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::RawWithoutSign => "--raw is only valid with --sign",
            Self::MissingInput => "the selected operation requires a file or --stdin",
            Self::StdinWithPaths => "--stdin cannot be combined with file inputs",
            Self::StdinValidationUnsupported => {
                "raw validation requires a file so its adjacent .rsg signature can be located"
            }
            Self::StdoutWithoutCryptoOperation => "--stdout requires encrypt, decrypt, or raw sign",
            Self::StdoutValidationUnsupported => {
                "--stdout is not an output mode for signature validation"
            }
            Self::StdoutWithMultipleInputs => "--stdout requires exactly one input",
            Self::StdinWithoutOutput => "--stdin requires --stdout or --write",
            Self::WriteWithMultipleInputs => "--write requires exactly one input file",
            Self::WriteOutputConflict => {
                "--write cannot select both an identity export and crypto output"
            }
            Self::BinaryStdoutWithTextOutput => {
                "--stdout cannot be combined with identity or hash text output"
            }
            Self::EncodingWithoutTextOutput => {
                "identity encodings require an identity print or export operation"
            }
            Self::PrivatePrintWithoutIdentityPrint => "--print-private requires --print-identity",
            Self::MetadataWithoutMessage => "--embed-meta requires --sign-message",
            Self::MetadataSpecWithoutMetadata => "--meta-spec requires --embed-meta",
            Self::ReadWithoutMessage => "--read requires --sign-message",
            Self::ConflictingMessageInput => {
                "provide signed-message text, --read, or --stdin, not more than one"
            }
            Self::MissingMessage => "--sign-message requires text, --read, or --stdin",
            Self::MessageOutputRequired => {
                "binary signed-message output requires --write or --stdout"
            }
        })
    }
}

impl std::error::Error for RnidArgumentError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_args() -> RnidArgs {
        RnidArgs {
            config: None,
            identity: None,
            generate: None,
            import_public: None,
            import_private: None,
            export_public: false,
            export_private: false,
            verbose: 0,
            quiet: 0,
            hash: None,
            announce: None,
            decrypt: None,
            encrypt: None,
            validate: None,
            sign: None,
            sign_message: None,
            embed_meta: None,
            meta_spec: None,
            raw: false,
            write: None,
            read: None,
            force: false,
            stdin: false,
            stdout: false,
            print_identity: false,
            print_private: false,
            request: false,
            no_cache: false,
            timeout: IdentityRequestTimeout(Duration::from_secs(15)),
            base32: false,
            base64: false,
            base256: false,
            hex: false,
            meta: false,
            version: false,
        }
    }

    #[test]
    fn raw_signing_accepts_one_file_or_stdin() {
        let mut args = empty_args();
        args.sign = Some(vec![PathBuf::from("message")]);
        args.raw = true;
        assert_eq!(args.validate_local(), Ok(()));

        args.sign = Some(Vec::new());
        args.stdin = true;
        args.stdout = true;
        assert_eq!(args.validate_local(), Ok(()));
    }

    #[test]
    fn artifact_and_ambiguous_output_shapes_are_checked() {
        let mut args = empty_args();
        args.sign = Some(vec![PathBuf::from("message")]);
        assert_eq!(args.validate_local(), Ok(()));

        args.raw = true;
        args.stdout = true;
        args.print_identity = true;
        assert_eq!(
            args.validate_local(),
            Err(RnidArgumentError::BinaryStdoutWithTextOutput)
        );
    }
}
