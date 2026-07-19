use std::path::PathBuf;

use personal_rns::identity::{
    create_signed_artifact, validate_signed_artifact, SignedArtifactError,
};
use personal_rns::message_pack::MessagePackValue;
use prns_config::configobj::{self, ConfigError, Section, Value};

use super::args::{IdentityEncoding, RnidArgs};
use super::encoding::encode;
use super::identity::LocalIdentity;
use super::io::{read_file, read_stdin, IdentityIoError};

#[derive(Debug)]
pub enum ArtifactError {
    Identity(super::identity::LocalIdentityError),
    Io(IdentityIoError),
    Artifact(SignedArtifactError),
    MetadataConfig(ConfigError),
    MetadataSpecConfig(ConfigError),
    MetadataSpec(String),
    MessageEncoding(PathBuf),
}

pub fn create_message_signature(
    args: &RnidArgs,
    identity: &LocalIdentity,
) -> Result<Vec<u8>, ArtifactError> {
    let private = identity.private().map_err(ArtifactError::Identity)?;
    let message = message_input(args)?;
    let metadata = metadata(args)?;
    create_signed_artifact(private, &message, true, &metadata).map_err(ArtifactError::Artifact)
}

pub fn validate(
    artifact: &[u8],
    message: Option<&[u8]>,
    identity: Option<&LocalIdentity>,
) -> Result<personal_rns::identity::ValidatedSignedArtifact, ArtifactError> {
    validate_signed_artifact(
        artifact,
        message,
        identity.map(LocalIdentity::identity_hash),
    )
    .map_err(ArtifactError::Artifact)
}

pub fn encoded_artifact(bytes: &[u8], encoding: IdentityEncoding) -> String {
    let encoded = encode(bytes, encoding);
    let header_prefix = "#### Start of rsg data ";
    let footer_suffix = " End of rsg data ####";
    let mut wrapped = String::new();
    wrapped.push_str(header_prefix);
    wrapped.extend(std::iter::repeat_n('#', 64 - header_prefix.len()));
    wrapped.push('\n');
    let mut characters = encoded.chars();
    loop {
        let chunk: String = characters.by_ref().take(64).collect();
        if chunk.is_empty() {
            break;
        }
        let chunk_length = chunk.chars().count();
        wrapped.push_str(&chunk);
        wrapped.extend(std::iter::repeat_n('=', 64 - chunk_length));
        wrapped.push('\n');
    }
    wrapped.extend(std::iter::repeat_n('#', 64 - footer_suffix.len()));
    wrapped.push_str(footer_suffix);
    wrapped
}

pub fn print_metadata(metadata: &[(String, MessagePackValue)]) {
    for (key, value) in metadata {
        print_value(key, value, 1);
    }
}

fn message_input(args: &RnidArgs) -> Result<Vec<u8>, ArtifactError> {
    if let Some(path) = &args.read {
        let bytes = read_file(path).map_err(ArtifactError::Io)?;
        std::str::from_utf8(&bytes).map_err(|_| ArtifactError::MessageEncoding(path.clone()))?;
        return Ok(bytes);
    }
    if args.stdin {
        return read_stdin().map_err(ArtifactError::Io);
    }
    Ok(args
        .sign_message
        .as_deref()
        .unwrap_or_default()
        .as_bytes()
        .to_vec())
}

fn metadata(args: &RnidArgs) -> Result<Vec<(String, MessagePackValue)>, ArtifactError> {
    let Some(path) = &args.embed_meta else {
        return Ok(Vec::new());
    };
    let bytes = read_file(path).map_err(ArtifactError::Io)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| ArtifactError::MessageEncoding(path.clone()))?;
    let parsed = configobj::parse(text).map_err(ArtifactError::MetadataConfig)?;
    match &args.meta_spec {
        Some(spec_path) => {
            let spec_bytes = read_file(spec_path).map_err(ArtifactError::Io)?;
            let spec_text = std::str::from_utf8(&spec_bytes)
                .map_err(|_| ArtifactError::MessageEncoding(spec_path.clone()))?;
            let spec = configobj::parse(spec_text).map_err(ArtifactError::MetadataSpecConfig)?;
            section_with_spec(&parsed, &spec)
        }
        None => Ok(section_entries(&parsed)),
    }
}

fn section_entries(section: &Section) -> Vec<(String, MessagePackValue)> {
    let mut entries = Vec::with_capacity(section.scalars.len() + section.sections.len());
    for (key, value) in &section.scalars {
        entries.push((key.clone(), config_value(value)));
    }
    for (key, section) in &section.sections {
        entries.push((
            key.clone(),
            MessagePackValue::Map(
                section_entries(section)
                    .into_iter()
                    .map(|(key, value)| (MessagePackValue::String(key), value))
                    .collect(),
            ),
        ));
    }
    entries
}

fn section_with_spec(
    section: &Section,
    spec: &Section,
) -> Result<Vec<(String, MessagePackValue)>, ArtifactError> {
    let mut entries = Vec::with_capacity(section.scalars.len() + section.sections.len());
    for (key, value) in &section.scalars {
        let rule = spec.get(key).and_then(Value::as_scalar).ok_or_else(|| {
            ArtifactError::MetadataSpec(format!("metadata key {key:?} has no scalar specification"))
        })?;
        entries.push((key.clone(), coerce(value, rule, key)?));
    }
    for (key, child) in &section.sections {
        let child_spec = spec.section(key).ok_or_else(|| {
            ArtifactError::MetadataSpec(format!("metadata section {key:?} has no specification"))
        })?;
        entries.push((
            key.clone(),
            MessagePackValue::Map(
                section_with_spec(child, child_spec)?
                    .into_iter()
                    .map(|(key, value)| (MessagePackValue::String(key), value))
                    .collect(),
            ),
        ));
    }
    for (key, _) in &spec.scalars {
        if section.get(key).is_none() {
            return Err(ArtifactError::MetadataSpec(format!(
                "required metadata key {key:?} is missing"
            )));
        }
    }
    Ok(entries)
}

