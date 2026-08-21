use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

mod storage;

use prns_flash_manifest::{
    minisign_public_key_id, pinned_key_is_configured, sha256_hex, verify_minisign, BoardCatalog,
    ReleaseChannel, ValidatedChannelDescriptor, ValidatedFlashManifest, PINNED_MINISIGN_PUBLIC_KEY,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::error::AppError;
use crate::events::{Phase, Reporter};

const MAX_PUBLIC_KEY_BYTES: u64 = 16 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 64 * 1024;
const MAX_MANIFEST_BYTES: u64 = 512 * 1024;
const MAX_CHANNEL_BYTES: u64 = 64 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
// Release payloads are currently far smaller than 512 MiB. This leaves headroom for
// platform archives while rejecting unexpectedly large untrusted files before hashing.
const MAX_CANDIDATE_FILE_BYTES: u64 = 512 * 1024 * 1024;
// Keep this aligned with the release.candidate.extract task's extraction ceiling.
const MAX_CANDIDATE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_CANDIDATE_ENTRIES: usize = 200_000;
const MAX_CANDIDATE_DEPTH: usize = 64;

#[derive(Clone, Copy)]
struct CandidateLimits {
    file_bytes: u64,
    total_bytes: u64,
}

const CANDIDATE_LIMITS: CandidateLimits = CandidateLimits {
    file_bytes: MAX_CANDIDATE_FILE_BYTES,
    total_bytes: MAX_CANDIDATE_BYTES,
};

#[derive(Debug)]
pub(crate) struct ImportedCandidate {
    pub(crate) version: String,
    pub(crate) channel: &'static str,
    pub(crate) artifact_count: usize,
    pub(crate) artifact_bytes: u64,
}

#[derive(Debug, Error)]
enum CandidateError {
    #[error("{path} is not an extracted signed candidate directory")]
    InputNotDirectory { path: PathBuf },
    #[error("candidate contains a symbolic link or unsupported entry: {path}")]
    UnsafeEntry { path: PathBuf },
    #[error("candidate directory exceeds the safe traversal limit at {path}")]
    CandidateTreeLimit { path: PathBuf },
    #[error("candidate path is not a safe relative UTF-8 path: {path:?}")]
    UnsafePath { path: String },
    #[error("could not {action} {path}: {source}")]
    Filesystem {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("candidate file {path} exceeds the {limit}-byte safety limit")]
    FileTooLarge { path: PathBuf, limit: u64 },
    #[error("candidate files exceed the {limit}-byte aggregate safety limit")]
    CandidateTooLarge { limit: u64 },
    #[error("candidate Minisign public key differs from the CLI's pinned release key")]
    PublicKeyMismatch,
    #[error("pinned Minisign public key has no canonical key ID")]
    InvalidPinnedKeyId,
    #[error("signature verification failed for {path}: {message}")]
    Signature { path: PathBuf, message: String },
    #[error("signed manifest is invalid: {0}")]
    Manifest(String),
    #[error("candidate must contain exactly one signed stable or preview channel descriptor")]
    ChannelSet,
    #[error("signed channel descriptor is invalid: {0}")]
    Channel(String),
    #[error("candidate channel, manifest, VERSION, and key identity disagree")]
    ReleaseIdentity,
    #[error("candidate artifact {path} does not match manifest size/hash")]
    ArtifactMismatch { path: PathBuf },
    #[error("candidate artifacts collide at cache path {path}")]
    CachePathCollision { path: PathBuf },
    #[error("candidate file {path} is not valid UTF-8: {source}")]
    Utf8 {
        path: PathBuf,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("invalid SHA256SUMS.txt line {line}: {message}")]
    ChecksumLine { line: usize, message: String },
    #[error("duplicate SHA256SUMS.txt path {0:?}")]
    DuplicateChecksumPath(String),
    #[error("SHA-256 mismatch for candidate file {0}")]
    ChecksumMismatch(String),
    #[error("SHA256SUMS.txt coverage differs; unlisted={unlisted:?}, missing={missing:?}")]
    ChecksumCoverage {
        unlisted: Vec<String>,
        missing: Vec<String>,
    },
    #[error("immutable cache path {path} already contains different bytes")]
    ImmutableConflict { path: PathBuf },
    #[error("cache import was cancelled")]
    Cancelled,
}

trait SignatureVerifier {
    fn verify(&self, bytes: &[u8], signature: &str, public_key: &str) -> Result<(), String>;
}

struct MinisignVerifier;

impl SignatureVerifier for MinisignVerifier {
    fn verify(&self, bytes: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
        verify_minisign(bytes, signature, public_key).map_err(|error| error.to_string())
    }
}

struct VerifiedArtifact {
    board_slug: String,
    file_name: String,
    bytes: Vec<u8>,
}

struct VerifiedCandidate {
    version: String,
    channel: ReleaseChannel,
    key_id: String,
    manifest: Vec<u8>,
    manifest_signature: Vec<u8>,
    artifacts: Vec<VerifiedArtifact>,
}

pub(crate) fn import_signed_candidate(
    catalog: &BoardCatalog,
    candidate: &Path,
    reporter: Reporter,
) -> Result<ImportedCandidate, AppError> {
    if !pinned_key_is_configured() {
        return Err(AppError::trust_signing(
            "release key custody is not configured; release/keys/minisign.pub still contains the fail-closed marker",
        ));
    }
    import_with(
        catalog,
        candidate,
        &root()?,
        PINNED_MINISIGN_PUBLIC_KEY,
        &MinisignVerifier,
        reporter,
    )
    .map_err(candidate_app_error)
}

fn import_with(
    catalog: &BoardCatalog,
    candidate: &Path,
    cache_root: &Path,
    trusted_public_key: &str,
    verifier: &dyn SignatureVerifier,
    reporter: Reporter,
) -> Result<ImportedCandidate, CandidateError> {
    import_with_limits(
        catalog,
        candidate,
        cache_root,
        trusted_public_key,
        verifier,
        reporter,
        CANDIDATE_LIMITS,
    )
}

fn import_with_limits(
    catalog: &BoardCatalog,
    candidate: &Path,
    cache_root: &Path,
    trusted_public_key: &str,
    verifier: &dyn SignatureVerifier,
    reporter: Reporter,
    limits: CandidateLimits,
) -> Result<ImportedCandidate, CandidateError> {
    reporter.phase(
        Phase::ValidatingManifest,
        None,
        "Verifying signed candidate identity and checksums…",
    );
    let verified = verify_candidate(
        catalog,
        candidate,
        trusted_public_key,
        verifier,
        reporter,
        limits,
    )?;
    check_cancelled()?;
    reporter.phase(
        Phase::PublishingCache,
        None,
        "Publishing the verified candidate to the immutable local cache…",
    );
    storage::publish(cache_root, &verified, catalog, trusted_public_key, verifier)?;

    Ok(ImportedCandidate {
        version: verified.version,
        channel: channel_name(verified.channel),
        artifact_count: verified.artifacts.len(),
        artifact_bytes: verified
            .artifacts
            .iter()
            .map(|artifact| artifact.bytes.len() as u64)
            .sum(),
    })
}

fn verify_candidate(
    catalog: &BoardCatalog,
    root: &Path,
    trusted_public_key: &str,
    verifier: &dyn SignatureVerifier,
    reporter: Reporter,
    limits: CandidateLimits,
) -> Result<VerifiedCandidate, CandidateError> {
    let payload_files = walk_payload_files_with_limits(root, limits)?;
    check_cancelled()?;

    let key_path = root.join("minisign.pub");
    let candidate_key = read_limited(&key_path, MAX_PUBLIC_KEY_BYTES)?;
    if candidate_key != trusted_public_key.as_bytes() {
        return Err(CandidateError::PublicKeyMismatch);
    }
    let key_id =
        minisign_public_key_id(trusted_public_key).ok_or(CandidateError::InvalidPinnedKeyId)?;

    let manifest_path = root.join("flash-manifest.json");
    let manifest_bytes = read_limited(&manifest_path, MAX_MANIFEST_BYTES)?;
    let manifest_signature = verify_signed_file(
        &manifest_path,
        &manifest_bytes,
        trusted_public_key,
        verifier,
    )?;
    let manifest = ValidatedFlashManifest::from_json(&manifest_bytes, catalog)
        .map_err(|error| CandidateError::Manifest(error.to_string()))?;
    if manifest.signing().key_id().as_str() != key_id.to_ascii_uppercase() {
        return Err(CandidateError::ReleaseIdentity);
    }

    let version_path = root.join("VERSION");
    let version_bytes = read_limited(&version_path, 256)?;
    let version = std::str::from_utf8(&version_bytes)
        .map_err(|source| CandidateError::Utf8 {
            path: version_path,
            source,
        })?
        .trim();
    if version != manifest.release().version().as_str() {
        return Err(CandidateError::ReleaseIdentity);
    }

    let descriptor_path = exact_channel_file(root, manifest.release().channel())?;
    let descriptor_bytes = read_limited(&descriptor_path, MAX_CHANNEL_BYTES)?;
    verify_signed_file(
        &descriptor_path,
        &descriptor_bytes,
        trusted_public_key,
        verifier,
    )?;
    let descriptor =
        ValidatedChannelDescriptor::from_json(&descriptor_bytes, manifest.release().channel())
            .map_err(|error| CandidateError::Channel(error.to_string()))?;
    if descriptor.version() != manifest.release().version()
        || descriptor.manifest_sha256().as_str() != sha256_hex(&manifest_bytes)
    {
        return Err(CandidateError::ReleaseIdentity);
    }

    verify_checksums(root, trusted_public_key, verifier, &payload_files)?;

    let mut destinations = BTreeSet::new();
    let mut artifacts = Vec::new();
    for target in manifest.targets() {
        for part in target.parts() {
            let board_slug = target.board_id().as_str();
            let part_path = part.path().as_str();
            check_cancelled()?;
            reporter.phase(
                Phase::VerifyingArtifacts,
                Some(board_slug),
                &format!("Verifying {} ({} bytes)…", part_path, part.size()),
            );
            let source = safe_join(root, part_path)?;
            if part.size() > MAX_ARTIFACT_BYTES {
                return Err(CandidateError::FileTooLarge {
                    path: source,
                    limit: MAX_ARTIFACT_BYTES,
                });
            }
            let limit = part
                .size()
                .checked_add(1)
                .ok_or_else(|| CandidateError::FileTooLarge {
                    path: source.clone(),
                    limit: part.size(),
                })?;
            let bytes = read_limited(&source, limit)?;
            if bytes.len() as u64 != part.size() || sha256_hex(&bytes) != part.sha256().as_str() {
                return Err(CandidateError::ArtifactMismatch { path: source });
            }
            let file_name = Path::new(part_path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| CandidateError::UnsafePath {
                    path: part_path.to_string(),
                })?
                .to_string();
            let destination = Path::new(board_slug).join(&file_name);
            if !destinations.insert(destination.clone()) {
                return Err(CandidateError::CachePathCollision { path: destination });
            }
            artifacts.push(VerifiedArtifact {
                board_slug: board_slug.to_string(),
                file_name,
                bytes,
            });
        }
    }

    Ok(VerifiedCandidate {
        version: manifest.release().version().as_str().to_string(),
        channel: manifest.release().channel(),
        key_id: manifest.signing().key_id().as_str().to_string(),
        manifest: manifest_bytes,
        manifest_signature,
        artifacts,
    })
}

fn verify_checksums(
    root: &Path,
    public_key: &str,
    verifier: &dyn SignatureVerifier,
    actual_payloads: &BTreeSet<String>,
) -> Result<(), CandidateError> {
    let sums_path = root.join("SHA256SUMS.txt");
    let sums = read_limited(&sums_path, MAX_CHECKSUM_BYTES)?;
    let _ = verify_signed_file(&sums_path, &sums, public_key, verifier)?;
    let text = std::str::from_utf8(&sums).map_err(|source| CandidateError::Utf8 {
        path: sums_path,
        source,
    })?;
    let mut listed = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let (digest, relative) =
            line.split_once("  ")
                .ok_or_else(|| CandidateError::ChecksumLine {
                    line: line_number,
                    message: "expected lowercase SHA-256, two spaces, and a relative path"
                        .to_string(),
                })?;
        validate_digest(digest).map_err(|message| CandidateError::ChecksumLine {
            line: line_number,
            message,
        })?;
        let path = safe_join(root, relative)?;
        if listed
            .insert(relative.to_string(), digest.to_string())
            .is_some()
        {
            return Err(CandidateError::DuplicateChecksumPath(relative.to_string()));
        }
        if digest_file(&path)? != digest {
            return Err(CandidateError::ChecksumMismatch(relative.to_string()));
        }
    }

    let expected = listed.keys().cloned().collect::<BTreeSet<_>>();
    if &expected != actual_payloads {
        return Err(CandidateError::ChecksumCoverage {
            unlisted: actual_payloads.difference(&expected).cloned().collect(),
            missing: expected.difference(actual_payloads).cloned().collect(),
        });
    }
    Ok(())
}

