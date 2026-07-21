use std::fs;
use std::path::{Path, PathBuf};

use prns_flash_manifest::{
    pinned_key_id, pinned_key_is_configured, sha256_hex, verify_minisign, BoardCatalog,
    ChannelDescriptor, FlashManifest, FlashPart, ReleaseChannel, TargetManifest,
    PINNED_MINISIGN_PUBLIC_KEY,
};
use url::Url;

use crate::cli::ChannelArg;
use crate::error::AppError;
use crate::events::Reporter;

const CHANNEL_BASE_URL: &str = "https://reticulum.rs/releases/channels/";
const IMMUTABLE_RELEASE_BASE_URL: &str = "https://reticulum.rs/releases/";
const MAX_MANIFEST_BYTES: u64 = 512 * 1024;

#[derive(Debug)]
pub(crate) struct PreparedTarget {
    pub(crate) version: String,
    pub(crate) target: TargetManifest,
    pub(crate) parts: Vec<PreparedPart>,
}

#[derive(Debug)]
pub(crate) struct PreparedPart {
    pub(crate) descriptor: FlashPart,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) fn prepare_candidate_target(
    catalog: &BoardCatalog,
    board_slug: &str,
    channel: ChannelArg,
    candidate: &Path,
    reporter: Reporter,
) -> Result<PreparedTarget, AppError> {
    if !pinned_key_is_configured() {
        return Err(AppError::trust(
            "release key custody is not configured; release/keys/minisign.pub still contains the fail-closed marker",
        ));
    }
    let candidate_key = fs::read_to_string(candidate.join("minisign.pub")).map_err(|error| {
        AppError::trust(format!(
            "could not read candidate Minisign public key: {error}"
        ))
    })?;
    if candidate_key != PINNED_MINISIGN_PUBLIC_KEY {
        return Err(AppError::trust(
            "candidate public key differs from the CLI's pinned release key",
        ));
    }
    let channel_name = channel.as_str();
    let descriptor_path = candidate
        .join("channels")
        .join(format!("{channel_name}.json"));
    let descriptor_bytes = fs::read(&descriptor_path).map_err(|error| {
        AppError::trust(format!("could not read signed candidate channel: {error}"))
    })?;
    verify_local_signature(&descriptor_path, &descriptor_bytes)?;
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    let descriptor = ChannelDescriptor::from_json(&descriptor_bytes, expected_channel)
        .map_err(|error| AppError::trust(error.to_string()))?;

    reporter.phase(
        "validating_manifest",
        Some(board_slug),
        &format!(
            "Verifying local signed Hopspot candidate {}…",
            descriptor.version
        ),
    );
    let manifest_path = candidate.join("flash-manifest.json");
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| AppError::trust(format!("could not read candidate manifest: {error}")))?;
    verify_local_signature(&manifest_path, &manifest_bytes)?;
    verify_hash(
        &manifest_bytes,
        &descriptor.manifest_sha256,
        "flash manifest",
    )?;
    let manifest = FlashManifest::from_json(&manifest_bytes, catalog)
        .map_err(|error| AppError::trust(error.to_string()))?;
    verify_manifest_key_id(&manifest)?;
    if manifest.release.version != descriptor.version
        || manifest.release.channel != expected_channel
    {
        return Err(AppError::trust(
            "candidate channel and manifest release identity disagree",
        ));
    }
    let target = manifest
        .targets
        .into_iter()
        .find(|target| target.board_slug == board_slug)
        .ok_or_else(|| {
            AppError::trust(format!("candidate does not contain board {board_slug:?}"))
        })?;
    let mut parts = Vec::with_capacity(target.parts.len());
    for part in &target.parts {
        reporter.phase(
            "verifying_artifacts",
            Some(board_slug),
            &format!("Verifying local {} ({} bytes)…", part.path, part.size),
        );
        let path = candidate.join(&part.path);
        let bytes = fs::read(&path).map_err(|error| {
            AppError::trust(format!(
                "could not read candidate artifact {}: {error}",
                path.display()
            ))
        })?;
        if bytes.len() as u64 != part.size {
            return Err(AppError::trust(format!(
                "candidate artifact {:?} is {} bytes; manifest requires {}",
                part.path,
                bytes.len(),
                part.size
            )));
        }
        verify_hash(&bytes, &part.sha256, &part.path)?;
        parts.push(PreparedPart {
            descriptor: part.clone(),
            bytes,
        });
    }
    Ok(PreparedTarget {
        version: descriptor.version,
        target,
        parts,
    })
}

