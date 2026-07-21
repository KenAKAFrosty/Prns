//! Resolve immutable release sources and perform bounded online or offline acquisition.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use prns_flash_manifest::{
    sha256_hex, verify_minisign, ReleaseChannel, ReleaseVersion, Sha256Digest,
    ValidatedChannelDescriptor, PINNED_MINISIGN_PUBLIC_KEY,
};
use url::Url;

use crate::cache;
use crate::cli::ChannelArg;
use crate::error::AppError;
use crate::events::{Phase, Reporter};

const CHANNEL_BASE_URL: &str = "https://reticulum.rs/releases/channels/";
const IMMUTABLE_RELEASE_BASE_URL: &str = "https://reticulum.rs/releases/";
const MAX_CHANNEL_BYTES: u64 = 64 * 1024;
const CHANNEL_HEAD_BYTES: u64 = 65;

pub(super) const MAX_MANIFEST_BYTES: u64 = 512 * 1024;
pub(super) const MAX_SIGNATURE_BYTES: u64 = 64 * 1024;

pub(super) fn resolve_channel(
    channel: ChannelArg,
    offline: bool,
    cache: &Path,
    reporter: Reporter,
) -> Result<(ReleaseVersion, String, Option<Sha256Digest>), AppError> {
    let channel_name = channel.as_str();
    reporter.phase(
        Phase::ResolvingRelease,
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
    let bytes = download(&url, MAX_CHANNEL_BYTES)?;
    let signature_bytes = download(&format!("{url}.minisig"), MAX_SIGNATURE_BYTES)?;
    let signature = String::from_utf8(signature_bytes).map_err(|error| {
        AppError::trust_signing(format!("channel signature is not UTF-8: {error}"))
    })?;
    verify_minisign(&bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| AppError::trust_signing(error.to_string()))?;
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    let descriptor = ValidatedChannelDescriptor::from_json(&bytes, expected_channel)
        .map_err(|error| AppError::trust_manifest(error.to_string()))?;
    let manifest_url = Url::parse(descriptor.manifest_url()).map_err(|error| {
        AppError::trust_identity(format!("invalid signed manifest URL: {error}"))
    })?;
    enforce_https(&manifest_url)?;
    cache::publish_verified_channel(cache, expected_channel, &bytes, signature.as_bytes())?;
    Ok((
        descriptor.version().clone(),
        descriptor.manifest_url().to_string(),
        Some(descriptor.manifest_sha256().clone()),
    ))
}

pub(super) fn immutable_manifest_url(version: &ReleaseVersion) -> Result<String, AppError> {
    Ok(format!(
        "{IMMUTABLE_RELEASE_BASE_URL}{version}/flash-manifest.json"
    ))
}

pub(super) fn validate_version(version: &str) -> Result<ReleaseVersion, AppError> {
    ReleaseVersion::parse(version.to_string())
        .map_err(|_| AppError::arguments(format!("invalid release version {version:?}")))
}

pub(super) fn acquire(
    url: &str,
    cache_path: &Path,
    offline: bool,
    limit: u64,
    cache_root: &Path,
) -> Result<Vec<u8>, AppError> {
    if offline {
        return read_cached_limited(cache_root, cache_path, limit);
    }
    download(url, limit)
}

pub(super) fn enforce_https(url: &Url) -> Result<(), AppError> {
    if url.scheme() != "https" {
        return Err(AppError::trust_identity(format!(
            "release URL must use HTTPS: {url}"
        )));
    }
    Ok(())
}

fn load_cached_channel(
    channel: ChannelArg,
    cache: &Path,
) -> Result<(ReleaseVersion, String, Option<Sha256Digest>), AppError> {
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    let directory = cache.join("channels").join(channel.as_str());
    let (identifier, descriptor_path, signature_path) = cached_channel_paths(cache, &directory)?;
    let bytes = read_cached_limited(cache, &descriptor_path, MAX_CHANNEL_BYTES)?;
    if sha256_hex(&bytes) != identifier {
        return Err(AppError::trust_cache(format!(
            "offline channel head {} does not match its descriptor bytes",
            directory.join("HEAD").display()
        )));
    }
    let signature_bytes = read_cached_limited(cache, &signature_path, MAX_SIGNATURE_BYTES)?;
    let signature = std::str::from_utf8(&signature_bytes).map_err(|error| {
        AppError::trust_cache(format!(
            "offline channel signature {} is not UTF-8: {error}",
            signature_path.display()
        ))
    })?;
    verify_minisign(&bytes, signature, PINNED_MINISIGN_PUBLIC_KEY).map_err(|error| {
        AppError::trust_cache(format!(
            "offline channel head failed signature verification: {error}"
        ))
    })?;
    let descriptor = ValidatedChannelDescriptor::from_json(&bytes, expected_channel)
        .map_err(|error| AppError::trust_cache(format!("offline channel is invalid: {error}")))?;
    Ok((
        descriptor.version().clone(),
        descriptor.manifest_url().to_string(),
        Some(descriptor.manifest_sha256().clone()),
    ))
}

pub(super) fn cached_channel_paths(
    cache_root: &Path,
    directory: &Path,
) -> Result<(String, PathBuf, PathBuf), AppError> {
    let head_path = directory.join("HEAD");
    let head_bytes = read_cached_limited(cache_root, &head_path, CHANNEL_HEAD_BYTES)?;
    let head = std::str::from_utf8(&head_bytes).map_err(|error| {
        AppError::trust_cache(format!(
            "offline channel head {} is not UTF-8: {error}",
            head_path.display()
        ))
    })?;
    let identifier = head.strip_suffix('\n').ok_or_else(|| {
        AppError::trust_cache(format!(
            "offline channel head {} has a non-canonical format",
            head_path.display()
        ))
    })?;
    if identifier.len() != 64
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(AppError::trust_cache(format!(
            "offline channel head {} does not contain one lowercase SHA-256",
            head_path.display()
        )));
    }
    let descriptor = directory.join(format!("{identifier}.json"));
    let signature = descriptor.with_extension("json.minisig");
    Ok((identifier.to_string(), descriptor, signature))
}