fn exact_channel_file(root: &Path, channel: ReleaseChannel) -> Result<PathBuf, CandidateError> {
    let directory = root.join("channels");
    let descriptor = directory.join(format!("{}.json", channel_name(channel)));
    let signature = signature_path(&descriptor);
    let expected = [descriptor.clone(), signature.clone()]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(&directory).map_err(|source| CandidateError::Filesystem {
        action: "inspect",
        path: directory.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| CandidateError::Filesystem {
            action: "inspect",
            path: directory.clone(),
            source,
        })?;
        actual.insert(entry.path());
    }
    if actual != expected {
        return Err(CandidateError::ChannelSet);
    }
    Ok(descriptor)
}

fn verify_signed_file(
    path: &Path,
    bytes: &[u8],
    public_key: &str,
    verifier: &dyn SignatureVerifier,
) -> Result<Vec<u8>, CandidateError> {
    let signature_path = signature_path(path);
    let signature_bytes = read_limited(&signature_path, MAX_SIGNATURE_BYTES)?;
    let signature =
        std::str::from_utf8(&signature_bytes).map_err(|error| CandidateError::Signature {
            path: path.to_path_buf(),
            message: format!("signature document is not UTF-8: {error}"),
        })?;
    verifier
        .verify(bytes, signature, public_key)
        .map_err(|message| CandidateError::Signature {
            path: path.to_path_buf(),
            message,
        })?;
    Ok(signature_bytes)
}