fn verify_local_signature(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let signature_path = PathBuf::from(format!("{}.minisig", path.display()));
    let signature = fs::read_to_string(&signature_path).map_err(|error| {
        AppError::trust(format!(
            "could not read candidate signature {}: {error}",
            signature_path.display()
        ))
    })?;
    verify_minisign(bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| AppError::trust(error.to_string()))
}

pub(crate) fn prepare_published_target(
    catalog: &BoardCatalog,
    board_slug: &str,
    channel: ChannelArg,
    version: Option<&str>,
    offline: bool,
    reporter: Reporter,
) -> Result<PreparedTarget, AppError> {
    if !pinned_key_is_configured() {
        return Err(AppError::trust(
            "release key custody is not configured; release/keys/minisign.pub still contains the fail-closed marker",
        ));
    }
    let cache = cache_root()?;
    let (version, manifest_url, expected_manifest_hash) = match version {
        Some(version) => (
            validate_version(version)?.to_string(),
            immutable_manifest_url(version)?,
            None,
        ),
        None => resolve_channel(channel, offline, &cache, reporter)?,
    };

    reporter.phase(
        "validating_manifest",
        Some(board_slug),
        &format!("Verifying signed Hopspot release {version}…"),
    );
    let manifest_cache = cache
        .join("releases")
        .join(&version)
        .join("flash-manifest.json");
    let signature_cache = manifest_cache.with_extension("json.minisig");
    let manifest_bytes = acquire(&manifest_url, &manifest_cache, offline, MAX_MANIFEST_BYTES)?;
    let signature_url = format!("{manifest_url}.minisig");
    let signature_bytes = acquire(&signature_url, &signature_cache, offline, 64 * 1024)?;
    let signature = String::from_utf8(signature_bytes)
        .map_err(|error| AppError::trust(format!("manifest signature is not UTF-8: {error}")))?;
    verify_minisign(&manifest_bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| AppError::trust(error.to_string()))?;
    if let Some(expected_hash) = expected_manifest_hash {
        verify_hash(&manifest_bytes, &expected_hash, "flash manifest")?;
    }
    let manifest = FlashManifest::from_json(&manifest_bytes, catalog)
        .map_err(|error| AppError::trust(error.to_string()))?;
    verify_manifest_key_id(&manifest)?;
    if manifest.release.version != version {
        return Err(AppError::trust(format!(
            "signed manifest version {:?} does not match selected release {:?}",
            manifest.release.version, version
        )));
    }
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    if manifest.release.channel != expected_channel {
        return Err(AppError::trust(format!(
            "signed manifest channel {:?} does not match requested channel {:?}",
            manifest.release.channel, expected_channel
        )));
    }
    if !offline {
        atomic_store(&manifest_cache, &manifest_bytes)?;
        atomic_store(&signature_cache, signature.as_bytes())?;
    }
    let target = manifest
        .targets
        .into_iter()
        .find(|target| target.board_slug == board_slug)
        .ok_or_else(|| AppError::trust(format!("release does not contain board {board_slug:?}")))?;

    let base = Url::parse(&manifest_url)
        .map_err(|error| AppError::trust(format!("invalid manifest URL: {error}")))?;
    let mut parts = Vec::with_capacity(target.parts.len());
    for part in &target.parts {
        reporter.phase(
            "downloading",
            Some(board_slug),
            &format!("Acquiring {} ({} bytes)…", part.path, part.size),
        );
        let part_url = base
            .join(&part.path)
            .map_err(|error| AppError::trust(format!("invalid artifact URL: {error}")))?;
        enforce_https(&part_url)?;
        let file_name = Path::new(&part.path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::trust(format!("invalid artifact path {:?}", part.path)))?;
        let part_cache = cache
            .join("releases")
            .join(&version)
            .join(&target.board_slug)
            .join(file_name);
        let limit = part
            .size
            .checked_add(1)
            .ok_or_else(|| AppError::trust("artifact size overflows download limit"))?;
        let bytes = acquire(part_url.as_str(), &part_cache, offline, limit)?;
        if bytes.len() as u64 != part.size {
            return Err(AppError::trust(format!(
                "artifact {:?} is {} bytes; signed manifest requires {}",
                part.path,
                bytes.len(),
                part.size
            )));
        }
        verify_hash(&bytes, &part.sha256, &part.path)?;
        if !offline {
            atomic_store(&part_cache, &bytes)?;
        }
        parts.push(PreparedPart {
            descriptor: part.clone(),
            bytes,
        });
    }

    Ok(PreparedTarget {
        version,
        target,
        parts,
    })
}