fn config_value(value: &Value) -> MessagePackValue {
    match value {
        Value::Scalar(value) => MessagePackValue::String(value.clone()),
        Value::List(values) => MessagePackValue::Array(
            values
                .iter()
                .cloned()
                .map(MessagePackValue::String)
                .collect(),
        ),
    }
}

fn coerce(value: &Value, rule: &str, key: &str) -> Result<MessagePackValue, ArtifactError> {
    let (kind, parameters) = rule
        .split_once('(')
        .map_or((rule, ""), |(kind, parameters)| {
            (kind, parameters.strip_suffix(')').unwrap_or(parameters))
        });
    let kind = kind.trim();
    if !parameters.trim().is_empty() {
        return Err(ArtifactError::MetadataSpec(format!(
            "metadata key {key:?} uses unsupported validator parameters"
        )));
    }
    match kind {
        "string" => scalar(value, key).map(|value| MessagePackValue::String(value.to_string())),
        "integer" => scalar(value, key)?
            .parse::<i64>()
            .map(MessagePackValue::Signed)
            .map_err(|_| invalid_value(key, kind)),
        "float" => scalar(value, key)?
            .parse::<f64>()
            .map(MessagePackValue::Float)
            .map_err(|_| invalid_value(key, kind)),
        "boolean" => boolean(scalar(value, key)?)
            .map(MessagePackValue::Boolean)
            .ok_or_else(|| invalid_value(key, kind)),
        "list" | "force_list" | "string_list" => Ok(MessagePackValue::Array(
            value
                .as_list()
                .into_iter()
                .map(|value| MessagePackValue::String(value.to_string()))
                .collect(),
        )),
        "int_list" => value
            .as_list()
            .into_iter()
            .map(|value| {
                value
                    .parse::<i64>()
                    .map(MessagePackValue::Signed)
                    .map_err(|_| invalid_value(key, kind))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(MessagePackValue::Array),
        "float_list" => value
            .as_list()
            .into_iter()
            .map(|value| {
                value
                    .parse::<f64>()
                    .map(MessagePackValue::Float)
                    .map_err(|_| invalid_value(key, kind))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(MessagePackValue::Array),
        "bool_list" => value
            .as_list()
            .into_iter()
            .map(|value| {
                boolean(value)
                    .map(MessagePackValue::Boolean)
                    .ok_or_else(|| invalid_value(key, kind))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(MessagePackValue::Array),
        _ => Err(ArtifactError::MetadataSpec(format!(
            "metadata key {key:?} uses unsupported validator {kind:?}"
        ))),
    }
}

fn scalar<'a>(value: &'a Value, key: &str) -> Result<&'a str, ArtifactError> {
    value.as_scalar().ok_or_else(|| {
        ArtifactError::MetadataSpec(format!("metadata key {key:?} requires a scalar value"))
    })
}

fn boolean(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn invalid_value(key: &str, kind: &str) -> ArtifactError {
    ArtifactError::MetadataSpec(format!("metadata key {key:?} is not a valid {kind}"))
}

fn print_value(key: &str, value: &MessagePackValue, level: usize) {
    let indentation = "  ".repeat(level);
    match value {
        MessagePackValue::Map(entries) => {
            println!("d{indentation}{key}:");
            for (key, value) in entries {
                let key = match key {
                    MessagePackValue::String(key) => key.as_str(),
                    _ => "<invalid key>",
                };
                print_value(key, value, level + 1);
            }
        }
        MessagePackValue::Array(values) => println!("l{indentation}{key}={values:?}"),
        MessagePackValue::Binary(value) => println!(
            "b{indentation}{key}={}",
            encode(value, IdentityEncoding::Hex)
        ),
        MessagePackValue::String(value) => println!("s{indentation}{key}={value}"),
        MessagePackValue::Signed(value) => println!("i{indentation}{key}={value}"),
        MessagePackValue::Unsigned(value) => println!("i{indentation}{key}={value}"),
        MessagePackValue::Float(value) => println!("f{indentation}{key}={value}"),
        MessagePackValue::Boolean(value) => println!("u{indentation}{key}={value}"),
        MessagePackValue::Nil => println!("N{indentation}{key}=None"),
    }
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(source) => source.fmt(formatter),
            Self::Io(source) => source.fmt(formatter),
            Self::Artifact(source) => write!(formatter, "invalid RNS signed artifact: {source:?}"),
            Self::MetadataConfig(source) => write!(formatter, "invalid metadata: {source}"),
            Self::MetadataSpecConfig(source) => {
                write!(formatter, "invalid metadata specification: {source}")
            }
            Self::MetadataSpec(message) => formatter.write_str(message),
            Self::MessageEncoding(path) => {
                write!(formatter, "{} is not valid UTF-8 text", path.display())
            }
        }
    }
}

impl std::error::Error for ArtifactError {}
