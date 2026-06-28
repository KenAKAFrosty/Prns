use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO_VERSION_PATH: &str = "../../VERSION";
const WRITE_PUBLIC_ASSETS_ENV: &str = "PRNS_WRITE_PUBLIC_ASSETS";
const EMBEDDED_SITE_ENV: &str = "PRNS_EMBEDDED_SITE";

fn main() {
    let version = build_version();
    let commit = env::var("PRNS_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let short = env::var("PRNS_BUILD_COMMIT_SHORT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| short_commit(&commit));
    let write_public_assets = should_write_public_assets();

    generate_board_images();
    generate_flash_manifest(&version, write_public_assets);
    if write_public_assets {
        generate_source_zip(&version, &commit);
    }

    println!("cargo:rustc-env=PRNS_BUILD_VERSION={version}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PRNS_GIT_COMMIT_SHORT={short}");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT_SHORT");
    println!("cargo:rerun-if-env-changed={EMBEDDED_SITE_ENV}");
    println!("cargo:rerun-if-env-changed={WRITE_PUBLIC_ASSETS_ENV}");

    if let Some(head) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
        if let Ok(head_contents) = fs::read_to_string(&head) {
            if let Some(reference) = head_contents.trim().strip_prefix("ref: ") {
                if let Some(path) = git_output(&["rev-parse", "--git-path", reference]) {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
    }
}

fn should_write_public_assets() -> bool {
    env_flag(WRITE_PUBLIC_ASSETS_ENV) || env_flag(EMBEDDED_SITE_ENV)
}

fn env_flag(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0")
}

fn build_version() -> String {
    env::var("PRNS_BUILD_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(read_repo_version)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn read_repo_version() -> Option<String> {
    let path = PathBuf::from(REPO_VERSION_PATH);
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Default)]
struct FlashManifestRecord {
    slug: String,
    state: String,
    transport: String,
    format: String,
    release_channel: String,
    version: String,
    artifact_path: String,
    artifact_sha256: String,
    artifact_size: String,
    local_command: String,
    browser_support: String,
    embedded_policy: String,
    summary: String,
    steps: Vec<String>,
}

fn generate_flash_manifest(build_version: &str, write_public_assets: bool) {
    let source_path = PathBuf::from("src")
        .join("assets")
        .join("flash")
        .join("manifest.txt");
    println!("cargo:rerun-if-changed={}", source_path.display());

    let source = fs::read_to_string(&source_path).unwrap_or_else(|err| {
        panic!(
            "failed to read flash manifest source {}: {err}",
            source_path.display()
        )
    });
    let mut records = parse_flash_manifest(&source, &source_path);
    apply_flash_release_version(&mut records, build_version);
    apply_flash_artifact_metadata(&mut records);

    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
    fs::write(
        PathBuf::from(out_dir).join("flash_manifest.rs"),
        render_flash_manifest_rs(&records),
    )
    .expect("failed to write generated flash manifest module");

    if write_public_assets {
        let public_path = PathBuf::from("public").join("flash-manifest.json");
        if let Some(parent) = public_path.parent() {
            fs::create_dir_all(parent).expect("failed to create public flash manifest directory");
        }
        write_if_changed(&public_path, &render_flash_manifest_json(&records));
    }
}

fn apply_flash_release_version(records: &mut [FlashManifestRecord], build_version: &str) {
    println!("cargo:rerun-if-env-changed=PRNS_FLASH_VERSION");

    let version = env::var("PRNS_FLASH_VERSION")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| build_version.to_string());

    for record in records {
        if record.version == "next" {
            record.version = version.clone();
        }
    }
}

fn apply_flash_artifact_metadata(records: &mut [FlashManifestRecord]) {
    println!("cargo:rerun-if-env-changed=PRNS_FLASH_ARTIFACT_ROOT");

    let root = env::var_os("PRNS_FLASH_ARTIFACT_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("public"));

    for record in records {
        let relative_path = record
            .artifact_path
            .strip_prefix('/')
            .unwrap_or(&record.artifact_path);
        let artifact_path = root.join(relative_path);
        println!("cargo:rerun-if-changed={}", artifact_path.display());

        if artifact_path.is_file() {
            let size = fs::metadata(&artifact_path)
                .unwrap_or_else(|err| {
                    panic!(
                        "failed to inspect flash artifact {}: {err}",
                        artifact_path.display()
                    )
                })
                .len();
            record.state = "published".to_string();
            record.artifact_size = size.to_string();
            record.artifact_sha256 = sha256_file(&artifact_path);
        }
    }
}

fn generate_source_zip(build_version: &str, build_commit: &str) {
    println!("cargo:rerun-if-env-changed=PRNS_SOURCE_ARCHIVE_REF");

    let archive_ref = env::var("PRNS_SOURCE_ARCHIVE_REF")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if build_commit == "unknown" {
                "HEAD".to_string()
            } else {
                build_commit.to_string()
            }
        });
    let output = PathBuf::from("public").join("source.zip");
    let checksum = PathBuf::from("public").join("source.zip.sha256");
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("failed to create public source archive directory");
    }

    let temp = output.with_extension("zip.tmp");
    let temp_for_git = env::current_dir()
        .unwrap_or_else(|err| panic!("failed to read current directory: {err}"))
        .join(&temp);
    let _ = fs::remove_file(&temp);
    let repo_root =
        git_output(&["rev-parse", "--show-toplevel"]).unwrap_or_else(|| ".".to_string());
    let prefix = format!("Prns-{}/", archive_version(build_version));
    let status = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .arg("archive")
        .arg("--format=zip")
        .arg(format!("--prefix={prefix}"))
        .arg("-o")
        .arg(&temp_for_git)
        .arg(&archive_ref)
        .status()
        .unwrap_or_else(|err| {
            panic!("failed to run git archive for source ZIP from {archive_ref}: {err}")
        });
    if !status.success() {
        panic!("git archive failed for source ZIP from {archive_ref} with status {status}");
    }

    replace_if_changed(&output, &temp);
    let hash = sha256_file(&output);
    write_if_changed(&checksum, &format!("{hash}  source.zip\n"));
}

