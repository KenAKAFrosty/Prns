use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use prns_flash_manifest::{
    board_catalog, pinned_key_id, sha256_hex, validate_uf2_artifact, verify_minisign,
    ReleaseTarget, ValidatedChannelDescriptor, ValidatedFlashManifest, PINNED_MINISIGN_PUBLIC_KEY,
};
use serde::Deserialize;

const MAX_PUBLIC_KEY_BYTES: u64 = 16 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 64 * 1024;
const MAX_VERSION_BYTES: u64 = 4 * 1024;
const MAX_MANIFEST_BYTES: u64 = 512 * 1024;
const MAX_CHANNEL_BYTES: u64 = 64 * 1024;
const MAX_BUILD_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CANDIDATE_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CANDIDATE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_CANDIDATE_ENTRIES: usize = 200_000;
const MAX_CANDIDATE_DEPTH: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildMetadata {
    schema: u8,
    source_commit: String,
    source_date_epoch: u64,
    built_at_utc: String,
    timestamp_source: String,
    host: BuildHost,
    expected_tools: ExpectedTools,
    tools: ResolvedTools,
    web_packages: WebPackages,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildHost {
    system: String,
    machine: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedTools {
    rustc: String,
    cargo: String,
    node: String,
    dioxus: String,
    wasm_bindgen: String,
    cargo_binstall: String,
    espup: String,
    esp_rustc: String,
    llvm_objcopy: String,
    xtensa_gcc: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedTools {
    rustc: String,
    cargo: String,
    node: String,
    npm: String,
    dioxus: String,
    wasm_bindgen: String,
    cargo_binstall: String,
    espup: String,
    esp_rustc: String,
    xtensa_gcc: String,
    llvm_objcopy: String,
    python: String,
    git: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebPackages {
    #[serde(rename = "esptool-js")]
    esptool_js: String,
    #[serde(rename = "spark-md5")]
    spark_md5: String,
    esbuild: String,
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

    validate_candidate(&root)
}

fn validate_candidate(root: &Path) -> Result<(), String> {
    let payload_files = walk_payload_files(root)?;

    let catalog = board_catalog().map_err(|error| error.to_string())?;
    let candidate_key = read_text_limited(&root.join("minisign.pub"), MAX_PUBLIC_KEY_BYTES)?;
    if candidate_key != PINNED_MINISIGN_PUBLIC_KEY {
        return Err("candidate Minisign public key differs from the repository pin".to_string());
    }
    let manifest_path = root.join("flash-manifest.json");
    let manifest_bytes = read_limited(&manifest_path, MAX_MANIFEST_BYTES)?;
    verify_file(&manifest_path, &manifest_bytes)?;
    let manifest = ValidatedFlashManifest::from_json(&manifest_bytes, &catalog)
        .map_err(|error| error.to_string())?;
    let actual_key_id = pinned_key_id()
        .ok_or_else(|| "repository-pinned Minisign key has no canonical key ID".to_string())?;
    if manifest.signing().key_id().as_str() != actual_key_id.to_ascii_uppercase() {
        return Err(format!(
            "manifest signing key ID {:?} differs from pinned key {actual_key_id}",
            manifest.signing().key_id().as_str()
        ));
    }
    verify_provenance(root, &manifest)?;

    for target in manifest.targets() {
        for part in target.parts() {
            let part_path = part.path().as_str();
            let path = safe_join(root, part_path)?;
            let bytes = read_limited(&path, part.size())?;
            if bytes.len() as u64 != part.size() || sha256_hex(&bytes) != part.sha256().as_str() {
                return Err(format!(
                    "{} does not match its signed size and SHA-256",
                    path.display()
                ));
            }
            let hosted_path = safe_join(
                &root
                    .join("website")
                    .join("releases")
                    .join(manifest.release().version().as_str()),
                part_path,
            )?;
            let hosted_bytes = read_limited(&hosted_path, part.size())?;
            if hosted_bytes.len() as u64 != part.size()
                || sha256_hex(&hosted_bytes) != part.sha256().as_str()
            {
                return Err(format!(
                    "{} does not match the signed hosted artifact",
                    hosted_path.display()
                ));
            }
        }
        if let ReleaseTarget::Uf2(target) = target {
            for variant in target.variants() {
                let path = safe_join(root, variant.part().path().as_str())?;
                let bytes = read_limited(&path, variant.part().size())?;
                validate_uf2_artifact(variant, &bytes).map_err(|error| {
                    format!(
                        "{} is not a valid signed UF2 variant: {error}",
                        path.display()
                    )
                })?;
            }
        }
    }

    let channel_name = match manifest.release().channel() {
        prns_flash_manifest::ReleaseChannel::Stable => "stable",
        prns_flash_manifest::ReleaseChannel::Preview => "preview",
    };
    let channel_path = root.join("channels").join(format!("{channel_name}.json"));
    let channel_bytes = read_limited(&channel_path, MAX_CHANNEL_BYTES)?;
    verify_file(&channel_path, &channel_bytes)?;
    let descriptor =
        ValidatedChannelDescriptor::from_json(&channel_bytes, manifest.release().channel())
            .map_err(|error| error.to_string())?;
    if descriptor.version() != manifest.release().version()
        || descriptor.manifest_sha256().as_str() != sha256_hex(&manifest_bytes)
    {
        return Err("signed channel descriptor disagrees with the manifest".to_string());
    }

    verify_sums(root, &payload_files)?;
    verify_website_copies(
        root,
        manifest.release().version().as_str(),
        channel_name,
        &manifest_bytes,
        &channel_bytes,
    )?;
    println!(
        "verified signed flasher candidate {} ({})",
        manifest.release().version(),
        channel_name
    );
    Ok(())
}

fn verify_provenance(root: &Path, manifest: &ValidatedFlashManifest) -> Result<(), String> {
    let version = read_text_limited(&root.join("VERSION"), MAX_VERSION_BYTES)?;
    if version.trim() != manifest.release().version().as_str() {
        return Err("candidate VERSION differs from the signed manifest".to_string());
    }
    let metadata_bytes = read_limited(
        &root.join("metadata").join("build.json"),
        MAX_BUILD_METADATA_BYTES,
    )?;
    validate_build_metadata(&metadata_bytes, manifest.release().commit())
}

fn validate_build_metadata(bytes: &[u8], expected_commit: &str) -> Result<(), String> {
    let metadata: BuildMetadata = serde_json::from_slice(bytes)
        .map_err(|error| format!("candidate build metadata is invalid: {error}"))?;
    if metadata.schema != 2 || metadata.source_commit != expected_commit {
        return Err("candidate build provenance differs from the signed manifest".to_string());
    }
    if metadata.source_date_epoch == 0
        || metadata.timestamp_source != "source_commit"
        || metadata.built_at_utc != utc_timestamp(metadata.source_date_epoch)?
    {
        return Err(
            "candidate build timestamp is not deterministically source-derived".to_string(),
        );
    }
    if metadata.host.system.trim().is_empty() || metadata.host.machine.trim().is_empty() {
        return Err("candidate build provenance is incomplete".to_string());
    }
    if metadata.expected_tools.rustc != "1.96.0"
        || metadata.expected_tools.cargo != "1.96.0"
        || metadata.expected_tools.node != "24.18.0"
        || metadata.expected_tools.dioxus != "0.7.5"
        || metadata.expected_tools.wasm_bindgen != "0.2.126"
        || metadata.expected_tools.cargo_binstall != "1.21.0"
        || metadata.expected_tools.espup != "0.17.1"
        || metadata.expected_tools.esp_rustc != "1.95.0"
        || metadata.expected_tools.llvm_objcopy != "rust-1.96.0-llvm-tools-preview"
        || metadata.expected_tools.xtensa_gcc != "esp-15.2.0_20250920-gcc-15.2.0"
    {
        return Err("candidate expected production tools drifted".to_string());
    }
    if !metadata.tools.rustc.starts_with("rustc 1.96.0 ")
        || !metadata.tools.cargo.starts_with("cargo 1.96.0 ")
        || metadata.tools.node != "v24.18.0"
        || !has_version_token(&metadata.tools.dioxus, "0.7.5")
        || metadata.tools.wasm_bindgen != "wasm-bindgen 0.2.126"
        || !has_version_token(&metadata.tools.cargo_binstall, "1.21.0")
        || !has_version_token(&metadata.tools.espup, "0.17.1")
        || !metadata.tools.esp_rustc.starts_with("rustc 1.95.0")
        || metadata.tools.xtensa_gcc
            != "xtensa-esp-elf-gcc (crosstool-NG esp-15.2.0_20250920) 15.2.0"
        || !metadata
            .tools
            .llvm_objcopy
            .to_ascii_lowercase()
            .contains("llvm")
    {
        return Err("candidate resolved production tools disagree with exact pins".to_string());
    }
    for (name, value) in [
        ("npm", metadata.tools.npm.as_str()),
        ("python", metadata.tools.python.as_str()),
        ("git", metadata.tools.git.as_str()),
    ] {
        if value.trim().is_empty() || value == "unavailable" {
            return Err(format!("candidate build provenance lacks {name}"));
        }
    }
    if metadata.web_packages.esptool_js != "0.6.0"
        || metadata.web_packages.spark_md5 != "3.0.2"
        || metadata.web_packages.esbuild != "0.28.1"
    {
        return Err("candidate exact web package pins drifted".to_string());
    }
    Ok(())
}

fn has_version_token(value: &str, expected: &str) -> bool {
    value
        .split_ascii_whitespace()
        .any(|token| token == expected)
}

fn utc_timestamp(epoch: u64) -> Result<String, String> {
    let seconds_per_day = 86_400_u64;
    let days = i64::try_from(epoch / seconds_per_day)
        .map_err(|_| "candidate source epoch is outside the supported range".to_string())?;
    let seconds = epoch % seconds_per_day;
    let (year, month, day) = civil_from_days(days)?;
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+00:00"
    ))
}

fn civil_from_days(days_since_epoch: i64) -> Result<(i64, i64, i64), String> {
    let adjusted = days_since_epoch
        .checked_add(719_468)
        .ok_or_else(|| "candidate source epoch is outside the supported range".to_string())?;
    let era = adjusted.div_euclid(146_097);
    let day_of_era = adjusted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year_adjustment = if month <= 2 { 1 } else { 0 };
    Ok((year + year_adjustment, month, day))
}

fn verify_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let signature_path = PathBuf::from(format!("{}.minisig", path.display()));
    let signature = read_text_limited(&signature_path, MAX_SIGNATURE_BYTES)?;
    verify_minisign(bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn verify_sums(root: &Path, actual_payloads: &BTreeSet<String>) -> Result<(), String> {
    let sums_path = root.join("SHA256SUMS.txt");
    let bytes = read_limited(&sums_path, MAX_CHECKSUM_BYTES)?;
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
        let actual = digest_file(&path, MAX_CANDIDATE_FILE_BYTES)?;
        if actual != digest {
            return Err(format!("SHA-256 mismatch for {relative}"));
        }
    }

    let expected = listed.keys().cloned().collect::<BTreeSet<_>>();
    if actual_payloads != &expected {
        let missing = actual_payloads
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>();
        let stale = expected
            .difference(actual_payloads)
            .cloned()
            .collect::<Vec<_>>();
        return Err(format!(
            "SHA256SUMS coverage differs; unlisted={missing:?}, missing-files={stale:?}"
        ));
    }
    Ok(())
}

fn walk_payload_files(root: &Path) -> Result<BTreeSet<String>, String> {
    fn visit(
        root: &Path,
        directory: &Path,
        output: &mut BTreeSet<String>,
        depth: usize,
        entries_seen: &mut usize,
        bytes_seen: &mut u64,
    ) -> Result<(), String> {
        if depth > MAX_CANDIDATE_DEPTH {
            return Err(format!(
                "candidate directory exceeds the safe traversal depth at {}",
                directory.display()
            ));
        }
        for entry in fs::read_dir(directory)
            .map_err(|error| format!("could not inspect {}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            *entries_seen = entries_seen.saturating_add(1);
            if *entries_seen > MAX_CANDIDATE_ENTRIES {
                return Err(format!(
                    "candidate directory exceeds the {MAX_CANDIDATE_ENTRIES}-entry safety limit"
                ));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "candidate cannot contain symlink {}",
                    path.display()
                ));
            }
            if metadata.file_type().is_dir() {
                visit(root, &path, output, depth + 1, entries_seen, bytes_seen)?;
            } else if metadata.file_type().is_file() {
                if metadata.len() > MAX_CANDIDATE_FILE_BYTES {
                    return Err(format!(
                        "candidate file {} exceeds the {MAX_CANDIDATE_FILE_BYTES}-byte safety limit",
                        path.display()
                    ));
                }
                *bytes_seen = bytes_seen.checked_add(metadata.len()).ok_or_else(|| {
                    format!(
                        "candidate files exceed the {MAX_CANDIDATE_BYTES}-byte aggregate safety limit"
                    )
                })?;
                if *bytes_seen > MAX_CANDIDATE_BYTES {
                    return Err(format!(
                        "candidate files exceed the {MAX_CANDIDATE_BYTES}-byte aggregate safety limit"
                    ));
                }
                let relative = path
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_str()
                    .ok_or_else(|| format!("candidate path {} is not valid UTF-8", path.display()))?
                    .replace('\\', "/");
                if relative != "SHA256SUMS.txt"
                    && relative != "acceptance.json"
                    && !relative.ends_with(".minisig")
                {
                    output.insert(relative);
                }
            } else {
                return Err(format!(
                    "candidate contains unsupported entry {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("could not inspect {}: {error}", root.display()))?;
    if !root_metadata.file_type().is_dir() {
        return Err(format!("{} is not a candidate directory", root.display()));
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
    )?;
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
    if read_limited(&immutable_manifest, manifest.len() as u64)? != manifest
        || read_limited(&hosted_channel, descriptor.len() as u64)? != descriptor
    {
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

fn open_limited(path: &Path, limit: u64) -> Result<File, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "candidate path {} is not a regular file",
            path.display()
        ));
    }
    if metadata.len() > limit {
        return Err(format!(
            "candidate file {} exceeds the {limit}-byte safety limit",
            path.display()
        ));
    }
    let file =
        File::open(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let opened_metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect open file {}: {error}", path.display()))?;
    if !opened_metadata.file_type().is_file() {
        return Err(format!(
            "candidate path {} is not a regular file",
            path.display()
        ));
    }
    if opened_metadata.len() > limit {
        return Err(format!(
            "candidate file {} exceeds the {limit}-byte safety limit",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.dev() != opened_metadata.dev() || metadata.ino() != opened_metadata.ino() {
            return Err(format!(
                "candidate file {} changed while it was opened",
                path.display()
            ));
        }
    }
    Ok(file)
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = open_limited(path, limit)?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "candidate file {} exceeds the {limit}-byte safety limit",
            path.display()
        ));
    }
    Ok(bytes)
}

fn read_text_limited(path: &Path, limit: u64) -> Result<String, String> {
    String::from_utf8(read_limited(path, limit)?)
        .map_err(|error| format!("{} is not UTF-8: {error}", path.display()))
}

fn digest_file(path: &Path, limit: u64) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let file = open_limited(path, limit)?;
    let mut file = file.take(limit.saturating_add(1));
    let mut digest = Sha256::new();
    let mut bytes_read = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        bytes_read += count as u64;
        if bytes_read > limit {
            return Err(format!(
                "candidate file {} exceeds the {limit}-byte safety limit",
                path.display()
            ));
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::walk_payload_files;
    use super::{
        read_limited, utc_timestamp, validate_build_metadata, validate_candidate,
        MAX_CANDIDATE_FILE_BYTES,
    };
    use serde_json::{json, Value};
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "vfc-{}-{:x}-{}",
                std::process::id(),
                nonce & u32::MAX as u128,
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

    fn metadata() -> Value {
        json!({
            "schema": 2,
            "source_commit": COMMIT,
            "source_date_epoch": 1_774_358_400_u64,
            "built_at_utc": "2026-03-24T13:20:00+00:00",
            "timestamp_source": "source_commit",
            "host": {"system": "Linux", "machine": "x86_64"},
            "expected_tools": {
                "rustc": "1.96.0",
                "cargo": "1.96.0",
                "node": "24.18.0",
                "dioxus": "0.7.5",
                "wasm_bindgen": "0.2.126",
                "cargo_binstall": "1.21.0",
                "espup": "0.17.1",
                "esp_rustc": "1.95.0",
                "llvm_objcopy": "rust-1.96.0-llvm-tools-preview",
                "xtensa_gcc": "esp-15.2.0_20250920-gcc-15.2.0"
            },
            "tools": {
                "rustc": "rustc 1.96.0 (fixture)",
                "cargo": "cargo 1.96.0 (fixture)",
                "node": "v24.18.0",
                "npm": "11.0.0",
                "dioxus": "dioxus 0.7.5",
                "wasm_bindgen": "wasm-bindgen 0.2.126",
                "cargo_binstall": "cargo-binstall 1.21.0",
                "espup": "espup 0.17.1",
                "esp_rustc": "rustc 1.95.0-nightly (fixture)",
                "xtensa_gcc": "xtensa-esp-elf-gcc (crosstool-NG esp-15.2.0_20250920) 15.2.0",
                "llvm_objcopy": "llvm-objcopy version 20.1.8",
                "python": "Python 3.13.0",
                "git": "git version 2.50.0"
            },
            "web_packages": {
                "esptool-js": "0.6.0",
                "spark-md5": "3.0.2",
                "esbuild": "0.28.1"
            }
        })
    }

    fn encoded(value: &Value) -> Vec<u8> {
        value.to_string().into_bytes()
    }

    #[test]
    fn accepts_exact_schema_three_release_metadata() {
        assert_eq!(
            utc_timestamp(1_774_358_400),
            Ok("2026-03-24T13:20:00+00:00".to_string())
        );
        assert!(validate_build_metadata(&encoded(&metadata()), COMMIT).is_ok());
    }

    #[test]
    fn rejects_schema_timestamp_and_unknown_field_tampering() {
        let mut wrong_schema = metadata();
        wrong_schema["schema"] = json!(1);
        assert!(validate_build_metadata(&encoded(&wrong_schema), COMMIT).is_err());

        let mut wrong_timestamp = metadata();
        wrong_timestamp["built_at_utc"] = json!("2026-03-24T13:20:01+00:00");
        assert!(validate_build_metadata(&encoded(&wrong_timestamp), COMMIT).is_err());

        let mut unknown = metadata();
        unknown["unreviewed"] = json!(true);
        assert!(validate_build_metadata(&encoded(&unknown), COMMIT).is_err());
    }

    #[test]
    fn rejects_production_tool_and_web_package_drift() {
        let mut wrong_node = metadata();
        wrong_node["tools"]["node"] = json!("v24.18.1");
        assert!(validate_build_metadata(&encoded(&wrong_node), COMMIT).is_err());

        let mut wrong_wasm_bindgen = metadata();
        wrong_wasm_bindgen["tools"]["wasm_bindgen"] = json!("wasm-bindgen 0.2.127");
        assert!(validate_build_metadata(&encoded(&wrong_wasm_bindgen), COMMIT).is_err());

        let mut wrong_expected_rust = metadata();
        wrong_expected_rust["expected_tools"]["rustc"] = json!("stable");
        assert!(validate_build_metadata(&encoded(&wrong_expected_rust), COMMIT).is_err());

        let mut wrong_expected_wasm_bindgen = metadata();
        wrong_expected_wasm_bindgen["expected_tools"]["wasm_bindgen"] = json!("latest");
        assert!(validate_build_metadata(&encoded(&wrong_expected_wasm_bindgen), COMMIT).is_err());

        let mut wrong_esptool = metadata();
        wrong_esptool["web_packages"]["esptool-js"] = json!("0.6.1");
        assert!(validate_build_metadata(&encoded(&wrong_esptool), COMMIT).is_err());
    }

    #[test]
    fn bounded_read_rejects_oversized_files_before_allocating_their_contents() {
        let root = TestDirectory::new();
        let path = root.path().join("oversized.bin");
        let file = File::create(&path).expect("create sparse payload");
        file.set_len(9).expect("size sparse payload");

        let error = read_limited(&path, 8).expect_err("oversized read must fail");
        assert!(error.contains("exceeds the 8-byte safety limit"));
    }

    #[test]
    fn candidate_inventory_rejects_oversized_files_before_required_files_are_read() {
        let root = TestDirectory::new();
        let path = root.path().join("oversized.bin");
        let file = File::create(&path).expect("create sparse payload");
        file.set_len(MAX_CANDIDATE_FILE_BYTES + 1)
            .expect("size sparse payload");

        let error = validate_candidate(root.path()).expect_err("oversized candidate must fail");
        assert!(error.contains("oversized.bin"));
        assert!(error.contains("safety limit"));
        assert!(!error.contains("minisign.pub"));
    }

    #[cfg(unix)]
    #[test]
    fn payload_walk_rejects_file_and_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let file_root = TestDirectory::new();
        fs::write(file_root.path().join("payload.bin"), b"payload").expect("write payload");
        symlink(
            file_root.path().join("payload.bin"),
            file_root.path().join("payload-link.bin"),
        )
        .expect("create file symlink");
        assert!(walk_payload_files(file_root.path()).is_err());

        let directory_root = TestDirectory::new();
        let real_directory = directory_root.path().join("real");
        fs::create_dir(&real_directory).expect("create payload directory");
        fs::write(real_directory.join("payload.bin"), b"payload").expect("write payload");
        symlink(
            &real_directory,
            directory_root.path().join("directory-link"),
        )
        .expect("create directory symlink");
        assert!(walk_payload_files(directory_root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn candidate_inventory_rejects_symlinks_before_required_files_are_read() {
        use std::os::unix::fs::symlink;

        let root = TestDirectory::new();
        let outside = TestDirectory::new();
        fs::write(outside.path().join("payload.bin"), b"payload").expect("write payload");
        fs::create_dir(root.path().join("nested")).expect("create nested directory");
        symlink(outside.path(), root.path().join("nested").join("escape"))
            .expect("create directory symlink");

        let error = validate_candidate(root.path()).expect_err("symlink candidate must fail");
        assert!(error.contains("candidate cannot contain symlink"));
        assert!(error.contains("escape"));
        assert!(!error.contains("minisign.pub"));
    }

    #[cfg(unix)]
    #[test]
    fn candidate_inventory_rejects_unsupported_entries_before_required_files_are_read() {
        use std::process::Command;

        let root = TestDirectory::new();
        let fifo_path = root.path().join("candidate.fifo");
        let status = Command::new("mkfifo")
            .arg(&fifo_path)
            .status()
            .expect("run mkfifo");
        assert!(status.success());

        let error =
            validate_candidate(root.path()).expect_err("unsupported candidate entry must fail");
        assert!(error.contains("candidate contains unsupported entry"));
        assert!(error.contains("candidate.fifo"));
        assert!(!error.contains("minisign.pub"));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_read_rejects_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let root = TestDirectory::new();
        let target = root.path().join("target.bin");
        let link = root.path().join("link.bin");
        fs::write(&target, b"payload").expect("write target");
        symlink(&target, &link).expect("create symlink");

        let error = read_limited(&link, 1024).expect_err("symlink read must fail");
        assert!(error.contains("is not a regular file"));
    }
}