fn verify_manifest_key_id(manifest: &FlashManifest) -> Result<(), AppError> {
    let expected = pinned_key_id()
        .ok_or_else(|| AppError::trust("pinned release key has no canonical key ID"))?;
    if manifest.signing.key_id.eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err(AppError::trust(format!(
            "manifest key ID {:?} differs from pinned key {expected}",
            manifest.signing.key_id
        )))
    }
}

fn resolve_channel(
    channel: ChannelArg,
    offline: bool,
    cache: &Path,
    reporter: Reporter,
) -> Result<(String, String, Option<String>), AppError> {
    let channel_name = channel.as_str();
    reporter.phase(
        "resolving_release",
        None,
        &format!("Resolving signed {channel_name} channel…"),
    );
    if offline {
        return load_cached_channel(channel, cache);
    }
    let base = std::env::var("PRNS_FLASH_CHANNEL_BASE_URL")
        .unwrap_or_else(|_| CHANNEL_BASE_URL.to_string());
    let url = format!(
        "{}{channel_name}.json",
        base.trim_end_matches('/').to_string() + "/"
    );
    let bytes = download(&url, 64 * 1024)?;
    let signature_bytes = download(&format!("{url}.minisig"), 64 * 1024)?;
    let signature = String::from_utf8(signature_bytes)
        .map_err(|error| AppError::trust(format!("channel signature is not UTF-8: {error}")))?;
    verify_minisign(&bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| AppError::trust(error.to_string()))?;
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    let descriptor = ChannelDescriptor::from_json(&bytes, expected_channel)
        .map_err(|error| AppError::trust(error.to_string()))?;
    let manifest_url = Url::parse(&descriptor.manifest_url)
        .map_err(|error| AppError::trust(format!("invalid signed manifest URL: {error}")))?;
    enforce_https(&manifest_url)?;
    let cache_id = sha256_hex(&bytes);
    let descriptor_cache = cache
        .join("channels")
        .join(channel_name)
        .join(format!("{cache_id}.json"));
    let signature_cache = descriptor_cache.with_extension("json.minisig");
    atomic_store(&descriptor_cache, &bytes)?;
    atomic_store(&signature_cache, signature.as_bytes())?;
    Ok((
        descriptor.version,
        descriptor.manifest_url,
        Some(descriptor.manifest_sha256),
    ))
}

fn load_cached_channel(
    channel: ChannelArg,
    cache: &Path,
) -> Result<(String, String, Option<String>), AppError> {
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    let directory = cache.join("channels").join(channel.as_str());
    let entries = fs::read_dir(&directory).map_err(|error| {
        AppError::trust(format!(
            "verified offline channel cache {} is unavailable: {error}",
            directory.display()
        ))
    })?;
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata.modified().ok();
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let signature_path = path.with_extension("json.minisig");
        let Ok(signature) = fs::read_to_string(signature_path) else {
            continue;
        };
        if verify_minisign(&bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY).is_err() {
            continue;
        }
        let Ok(descriptor) = ChannelDescriptor::from_json(&bytes, expected_channel) else {
            continue;
        };
        candidates.push((modified, descriptor));
    }
    candidates.sort_by_key(|(modified, _)| *modified);
    let descriptor = candidates
        .pop()
        .map(|(_, descriptor)| descriptor)
        .ok_or_else(|| AppError::trust("no verified offline channel descriptor is cached"))?;
    Ok((
        descriptor.version,
        descriptor.manifest_url,
        Some(descriptor.manifest_sha256),
    ))
}