fn archive_version(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        .collect();
    if sanitized.is_empty() {
        "source".to_string()
    } else {
        sanitized
    }
}

fn sha256_file(path: &Path) -> String {
    if let Some(hash) = sha256_with("shasum", path) {
        return hash;
    }
    if let Some(hash) = sha256_with("sha256sum", path) {
        return hash;
    }
    panic!(
        "failed to compute sha256 for {}; install shasum or sha256sum",
        path.display()
    );
}

fn sha256_with(program: &str, path: &Path) -> Option<String> {
    let mut command = Command::new(program);
    if program == "shasum" {
        command.arg("-a").arg("256");
    }
    let output = command.arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|stdout| stdout.split_whitespace().next().map(str::to_string))
}

fn parse_flash_manifest(source: &str, source_path: &PathBuf) -> Vec<FlashManifestRecord> {
    let mut records = Vec::new();
    let mut current = None;

    for (line_index, raw_line) in source.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line == "[[board]]" {
            if let Some(record) = current.take() {
                records.push(validate_flash_record(record, source_path, line_index + 1));
            }
            current = Some(FlashManifestRecord::default());
            continue;
        }

        let (key, value) = line.split_once('=').unwrap_or_else(|| {
            panic!(
                "{}:{}: expected key = value",
                source_path.display(),
                line_index + 1
            )
        });
        let record = current.as_mut().unwrap_or_else(|| {
            panic!(
                "{}:{}: field appears before [[board]]",
                source_path.display(),
                line_index + 1
            )
        });
        let value = parse_manifest_value(value.trim(), source_path, line_index + 1);

        match key.trim() {
            "slug" => record.slug = value,
            "state" => record.state = value,
            "transport" => record.transport = value,
            "format" => record.format = value,
            "release_channel" => record.release_channel = value,
            "version" => record.version = value,
            "artifact_path" => record.artifact_path = value,
            "artifact_sha256" => record.artifact_sha256 = value,
            "artifact_size" => record.artifact_size = value,
            "local_command" => record.local_command = value,
            "browser_support" => record.browser_support = value,
            "embedded_policy" => record.embedded_policy = value,
            "summary" => record.summary = value,
            "step" => record.steps.push(value),
            other => panic!(
                "{}:{}: unknown flash manifest field {other:?}",
                source_path.display(),
                line_index + 1
            ),
        }
    }

    if let Some(record) = current {
        records.push(validate_flash_record(
            record,
            source_path,
            source.lines().count(),
        ));
    }

    if records.is_empty() {
        panic!(
            "{}: flash manifest has no board entries",
            source_path.display()
        );
    }
    records
}

fn parse_manifest_value(value: &str, source_path: &PathBuf, line: usize) -> String {
    if let Some(unquoted) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        unquoted.to_owned()
    } else {
        panic!(
            "{}:{line}: manifest values must be quoted",
            source_path.display()
        );
    }
}

