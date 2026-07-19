use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::{sha256_file, write_if_changed};

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

pub(crate) fn generate(build_version: &str, write_public_assets: bool) {
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

fn parse_flash_manifest(source: &str, source_path: &Path) -> Vec<FlashManifestRecord> {
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

fn parse_manifest_value(value: &str, source_path: &Path, line: usize) -> String {
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
    source_path: &Path,
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
