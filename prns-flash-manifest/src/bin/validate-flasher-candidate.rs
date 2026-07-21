use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use prns_flash_manifest::{
    board_catalog, pinned_key_id, sha256_hex, verify_minisign, ChannelDescriptor, FlashManifest,
    PINNED_MINISIGN_PUBLIC_KEY,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildMetadata {
    schema: u8,
    source_commit: String,
    built_at_utc: String,
    host: BuildHost,
    tools: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildHost {
    system: String,
    machine: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("candidate validation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let root = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "usage: validate-flasher-candidate CANDIDATE_DIR".to_string())?;
    if arguments.next().is_some() {
        return Err("usage: validate-flasher-candidate CANDIDATE_DIR".to_string());
    }
    if !root.is_dir() {
        return Err(format!("{} is not a candidate directory", root.display()));
    }

    let catalog = board_catalog().map_err(|error| error.to_string())?;
    let candidate_key = fs::read_to_string(root.join("minisign.pub"))
        .map_err(|error| format!("could not read candidate minisign.pub: {error}"))?;
    if candidate_key != PINNED_MINISIGN_PUBLIC_KEY {
        return Err("candidate Minisign public key differs from the repository pin".to_string());
    }
    let manifest_path = root.join("flash-manifest.json");
    let manifest_bytes = read(&manifest_path)?;
    verify_file(&manifest_path, &manifest_bytes)?;
    let manifest =
        FlashManifest::from_json(&manifest_bytes, &catalog).map_err(|error| error.to_string())?;
    let actual_key_id = pinned_key_id()
        .ok_or_else(|| "repository-pinned Minisign key has no canonical key ID".to_string())?;
    if !manifest.signing.key_id.eq_ignore_ascii_case(&actual_key_id) {
        return Err(format!(
            "manifest signing key ID {:?} differs from pinned key {actual_key_id}",
            manifest.signing.key_id
        ));
    }
    verify_provenance(&root, &manifest)?;

    for target in &manifest.targets {
        for part in &target.parts {
            let path = safe_join(&root, &part.path)?;
            let bytes = read(&path)?;
            if bytes.len() as u64 != part.size || sha256_hex(&bytes) != part.sha256 {
                return Err(format!(
                    "{} does not match its signed size and SHA-256",
                    path.display()
                ));
            }
            let hosted_path = safe_join(
                &root
                    .join("website")
                    .join("releases")
                    .join(&manifest.release.version),
                &part.path,
            )?;
            let hosted_bytes = read(&hosted_path)?;
            if hosted_bytes.len() as u64 != part.size || sha256_hex(&hosted_bytes) != part.sha256 {
                return Err(format!(
                    "{} does not match the signed hosted artifact",
                    hosted_path.display()
                ));
            }
        }
    }

    let channel_name = match manifest.release.channel {
        prns_flash_manifest::ReleaseChannel::Stable => "stable",
        prns_flash_manifest::ReleaseChannel::Preview => "preview",
    };
    let channel_path = root.join("channels").join(format!("{channel_name}.json"));
    let channel_bytes = read(&channel_path)?;
    verify_file(&channel_path, &channel_bytes)?;
    let descriptor = ChannelDescriptor::from_json(&channel_bytes, manifest.release.channel)
        .map_err(|error| error.to_string())?;
    if descriptor.version != manifest.release.version
        || descriptor.manifest_sha256 != sha256_hex(&manifest_bytes)
    {
        return Err("signed channel descriptor disagrees with the manifest".to_string());
    }

    verify_sums(&root)?;
    verify_website_copies(
        &root,
        &manifest.release.version,
        channel_name,
        &manifest_bytes,
        &channel_bytes,
    )?;
    println!(
        "verified signed flasher candidate {} ({})",
        manifest.release.version, channel_name
    );
    Ok(())
}

fn verify_provenance(root: &Path, manifest: &FlashManifest) -> Result<(), String> {
    let version = fs::read_to_string(root.join("VERSION"))
        .map_err(|error| format!("could not read candidate VERSION: {error}"))?;
    if version.trim() != manifest.release.version {
        return Err("candidate VERSION differs from the signed manifest".to_string());
    }
    let metadata_bytes = read(&root.join("metadata").join("build.json"))?;
    let metadata: BuildMetadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|error| format!("candidate build metadata is invalid: {error}"))?;
    if metadata.schema != 1 || metadata.source_commit != manifest.release.commit {
        return Err("candidate build provenance differs from the signed manifest".to_string());
    }
    if metadata.built_at_utc.trim().is_empty()
        || metadata.host.system.trim().is_empty()
        || metadata.host.machine.trim().is_empty()
    {
        return Err("candidate build provenance is incomplete".to_string());
    }
    for required in ["rustc", "cargo", "node", "npm", "dioxus", "git"] {
        let value = metadata
            .tools
            .get(required)
            .map(String::as_str)
            .unwrap_or("");
        if value.trim().is_empty() || value == "unavailable" {
            return Err(format!("candidate build provenance lacks {required}"));
        }
    }
    if !metadata
        .tools
        .get("dioxus")
        .is_some_and(|value| value.contains("0.7.5"))
    {
        return Err("candidate was not built with dioxus-cli 0.7.5".to_string());
    }
    Ok(())
}

fn verify_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let signature_path = PathBuf::from(format!("{}.minisig", path.display()));
    let signature = fs::read_to_string(&signature_path)
        .map_err(|error| format!("could not read {}: {error}", signature_path.display()))?;
    verify_minisign(bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn verify_sums(root: &Path) -> Result<(), String> {
    let sums_path = root.join("SHA256SUMS.txt");
    let bytes = read(&sums_path)?;
    verify_file(&sums_path, &bytes)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("SHA256SUMS.txt is not UTF-8: {error}"))?;
    let mut listed = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let (digest, relative) = line
            .split_once("  ")
            .ok_or_else(|| format!("invalid SHA256SUMS line {}", index + 1))?;
        validate_digest(digest)?;
        let path = safe_join(root, relative)?;
        if listed
            .insert(relative.to_string(), digest.to_string())
            .is_some()
        {
            return Err(format!("duplicate SHA256SUMS path {relative:?}"));
        }
        let actual = sha256_hex(&read(&path)?);
        if actual != digest {
            return Err(format!("SHA-256 mismatch for {relative}"));
        }
    }

    let actual = walk_payload_files(root)?;
    let expected = listed.keys().cloned().collect::<BTreeSet<_>>();
    if actual != expected {
        let missing = actual.difference(&expected).cloned().collect::<Vec<_>>();
        let stale = expected.difference(&actual).cloned().collect::<Vec<_>>();
        return Err(format!(
            "SHA256SUMS coverage differs; unlisted={missing:?}, missing-files={stale:?}"
        ));
    }
    Ok(())
}

fn walk_payload_files(root: &Path) -> Result<BTreeSet<String>, String> {
    fn visit(root: &Path, directory: &Path, output: &mut BTreeSet<String>) -> Result<(), String> {
        for entry in fs::read_dir(directory)
            .map_err(|error| format!("could not inspect {}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let metadata = entry
                .metadata()
                .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "candidate cannot contain symlink {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                visit(root, &path, output)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                if relative != "SHA256SUMS.txt"
                    && relative != "acceptance.json"
                    && !relative.ends_with(".minisig")
                {
                    output.insert(relative);
                }
            }
        }
        Ok(())
    }

    let mut output = BTreeSet::new();
    visit(root, root, &mut output)?;
    Ok(output)
}

fn verify_website_copies(
    root: &Path,
    version: &str,
    channel: &str,
    manifest: &[u8],
    descriptor: &[u8],
) -> Result<(), String> {
    let immutable_manifest = root
        .join("website")
        .join("releases")
        .join(version)
        .join("flash-manifest.json");
    let hosted_channel = root
        .join("website")
        .join("releases")
        .join("channels")
        .join(format!("{channel}.json"));
    if read(&immutable_manifest)? != manifest || read(&hosted_channel)? != descriptor {
        return Err("website release documents differ from the signed candidate".to_string());
    }
    verify_file(&immutable_manifest, manifest)?;
    verify_file(&hosted_channel, descriptor)
}

fn safe_join(root: &Path, relative: impl AsRef<Path>) -> Result<PathBuf, String> {
    let relative = relative.as_ref();
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("unsafe candidate path {}", relative.display()));
    }
    Ok(root.join(relative))
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

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))
}