fn validate_flash_record(
    record: FlashManifestRecord,
    source_path: &PathBuf,
    line: usize,
) -> FlashManifestRecord {
    for (key, value) in [
        ("slug", &record.slug),
        ("state", &record.state),
        ("transport", &record.transport),
        ("format", &record.format),
        ("release_channel", &record.release_channel),
        ("version", &record.version),
        ("artifact_path", &record.artifact_path),
        ("local_command", &record.local_command),
        ("browser_support", &record.browser_support),
        ("embedded_policy", &record.embedded_policy),
        ("summary", &record.summary),
    ] {
        if value.is_empty() {
            panic!(
                "{}:{line}: flash manifest entry for {:?} is missing {key}",
                source_path.display(),
                record.slug
            );
        }
    }
    if record.steps.is_empty() {
        panic!(
            "{}:{line}: flash manifest entry for {:?} needs at least one step",
            source_path.display(),
            record.slug
        );
    }
    if !record.artifact_size.is_empty() {
        record.artifact_size.parse::<u64>().unwrap_or_else(|err| {
            panic!(
                "{}:{line}: artifact_size for {:?} must be a u64: {err}",
                source_path.display(),
                record.slug
            )
        });
    }
    record
}

fn render_flash_manifest_rs(records: &[FlashManifestRecord]) -> String {
    let mut generated = String::from(
        "// @generated by build.rs; do not edit.\nuse super::{EmbeddedPolicy, FlashArtifactFormat, FlashArtifactRecord, FlashArtifactState, FlashTransport};\n\npub const FLASH_ARTIFACTS: &[FlashArtifactRecord] = &[\n",
    );

    for record in records {
        generated.push_str("    FlashArtifactRecord {\n");
        generated.push_str(&format!(
            "        board_slug: {},\n",
            rust_string(&record.slug)
        ));
        generated.push_str(&format!(
            "        state: FlashArtifactState::{},\n",
            state_variant(&record.state)
        ));
        generated.push_str(&format!(
            "        transport: FlashTransport::{},\n",
            transport_variant(&record.transport)
        ));
        generated.push_str(&format!(
            "        format: FlashArtifactFormat::{},\n",
            format_variant(&record.format)
        ));
        generated.push_str(&format!(
            "        release_channel: {},\n",
            rust_string(&record.release_channel)
        ));
        generated.push_str(&format!(
            "        version: {},\n",
            rust_string(&record.version)
        ));
        generated.push_str(&format!(
            "        artifact_path: {},\n",
            rust_option_string(&record.artifact_path)
        ));
        generated.push_str(&format!(
            "        artifact_sha256: {},\n",
            rust_option_string(&record.artifact_sha256)
        ));
        generated.push_str(&format!(
            "        artifact_size: {},\n",
            rust_option_u64(&record.artifact_size)
        ));
        generated.push_str(&format!(
            "        local_command: {},\n",
            rust_string(&record.local_command)
        ));
        generated.push_str(&format!(
            "        browser_support: {},\n",
            rust_string(&record.browser_support)
        ));
        generated.push_str(&format!(
            "        embedded_policy: EmbeddedPolicy::{},\n",
            embedded_policy_variant(&record.embedded_policy)
        ));
        generated.push_str(&format!(
            "        summary: {},\n",
            rust_string(&record.summary)
        ));
        generated.push_str("        steps: &[\n");
        for step in &record.steps {
            generated.push_str(&format!("            {},\n", rust_string(step)));
        }
        generated.push_str("        ],\n");
        generated.push_str("    },\n");
    }

    generated.push_str("];\n");
    generated
}

fn render_flash_manifest_json(records: &[FlashManifestRecord]) -> String {
    let mut json = String::from(
        "{\n  \"schema\": 1,\n  \"generated_from\": \"src/assets/flash/manifest.txt\",\n  \"targets\": [\n",
    );

    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            json.push_str(",\n");
        }
        json.push_str("    {\n");
        json.push_str(&format!(
            "      \"board_slug\": {},\n",
            json_string(&record.slug)
        ));
        json.push_str(&format!(
            "      \"state\": {},\n",
            json_string(&record.state)
        ));
        json.push_str(&format!(
            "      \"transport\": {},\n",
            json_string(&record.transport)
        ));
        json.push_str(&format!(
            "      \"format\": {},\n",
            json_string(&record.format)
        ));
        json.push_str(&format!(
            "      \"release_channel\": {},\n",
            json_string(&record.release_channel)
        ));
        json.push_str(&format!(
            "      \"version\": {},\n",
            json_string(&record.version)
        ));
        json.push_str(&format!(
            "      \"artifact_path\": {},\n",
            json_option_string(&record.artifact_path)
        ));
        json.push_str(&format!(
            "      \"artifact_sha256\": {},\n",
            json_option_string(&record.artifact_sha256)
        ));
        json.push_str(&format!(
            "      \"artifact_size\": {},\n",
            json_option_u64(&record.artifact_size)
        ));
        json.push_str(&format!(
            "      \"local_command\": {},\n",
            json_string(&record.local_command)
        ));
        json.push_str(&format!(
            "      \"browser_support\": {},\n",
            json_string(&record.browser_support)
        ));
        json.push_str(&format!(
            "      \"embedded_policy\": {},\n",
            json_string(&record.embedded_policy)
        ));
        json.push_str(&format!(
            "      \"summary\": {},\n",
            json_string(&record.summary)
        ));
        json.push_str("      \"steps\": [\n");
        for (step_index, step) in record.steps.iter().enumerate() {
            let suffix = if step_index + 1 == record.steps.len() {
                "\n"
            } else {
                ",\n"
            };
            json.push_str(&format!("        {}{}", json_string(step), suffix));
        }
        json.push_str("      ]\n");
        json.push_str("    }");
    }

    json.push_str("\n  ]\n}\n");
    json
}