fn read_cached_limited(cache_root: &Path, path: &Path, limit: u64) -> Result<Vec<u8>, AppError> {
    validate_cache_ancestry(cache_root, path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        AppError::trust_cache(format!(
            "verified offline cache entry {} is unavailable: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(AppError::trust_cache(format!(
            "offline cache entry {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > limit {
        return Err(AppError::trust_cache(format!(
            "offline cache entry {} exceeds its {limit}-byte limit",
            path.display()
        )));
    }
    let file = File::open(path).map_err(|error| {
        AppError::trust_cache(format!(
            "verified offline cache entry {} could not be opened: {error}",
            path.display()
        ))
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        AppError::trust_cache(format!(
            "verified offline cache entry {} could not be inspected: {error}",
            path.display()
        ))
    })?;
    if !opened_metadata.file_type().is_file() || opened_metadata.len() > limit {
        return Err(AppError::trust_cache(format!(
            "offline cache entry {} changed during validation",
            path.display()
        )));
    }
    let read_limit = limit
        .checked_add(1)
        .ok_or_else(|| AppError::trust_cache("offline cache read limit overflows"))?;
    let mut bytes = Vec::new();
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            AppError::trust_cache(format!(
                "verified offline cache entry {} could not be read: {error}",
                path.display()
            ))
        })?;
    if bytes.len() as u64 > limit {
        return Err(AppError::trust_cache(format!(
            "offline cache entry {} exceeds its {limit}-byte limit",
            path.display()
        )));
    }
    Ok(bytes)
}

fn validate_cache_ancestry(cache_root: &Path, path: &Path) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(|| {
        AppError::trust_cache(format!(
            "offline cache path {} has no parent",
            path.display()
        ))
    })?;
    let relative = parent.strip_prefix(cache_root).map_err(|_| {
        AppError::trust_cache(format!(
            "offline cache path {} escapes cache root {}",
            path.display(),
            cache_root.display()
        ))
    })?;
    let root_metadata = fs::symlink_metadata(cache_root).map_err(|error| {
        AppError::trust_cache(format!(
            "offline cache root {} is unavailable: {error}",
            cache_root.display()
        ))
    })?;
    if !root_metadata.file_type().is_dir() {
        return Err(AppError::trust_cache(format!(
            "offline cache root {} is not a real directory",
            cache_root.display()
        )));
    }
    let mut current = cache_root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(AppError::trust_cache(format!(
                "offline cache path {} is not canonical",
                path.display()
            )));
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            AppError::trust_cache(format!(
                "offline cache directory {} is unavailable: {error}",
                current.display()
            ))
        })?;
        if !metadata.file_type().is_dir() {
            return Err(AppError::trust_cache(format!(
                "offline cache directory {} is not a real directory",
                current.display()
            )));
        }
    }
    Ok(())
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, AppError> {
    if crate::esp::cancelled() {
        return Err(AppError::Cancelled);
    }
    let parsed = Url::parse(url).map_err(|error| {
        AppError::trust_identity(format!("invalid release URL {url:?}: {error}"))
    })?;
    enforce_https(&parsed)?;
    let mut response = ureq::get(url)
        .header(
            "User-Agent",
            concat!("hopspot-flash/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| AppError::trust_artifact(format!("download failed for {url}: {error}")))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| AppError::trust_artifact(format!("could not read {url}: {error}")))?;
    if crate::esp::cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(bytes)
    }
}