pub(crate) fn root() -> Result<PathBuf, AppError> {
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| {
            path.join("Personal Reticulum")
                .join("hopspot-flash")
                .join("cache")
        });
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join("Library").join("Caches").join("hopspot-flash"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".cache"))
        })
        .map(|path| path.join("hopspot-flash"));
    root.ok_or_else(|| {
        AppError::host_preflight("this operating system has no user cache directory")
    })
}

pub(crate) fn store_immutable(
    cache_root: &Path,
    path: &Path,
    bytes: &[u8],
) -> Result<(), AppError> {
    storage::store_immutable(cache_root, path, bytes).map_err(candidate_app_error)
}

pub(crate) fn publish_verified_channel(
    cache_root: &Path,
    channel: ReleaseChannel,
    descriptor: &[u8],
    signature: &[u8],
) -> Result<(), AppError> {
    storage::publish_verified_channel(cache_root, channel, descriptor, signature)
        .map_err(candidate_app_error)
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, CandidateError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| CandidateError::Filesystem {
        action: "inspect",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(CandidateError::UnsafeEntry {
            path: path.to_path_buf(),
        });
    }
    if metadata.len() > limit {
        return Err(CandidateError::FileTooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    let file = File::open(path).map_err(|source| CandidateError::Filesystem {
        action: "read",
        path: path.to_path_buf(),
        source,
    })?;
    let opened_metadata = file
        .metadata()
        .map_err(|source| CandidateError::Filesystem {
            action: "inspect open file",
            path: path.to_path_buf(),
            source,
        })?;
    if !opened_metadata.file_type().is_file() {
        return Err(CandidateError::UnsafeEntry {
            path: path.to_path_buf(),
        });
    }
    if opened_metadata.len() > limit {
        return Err(CandidateError::FileTooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| CandidateError::Filesystem {
            action: "read",
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > limit {
        return Err(CandidateError::FileTooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    Ok(bytes)
}

fn digest_file(path: &Path) -> Result<String, CandidateError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| CandidateError::Filesystem {
        action: "inspect",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(CandidateError::UnsafeEntry {
            path: path.to_path_buf(),
        });
    }
    let mut file = File::open(path).map_err(|source| CandidateError::Filesystem {
        action: "read",
        path: path.to_path_buf(),
        source,
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancelled()?;
        let count = file
            .read(&mut buffer)
            .map_err(|source| CandidateError::Filesystem {
                action: "read",
                path: path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn walk_payload_files_with_limits(
    root: &Path,
    limits: CandidateLimits,
) -> Result<BTreeSet<String>, CandidateError> {
    let root_metadata =
        fs::symlink_metadata(root).map_err(|source| CandidateError::Filesystem {
            action: "inspect",
            path: root.to_path_buf(),
            source,
        })?;
    if !root_metadata.file_type().is_dir() {
        return Err(CandidateError::InputNotDirectory {
            path: root.to_path_buf(),
        });
    }

    fn visit(
        root: &Path,
        directory: &Path,
        output: &mut BTreeSet<String>,
        depth: usize,
        entries_seen: &mut usize,
        bytes_seen: &mut u64,
        limits: CandidateLimits,
    ) -> Result<(), CandidateError> {
        if depth > MAX_CANDIDATE_DEPTH {
            return Err(CandidateError::CandidateTreeLimit {
                path: directory.to_path_buf(),
            });
        }
        for entry in fs::read_dir(directory).map_err(|source| CandidateError::Filesystem {
            action: "inspect",
            path: directory.to_path_buf(),
            source,
        })? {
            check_cancelled()?;
            *entries_seen = entries_seen.saturating_add(1);
            if *entries_seen > MAX_CANDIDATE_ENTRIES {
                return Err(CandidateError::CandidateTreeLimit {
                    path: directory.to_path_buf(),
                });
            }
            let entry = entry.map_err(|source| CandidateError::Filesystem {
                action: "inspect",
                path: directory.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|source| CandidateError::Filesystem {
                    action: "inspect",
                    path: path.clone(),
                    source,
                })?;
            if metadata.file_type().is_symlink() {
                return Err(CandidateError::UnsafeEntry { path });
            }
            if metadata.is_dir() {
                visit(
                    root,
                    &path,
                    output,
                    depth + 1,
                    entries_seen,
                    bytes_seen,
                    limits,
                )?;
            } else if metadata.is_file() {
                if metadata.len() > limits.file_bytes {
                    return Err(CandidateError::FileTooLarge {
                        path,
                        limit: limits.file_bytes,
                    });
                }
                *bytes_seen = bytes_seen.checked_add(metadata.len()).ok_or(
                    CandidateError::CandidateTooLarge {
                        limit: limits.total_bytes,
                    },
                )?;
                if *bytes_seen > limits.total_bytes {
                    return Err(CandidateError::CandidateTooLarge {
                        limit: limits.total_bytes,
                    });
                }
                let relative = relative_path(root, &path)?;
                if relative != "SHA256SUMS.txt"
                    && relative != "acceptance.json"
                    && !relative.ends_with(".minisig")
                {
                    output.insert(relative);
                }
            } else {
                return Err(CandidateError::UnsafeEntry { path });
            }
        }
        Ok(())
    }

    let mut output = BTreeSet::new();
    let mut entries_seen = 0;
    let mut bytes_seen = 0;
    visit(
        root,
        root,
        &mut output,
        0,
        &mut entries_seen,
        &mut bytes_seen,
        limits,
    )?;
    Ok(output)
}

fn relative_path(root: &Path, path: &Path) -> Result<String, CandidateError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CandidateError::UnsafePath {
            path: path.display().to_string(),
        })?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(CandidateError::UnsafePath {
                path: relative.display().to_string(),
            });
        };
        let value = value.to_str().ok_or_else(|| CandidateError::UnsafePath {
            path: relative.display().to_string(),
        })?;
        components.push(value);
    }
    Ok(components.join("/"))
}

fn safe_join(root: &Path, relative: impl AsRef<Path>) -> Result<PathBuf, CandidateError> {
    let relative = relative.as_ref();
    let rendered = relative
        .to_str()
        .ok_or_else(|| CandidateError::UnsafePath {
            path: relative.display().to_string(),
        })?;
    if rendered.contains('\\')
        || rendered.chars().any(char::is_control)
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CandidateError::UnsafePath {
            path: rendered.to_string(),
        });
    }
    Ok(root.join(relative))
}

fn signature_path(path: &Path) -> PathBuf {
    let mut signature_path = path.as_os_str().to_os_string();
    signature_path.push(".minisig");
    PathBuf::from(signature_path)
}

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!("invalid lowercase SHA-256 {value:?}"))
    }
}

fn channel_name(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Stable => "stable",
        ReleaseChannel::Preview => "preview",
    }
}

fn check_cancelled() -> Result<(), CandidateError> {
    if crate::esp::cancelled() {
        Err(CandidateError::Cancelled)
    } else {
        Ok(())
    }
}

fn candidate_app_error(error: CandidateError) -> AppError {
    match error {
        CandidateError::Cancelled => AppError::Cancelled,
        error => AppError::trust_candidate(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_flash_manifest::{
        board_catalog, BoardBuild, ChannelDescriptor, FlashManifest, FlashPart, FlashPartKind,
        NrfSerialDfuManifest, NrfSerialDfuRecoveryManifest, OfflineKeySigningInfo, ReleaseInfo,
        TargetManifest, Uf2VariantManifest, FLASH_MANIFEST_SCHEMA,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_PUBLIC_KEY: &str = "untrusted comment: minisign public key 1FB2CA18B2C25E1F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==\n";
    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct FakeVerifier;

    impl SignatureVerifier for FakeVerifier {
        fn verify(&self, bytes: &[u8], signature: &str, _public_key: &str) -> Result<(), String> {
            if signature == fake_signature(bytes) {
                Ok(())
            } else {
                Err("fake signature mismatch".to_string())
            }
        }
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "hopspot-flash-{label}-{}-{nonce}-{}",
                std::process::id(),
                TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        directory: TestDirectory,
        application_path: PathBuf,
    }

    fn fixture(seed: &str) -> Fixture {
        fixture_with_version(seed, "0.2.6")
    }

    fn fixture_with_version(seed: &str, version: &str) -> Fixture {
        let directory = TestDirectory::new(&format!("candidate-{seed}"));
        let catalog = board_catalog().expect("catalog");
        let mut application_path = None;
        let targets = catalog
            .shipping_boards()
            .map(|board| {
                let (flash_mode, flash_frequency, before_reset, after_reset, recipes) =
                    match &board.build {
                        BoardBuild::Esp(build) => (
                            Some(build.flash_mode.clone()),
                            Some(build.flash_frequency.clone()),
                            Some(build.before_reset.clone()),
                            Some(build.after_reset.clone()),
                            vec![
                                (FlashPartKind::Bootloader, "bootloader.bin", Some(0)),
                                (
                                    FlashPartKind::PartitionTable,
                                    "partition-table.bin",
                                    Some(0x8000),
                                ),
                                (FlashPartKind::Application, "application.bin", Some(0x10000)),
                            ],
                        ),
                        BoardBuild::Uf2(_) | BoardBuild::NrfSerialDfu(_) => {
                            (None, None, None, None, Vec::new())
                        }
                    };
                let parts = recipes
                    .into_iter()
                    .map(|(kind, name, offset)| {
                        let relative = format!("firmware/hopspot/{}/{version}/{name}", board.slug);
                        let bytes = format!("{seed}:{}:{name}", board.slug).into_bytes();
                        write_fixture(directory.path(), &relative, &bytes);
                        if board.slug == "heltec-v4" && kind == FlashPartKind::Application {
                            application_path = Some(directory.path().join(&relative));
                        }
                        FlashPart {
                            kind,
                            path: relative,
                            offset,
                            size: bytes.len() as u64,
                            sha256: sha256_hex(&bytes),
                        }
                    })
                    .collect();
                let variants = match &board.build {
                    BoardBuild::Esp(_) => Vec::new(),
                    BoardBuild::Uf2(build) => build
                        .variants
                        .iter()
                        .map(|variant| {
                            let relative = format!(
                                "firmware/hopspot/{}/{version}/{}",
                                board.slug, variant.filename
                            );
                            let bytes =
                                format!("{seed}:{}:{}", board.slug, variant.filename).into_bytes();
                            write_fixture(directory.path(), &relative, &bytes);
                            Uf2VariantManifest {
                                softdevice_family: variant.softdevice_family.clone(),
                                softdevice_version: variant.softdevice_version.clone(),
                                fwid: variant.fwid.clone(),
                                application_base: variant.application_base.clone(),
                                family_id: variant.family_id.clone(),
                                path: relative,
                                size: bytes.len() as u64,
                                sha256: sha256_hex(&bytes),
                            }
                        })
                        .collect(),
                    BoardBuild::NrfSerialDfu(_) => Vec::new(),
                };
                let nrf_serial_dfu = match &board.build {
                    BoardBuild::NrfSerialDfu(build) => {
                        let artifact = |name: &str, kind: FlashPartKind| {
                            let relative =
                                format!("firmware/hopspot/{}/{version}/{name}", board.slug);
                            let bytes = format!("{seed}:{}:{name}", board.slug).into_bytes();
                            write_fixture(directory.path(), &relative, &bytes);
                            FlashPart {
                                kind,
                                path: relative,
                                offset: None,
                                size: bytes.len() as u64,
                                sha256: sha256_hex(&bytes),
                            }
                        };
                        Some(NrfSerialDfuManifest {
                            serial: build.serial.clone(),
                            compatibility: build.compatibility.clone(),
                            application: artifact(
                                &build.application_filename,
                                FlashPartKind::DfuApplication,
                            ),
                            init_packet: artifact(
                                &build.init_packet_filename,
                                FlashPartKind::DfuInitPacket,
                            ),
                            recovery: NrfSerialDfuRecoveryManifest {
                                mount_label: build.recovery.mount_label.clone(),
                                board_id_prefix: build.recovery.board_identity.value.clone(),
                                family_id: build.recovery.family_id.clone(),
                                artifact: artifact(&build.recovery.filename, FlashPartKind::Uf2),
                            },
                        })
                    }
                    BoardBuild::Esp(_) | BoardBuild::Uf2(_) => None,
                };
                TargetManifest {
                    board_slug: board.slug.clone(),
                    display_name: board.display_name.clone(),
                    silicon: board.silicon.clone(),
                    interfaces: board.interfaces.clone(),
                    transport: board.transport,
                    expected_chip: board.expected_chip.clone(),
                    flash_size: board.flash_size,
                    flash_mode,
                    flash_frequency,
                    before_reset,
                    after_reset,
                    preparation_profile: board.preparation_profile.clone(),
                    parts,
                    variants,
                    nrf_serial_dfu,
                    provisioning: board.provisioning.clone(),
                    source: None,
                }
            })
            .collect();
        let manifest = FlashManifest {
            schema_version: FLASH_MANIFEST_SCHEMA,
            release: ReleaseInfo {
                version: version.to_string(),
                channel: ReleaseChannel::Preview,
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
            signing: OfflineKeySigningInfo {
                key_id: "1FB2CA18B2C25E1F".to_string(),
            },
            targets,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest JSON");
        write_fixture(
            directory.path(),
            "VERSION",
            format!("{version}\n").as_bytes(),
        );
        write_fixture(directory.path(), "minisign.pub", TEST_PUBLIC_KEY.as_bytes());
        write_fixture(directory.path(), "flash-manifest.json", &manifest_bytes);
        sign_fixture(directory.path(), "flash-manifest.json");

        let descriptor = ChannelDescriptor {
            schema_version: 1,
            channel: ReleaseChannel::Preview,
            version: version.to_string(),
            manifest_url: format!("https://reticulum.rs/releases/{version}/flash-manifest.json"),
            manifest_sha256: sha256_hex(&manifest_bytes),
        };
        write_fixture(
            directory.path(),
            "channels/preview.json",
            &serde_json::to_vec_pretty(&descriptor).expect("channel JSON"),
        );
        sign_fixture(directory.path(), "channels/preview.json");

        let payloads = walk_payload_files_with_limits(directory.path(), CANDIDATE_LIMITS)
            .expect("walk fixture");
        let sums = payloads
            .iter()
            .map(|relative| {
                format!(
                    "{}  {relative}",
                    digest_file(&directory.path().join(relative)).expect("fixture digest")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        write_fixture(directory.path(), "SHA256SUMS.txt", sums.as_bytes());
        sign_fixture(directory.path(), "SHA256SUMS.txt");

        Fixture {
            directory,
            application_path: application_path.expect("application path"),
        }
    }

    fn write_fixture(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create parent");
        fs::write(path, bytes).expect("write fixture");
    }

    fn fixture_file_sizes(root: &Path) -> (u64, u64) {
        let mut pending = vec![root.to_path_buf()];
        let mut total = 0u64;
        let mut largest = 0u64;
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).expect("read fixture directory") {
                let path = entry.expect("fixture entry").path();
                let metadata = fs::symlink_metadata(&path).expect("fixture metadata");
                if metadata.is_dir() {
                    pending.push(path);
                } else if metadata.is_file() {
                    total = total.checked_add(metadata.len()).expect("fixture size");
                    largest = largest.max(metadata.len());
                }
            }
        }
        (total, largest)
    }

    fn sign_fixture(root: &Path, relative: &str) {
        let path = root.join(relative);
        let bytes = fs::read(&path).expect("read signed fixture");
        fs::write(signature_path(&path), fake_signature(&bytes)).expect("write fake signature");
    }

    fn fake_signature(bytes: &[u8]) -> String {
        format!("test-signature:{}\n", sha256_hex(bytes))
    }

    #[test]
    fn production_minisign_adapter_verifies_a_standard_vector() {
        MinisignVerifier
            .verify(b"test", TEST_SIGNATURE, TEST_PUBLIC_KEY)
            .expect("test vector verifies");
    }

    #[test]
    fn valid_candidate_imports_every_manifest_artifact_for_offline_use() {
        let fixture = fixture("valid");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        let imported = import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("import valid fixture");

        assert_eq!(imported.version, "0.2.6");
        assert_eq!(imported.channel, "preview");
        assert_eq!(imported.artifact_count, 19);
        assert!(cache
            .path()
            .join("releases/0.2.6/heltec-v4/application.bin")
            .is_file());
        assert!(!cache.path().join("channels").exists());
    }

    #[test]
    fn candidate_import_cannot_move_an_offline_channel_head() {
        let fixture = fixture("head-isolation");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        storage::publish_verified_channel(
            cache.path(),
            ReleaseChannel::Preview,
            b"previously verified channel",
            b"previously verified signature",
        )
        .expect("publish existing channel");
        let head_path = cache.path().join("channels/preview/HEAD");
        let head_before = fs::read(&head_path).expect("read existing head");

        import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("import immutable candidate");

        assert_eq!(
            fs::read(head_path).expect("read unchanged head"),
            head_before
        );
    }

    #[test]
    fn verified_channel_publish_repairs_its_content_addressed_cache_files() {
        let cache = TestDirectory::new("cache");
        let descriptor = b"verified channel descriptor";
        let signature = b"verified channel signature";
        storage::publish_verified_channel(
            cache.path(),
            ReleaseChannel::Stable,
            descriptor,
            signature,
        )
        .expect("publish channel");
        let identifier = sha256_hex(descriptor);
        let signature_path = cache
            .path()
            .join("channels/stable")
            .join(format!("{identifier}.json.minisig"));
        fs::write(&signature_path, b"corrupt cache bytes").expect("corrupt signature cache");

        storage::publish_verified_channel(
            cache.path(),
            ReleaseChannel::Stable,
            descriptor,
            signature,
        )
        .expect("repair channel cache");

        assert_eq!(
            fs::read(signature_path).expect("read repaired signature"),
            signature
        );
        assert_eq!(
            fs::read_to_string(cache.path().join("channels/stable/HEAD"))
                .expect("read channel head"),
            format!("{identifier}\n")
        );
    }

    #[test]
    fn verified_reimport_repairs_corrupt_local_cache_bytes() {
        let fixture = fixture("repair");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("first import");

        let cached_application = cache
            .path()
            .join("releases/0.2.6/heltec-v4/application.bin");
        fs::write(&cached_application, b"corrupt cached application")
            .expect("corrupt cached application");

        import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("verified reimport repairs cache");

        assert_eq!(
            fs::read(&cached_application).expect("repaired application"),
            fs::read(&fixture.application_path).expect("candidate application")
        );
    }

    #[test]
    fn signed_manifest_with_the_wrong_cached_identity_is_repaired() {
        let candidate = fixture_with_version("identity-candidate", "0.2.6");
        let foreign = fixture_with_version("foreign-version", "0.2.5");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        import_with(
            &catalog,
            candidate.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("first import");
        let cached_manifest = cache.path().join("releases/0.2.6/flash-manifest.json");
        let cached_signature = cache
            .path()
            .join("releases/0.2.6/flash-manifest.json.minisig");
        fs::copy(
            foreign.directory.path().join("flash-manifest.json"),
            &cached_manifest,
        )
        .expect("install foreign signed manifest");
        fs::copy(
            foreign.directory.path().join("flash-manifest.json.minisig"),
            &cached_signature,
        )
        .expect("install foreign signature");

        import_with(
            &catalog,
            candidate.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("repair wrong cached identity");

        assert_eq!(
            fs::read(cached_manifest).expect("read repaired manifest"),
            fs::read(candidate.directory.path().join("flash-manifest.json"))
                .expect("read candidate manifest")
        );
    }

    #[cfg(unix)]
    #[test]
    fn verified_reimport_replaces_a_nested_cache_directory_symlink() {
        use std::os::unix::fs::symlink;

        let fixture = fixture("nested-cache-link");
        let cache = TestDirectory::new("cache");
        let outside = TestDirectory::new("outside-board-cache");
        let catalog = board_catalog().expect("catalog");
        import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("first import");
        let cached_board = cache.path().join("releases/0.2.6/heltec-v4");
        let outside_board = outside.path().join("heltec-v4");
        fs::rename(&cached_board, &outside_board).expect("move board cache outside");
        let outside_application =
            fs::read(outside_board.join("application.bin")).expect("read outside application");
        symlink(&outside_board, &cached_board).expect("install nested cache symlink");

        import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("repair nested cache symlink");

        let metadata = fs::symlink_metadata(&cached_board).expect("inspect repaired board cache");
        assert!(metadata.file_type().is_dir());
        assert_eq!(
            fs::read(outside_board.join("application.bin")).expect("outside application remains"),
            outside_application
        );
    }

    #[test]
    fn verified_reimport_replaces_an_overdeep_cached_release_tree() {
        let fixture = fixture("overdeep-cache");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("first import");
        let junk_root = cache.path().join("releases/0.2.6/untrusted-depth");
        let mut current = junk_root.clone();
        for _ in 0..=storage::MAX_CACHED_RELEASE_DEPTH {
            fs::create_dir(&current).expect("create overdeep cache entry");
            current.push("nested");
        }

        import_with(
            &catalog,
            fixture.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("replace overdeep cache tree");

        assert!(!junk_root.exists());
    }

    #[test]
    fn tampered_or_partial_candidate_never_becomes_visible() {
        let tampered = fixture("tampered");
        fs::write(&tampered.application_path, b"tampered").expect("tamper application");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        assert!(matches!(
            import_with(
                &catalog,
                tampered.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::ChecksumMismatch(_) | CandidateError::ArtifactMismatch { .. })
        ));
        assert!(!cache.path().join("releases/0.2.6").exists());
        assert!(!cache.path().join("channels/preview").exists());

        let partial = fixture("partial");
        fs::remove_file(partial.directory.path().join("flash-manifest.json.minisig"))
            .expect("remove signature");
        assert!(import_with(
            &catalog,
            partial.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .is_err());
        assert!(!cache.path().join("releases/0.2.6").exists());
    }

    #[test]
    fn oversized_candidate_payload_is_rejected_before_cache_publication() {
        let fixture = fixture("oversized-payload");
        let (_, largest) = fixture_file_sizes(fixture.directory.path());
        let oversized_path = "unexpected-large-payload.bin";
        write_fixture(
            fixture.directory.path(),
            oversized_path,
            &vec![0; usize::try_from(largest + 1).expect("fixture allocation")],
        );
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");

        assert!(matches!(
            import_with_limits(
                &catalog,
                fixture.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
                CandidateLimits {
                    file_bytes: largest,
                    total_bytes: MAX_CANDIDATE_BYTES,
                },
            ),
            Err(CandidateError::FileTooLarge { path, limit })
                if path.ends_with(oversized_path) && limit == largest
        ));
        assert!(!cache.path().join("releases/0.2.6").exists());
        assert!(!cache.path().join("channels/preview").exists());
    }

    #[test]
    fn aggregate_candidate_limit_is_rejected_before_cache_publication() {
        let fixture = fixture("aggregate-limit");
        let (fixture_bytes, _) = fixture_file_sizes(fixture.directory.path());
        write_fixture(fixture.directory.path(), "aggregate-overflow.bin", b"x");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");

        assert!(matches!(
            import_with_limits(
                &catalog,
                fixture.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
                CandidateLimits {
                    file_bytes: MAX_CANDIDATE_FILE_BYTES,
                    total_bytes: fixture_bytes,
                },
            ),
            Err(CandidateError::CandidateTooLarge { limit }) if limit == fixture_bytes
        ));
        assert!(!cache.path().join("releases/0.2.6").exists());
        assert!(!cache.path().join("channels/preview").exists());
    }

    #[test]
    fn wrong_key_and_extra_channel_are_rejected() {
        let wrong_key = fixture("wrong-key");
        fs::write(
            wrong_key.directory.path().join("minisign.pub"),
            TEST_PUBLIC_KEY.replace("1FB2CA18B2C25E1F", "0000000000000000"),
        )
        .expect("replace key");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        assert!(matches!(
            import_with(
                &catalog,
                wrong_key.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::PublicKeyMismatch)
        ));

        let extra_channel = fixture("extra-channel");
        write_fixture(
            extra_channel.directory.path(),
            "channels/stable.json",
            b"{}",
        );
        write_fixture(
            extra_channel.directory.path(),
            "channels/stable.json.minisig",
            b"unused",
        );
        assert!(matches!(
            import_with(
                &catalog,
                extra_channel.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::ChannelSet)
        ));
    }

    #[test]
    fn corrupt_signature_and_checksum_path_escape_are_rejected() {
        let corrupt = fixture("corrupt-signature");
        fs::write(
            corrupt.directory.path().join("flash-manifest.json.minisig"),
            b"corrupt signature",
        )
        .expect("corrupt signature");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        assert!(matches!(
            import_with(
                &catalog,
                corrupt.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::Signature { .. })
        ));

        let escaping = fixture("escaping-checksum");
        let sums_path = escaping.directory.path().join("SHA256SUMS.txt");
        fs::write(&sums_path, format!("{}  ../outside\n", "0".repeat(64))).expect("replace sums");
        fs::write(
            signature_path(&sums_path),
            fake_signature(&fs::read(&sums_path).expect("read sums")),
        )
        .expect("resign sums");
        assert!(matches!(
            import_with(
                &catalog,
                escaping.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::UnsafePath { .. })
        ));
        assert!(!cache.path().join("releases/0.2.6").exists());
    }

    #[test]
    fn signed_checksum_coverage_rejects_an_unlisted_payload() {
        let fixture = fixture("unlisted-payload");
        write_fixture(
            fixture.directory.path(),
            "firmware/hopspot/heltec-v4/0.2.6/unlisted.bin",
            b"not covered by the signed checksum document",
        );
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        assert!(matches!(
            import_with(
                &catalog,
                fixture.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::ChecksumCoverage { .. })
        ));
        assert!(!cache.path().join("releases/0.2.6").exists());
    }

    #[test]
    fn existing_version_is_immutable_and_failed_reimport_is_atomic() {
        let first = fixture("first");
        let second = fixture("second");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        import_with(
            &catalog,
            first.directory.path(),
            cache.path(),
            TEST_PUBLIC_KEY,
            &FakeVerifier,
            Reporter::human(),
        )
        .expect("first import");
        let cached_application = cache
            .path()
            .join("releases/0.2.6/heltec-v4/application.bin");
        let before = fs::read(&cached_application).expect("cached application");

        assert!(matches!(
            import_with(
                &catalog,
                second.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::ImmutableConflict { .. })
        ));
        assert_eq!(
            fs::read(cached_application).expect("cached application remains"),
            before
        );
        let staging = fs::read_dir(cache.path().join("releases"))
            .expect("release cache")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".import-"))
            .count();
        assert_eq!(staging, 0);
    }

    #[cfg(unix)]
    #[test]
    fn candidate_symlinks_are_rejected_before_trust_documents_are_read() {
        use std::os::unix::fs::symlink;

        let fixture = fixture("symlink");
        symlink(
            fixture.directory.path().join("VERSION"),
            fixture.directory.path().join("unexpected-link"),
        )
        .expect("create symlink");
        let cache = TestDirectory::new("cache");
        let catalog = board_catalog().expect("catalog");
        assert!(matches!(
            import_with(
                &catalog,
                fixture.directory.path(),
                cache.path(),
                TEST_PUBLIC_KEY,
                &FakeVerifier,
                Reporter::human(),
            ),
            Err(CandidateError::UnsafeEntry { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cache_publication_rejects_symlinked_storage_directories() {
        use std::os::unix::fs::symlink;

        let cache = TestDirectory::new("cache-parent-link");
        let outside = TestDirectory::new("outside-cache");
        symlink(outside.path(), cache.path().join("releases")).expect("create releases symlink");
        let target = cache.path().join("releases/0.2.6/application.bin");
        assert!(matches!(
            storage::store_immutable(cache.path(), &target, b"verified"),
            Err(CandidateError::UnsafeEntry { .. })
        ));
        assert!(fs::read_dir(outside.path())
            .expect("inspect outside directory")
            .next()
            .is_none());

        fs::remove_file(cache.path().join("releases")).expect("remove releases symlink");
        symlink(outside.path(), cache.path().join("channels")).expect("create channels symlink");
        assert!(matches!(
            storage::publish_verified_channel(
                cache.path(),
                ReleaseChannel::Preview,
                b"descriptor",
                b"signature",
            ),
            Err(CandidateError::UnsafeEntry { .. })
        ));
        assert!(fs::read_dir(outside.path())
            .expect("inspect outside directory")
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cache_publication_rejects_equal_bytes_behind_a_parent_symlink() {
        use std::os::unix::fs::symlink;

        let cache = TestDirectory::new("cache-equal-parent-link");
        let outside = TestDirectory::new("outside-equal-cache");
        let outside_release = outside.path().join("0.2.6");
        fs::create_dir(&outside_release).expect("create outside release");
        fs::write(outside_release.join("application.bin"), b"verified")
            .expect("write matching outside bytes");
        symlink(outside.path(), cache.path().join("releases")).expect("create releases symlink");
        let redirected = cache.path().join("releases/0.2.6/application.bin");

        assert!(matches!(
            storage::store_immutable(cache.path(), &redirected, b"verified"),
            Err(CandidateError::UnsafeEntry { .. })
        ));
        assert_eq!(
            fs::read(outside_release.join("application.bin")).expect("outside bytes remain"),
            b"verified"
        );
    }
}