fn immutable_manifest_url(version: &str) -> Result<String, AppError> {
    validate_version(version)?;
    Ok(format!(
        "{IMMUTABLE_RELEASE_BASE_URL}{version}/flash-manifest.json"
    ))
}

fn validate_version(version: &str) -> Result<&str, AppError> {
    let valid = !version.is_empty()
        && !version.eq_ignore_ascii_case("next")
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
    if valid {
        Ok(version)
    } else {
        Err(AppError::usage(format!(
            "invalid release version {version:?}"
        )))
    }
}

fn cache_root() -> Result<PathBuf, AppError> {
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
    root.ok_or_else(|| AppError::preflight("this operating system has no user cache directory"))
}

fn acquire(url: &str, cache_path: &Path, offline: bool, limit: u64) -> Result<Vec<u8>, AppError> {
    if offline {
        return fs::read(cache_path).map_err(|error| {
            AppError::trust(format!(
                "verified offline cache entry {} is unavailable: {error}",
                cache_path.display()
            ))
        });
    }
    download(url, limit)
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, AppError> {
    if crate::esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    let parsed = Url::parse(url)
        .map_err(|error| AppError::trust(format!("invalid release URL {url:?}: {error}")))?;
    enforce_https(&parsed)?;
    let mut response = ureq::get(url)
        .header(
            "User-Agent",
            concat!("hopspot-flash/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| AppError::trust(format!("download failed for {url}: {error}")))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| AppError::trust(format!("could not read {url}: {error}")))?;
    if crate::esp::cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(bytes)
    }
}

fn enforce_https(url: &Url) -> Result<(), AppError> {
    if url.scheme() != "https" {
        return Err(AppError::trust(format!(
            "release URL must use HTTPS: {url}"
        )));
    }
    Ok(())
}

fn verify_hash(bytes: &[u8], expected: &str, label: &str) -> Result<(), AppError> {
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(AppError::trust(format!(
            "SHA-256 mismatch for {label}: expected {expected}, found {actual}"
        )))
    }
}

fn atomic_store(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::trust(format!("cache path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|error| AppError::trust(format!("could not create cache: {error}")))?;
    if path.is_file() {
        let existing = fs::read(path)
            .map_err(|error| AppError::trust(format!("could not read cache entry: {error}")))?;
        if existing == bytes {
            return Ok(());
        }
        return Err(AppError::trust(format!(
            "immutable cache path {} already contains different verified bytes",
            path.display()
        )));
    }
    let temporary = path.with_extension(format!("part-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| {
        AppError::trust(format!("could not write cache temporary file: {error}"))
    })?;
    fs::rename(&temporary, path)
        .map_err(|error| AppError::trust(format!("could not publish cache entry: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_cache() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("hopspot-flash-cache-{nonce}"))
    }

    #[test]
    fn versions_cannot_escape_release_paths() {
        assert!(validate_version("0.2.6").is_ok());
        assert!(validate_version("../latest").is_err());
        assert!(validate_version("next").is_err());
    }

    #[test]
    fn hash_mismatch_is_a_trust_error() {
        assert!(matches!(
            verify_hash(b"payload", &"0".repeat(64), "test"),
            Err(AppError::Trust(_))
        ));
    }

    #[test]
    fn verified_cache_publication_is_atomic_and_immutable() -> Result<(), AppError> {
        let root = temporary_cache();
        let path = root.join("releases/0.2.6/application.bin");
        atomic_store(&path, b"verified")?;
        assert_eq!(fs::read(&path).expect("read cache"), b"verified");
        atomic_store(&path, b"verified")?;
        assert!(matches!(
            atomic_store(&path, b"different"),
            Err(AppError::Trust(_))
        ));
        fs::remove_dir_all(root).expect("remove cache fixture");
        Ok(())
    }

    #[test]
    fn offline_cache_never_falls_back_to_network() {
        let root = temporary_cache();
        let missing = root.join("missing.bin");
        assert!(matches!(
            acquire("https://example.invalid/missing.bin", &missing, true, 64),
            Err(AppError::Trust(_))
        ));
    }
}