fn write_if_changed(path: &PathBuf, contents: &str) {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return;
    }
    fs::write(path, contents).unwrap_or_else(|err| {
        panic!("failed to write {}: {err}", path.display());
    });
}

fn replace_if_changed(path: &Path, temp: &Path) {
    let same = fs::read(path)
        .ok()
        .zip(fs::read(temp).ok())
        .is_some_and(|(current, next)| current == next);
    if same {
        let _ = fs::remove_file(temp);
        return;
    }
    fs::rename(temp, path).unwrap_or_else(|err| {
        panic!(
            "failed to replace {} with {}: {err}",
            path.display(),
            temp.display()
        );
    });
}

fn state_variant(value: &str) -> &'static str {
    match value {
        "published" => "Published",
        "artifact-pending" => "ArtifactPending",
        other => panic!("unknown flash artifact state {other:?}"),
    }
}

fn transport_variant(value: &str) -> &'static str {
    match value {
        "esp-web-serial" => "EspWebSerial",
        "uf2-mass-storage" => "Uf2MassStorage",
        other => panic!("unknown flash transport {other:?}"),
    }
}

fn format_variant(value: &str) -> &'static str {
    match value {
        "esp-bin" => "EspBin",
        "uf2" => "Uf2",
        other => panic!("unknown flash artifact format {other:?}"),
    }
}

fn embedded_policy_variant(value: &str) -> &'static str {
    match value {
        "hosted-only" => "HostedOnly",
        "bundled" => "Bundled",
        other => panic!("unknown embedded policy {other:?}"),
    }
}

fn rust_option_string(value: &str) -> String {
    if value.is_empty() {
        "None".to_string()
    } else {
        format!("Some({})", rust_string(value))
    }
}

fn rust_option_u64(value: &str) -> String {
    if value.is_empty() {
        "None".to_string()
    } else {
        format!("Some({value})")
    }
}

fn rust_string(value: &str) -> String {
    quoted_string(value)
}

fn json_option_string(value: &str) -> String {
    if value.is_empty() {
        "null".to_string()
    } else {
        json_string(value)
    }
}

fn json_option_u64(value: &str) -> String {
    if value.is_empty() {
        "null".to_string()
    } else {
        value.to_string()
    }
}

fn json_string(value: &str) -> String {
    quoted_string(value)
}

fn quoted_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

fn generate_board_images() {
    const BOARD_IMAGES: &[(&str, &str)] = &[
        ("HELTEC_V4", "heltec-v4.webp"),
        ("T_BEAM_SUPREME", "t-beam-supreme.webp"),
        ("XIAO_ESP32_C6", "xiao-esp32-c6.webp"),
        ("T_ECHO", "t-echo.webp"),
    ];

    let mut generated =
        String::from("// @generated by build.rs; do not edit.\nuse super::BoardImage;\n\n");
    for (ident, file_name) in BOARD_IMAGES {
        let asset_path = PathBuf::from("src")
            .join("assets")
            .join("boards")
            .join(file_name);
        println!("cargo:rerun-if-changed={}", asset_path.display());
        let bytes = fs::read(&asset_path).unwrap_or_else(|err| {
            panic!(
                "failed to read board image asset {}: {err}",
                asset_path.display()
            )
        });
        let data_uri = format!("data:image/webp;base64,{}", base64_encode(&bytes));
        generated.push_str(&format!(
            "pub static {ident}: BoardImage = BoardImage {{ data_uri: \"{data_uri}\" }};\n",
        ));
    }

    let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
    fs::write(PathBuf::from(out_dir).join("board_images.rs"), generated)
        .expect("failed to write generated board image module");
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        encoded.push(TABLE[(b0 >> 2) as usize] as char);
        encoded.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    Some(value.trim().to_owned())
}

fn short_commit(commit: &str) -> String {
    commit.chars().take(12).collect()
}
