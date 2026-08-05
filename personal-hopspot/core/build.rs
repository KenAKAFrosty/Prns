use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE_ENV: &[&str] = &[
    "PRNS_SOURCE_ARCHIVE",
    "PRNS_SOURCE_VERSION",
    "PRNS_SOURCE_COMMIT",
    "PRNS_SOURCE_SIZE",
    "PRNS_SOURCE_SHA256",
];

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=src/node_pages/hopspot_welcome.mu");
    println!("cargo:rerun-if-changed=src/node_pages/quickstart.mu");
    println!("cargo:rerun-if-changed=src/node_pages/browser_welcome.mu");
    println!("cargo:rerun-if-changed=src/node_pages/source_missing.mu");
    println!("cargo:rerun-if-changed=../../assets/nnpages/masthead.mu");
    println!("cargo:rerun-if-changed=../../assets/nnpages/nav.mu");
    println!("cargo:rerun-if-changed=../../assets/nnpages/why_prns.mu");
    println!("cargo:rerun-if-changed=../../assets/nnpages/license.mu");
    println!("cargo:rerun-if-changed=../../assets/nnpages/quote.mu");
    println!("cargo:rerun-if-changed=../../assets/nnpages/credits.mu");
    println!("cargo:rerun-if-changed=../../assets/nnpages/source_available.mu");
    println!("cargo:rerun-if-changed=../../VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT");
    for name in SOURCE_ENV {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let manifest = path_environment("CARGO_MANIFEST_DIR")?;
    let repo = manifest.join("../..");
    let source_enabled = env::var_os("CARGO_FEATURE_SOURCE_ARCHIVE").is_some();
    let fallback_version = match fs::read_to_string(repo.join("VERSION")) {
        Ok(version) => version,
        Err(_) => environment("CARGO_PKG_VERSION")?,
    }
    .trim()
    .to_owned();
    let fallback_commit = git_commit(&repo).unwrap_or_else(|| "development".to_owned());

    let (version, commit, source) = if source_enabled {
        let archive = required_path("PRNS_SOURCE_ARCHIVE")?;
        println!("cargo:rerun-if-changed={}", archive.display());
        let bytes = fs::read(&archive)?;
        let version = required("PRNS_SOURCE_VERSION")?;
        let commit = required("PRNS_SOURCE_COMMIT")?;
        ensure(
            version == fallback_version,
            "PRNS_SOURCE_VERSION must match the repository VERSION",
        )?;
        ensure(
            commit.len() == 40
                && commit
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "PRNS_SOURCE_COMMIT must be a lowercase full Git commit",
        )?;
        ensure(
            git_commit(&repo).as_deref() == Some(commit.as_str()),
            "PRNS_SOURCE_COMMIT must match repository HEAD",
        )?;
        let expected_size: usize = required("PRNS_SOURCE_SIZE")?.parse().map_err(|error| {
            invalid_input(format!("PRNS_SOURCE_SIZE must be an integer: {error}"))
        })?;
        ensure(
            bytes.len() == expected_size,
            "PRNS_SOURCE_ARCHIVE size does not match canonical metadata",
        )?;
        let digest = hex_digest(&bytes);
        let expected_digest = required("PRNS_SOURCE_SHA256")?;
        ensure(
            expected_digest.len() == 64
                && expected_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "PRNS_SOURCE_SHA256 must be a lowercase SHA-256 digest",
        )?;
        ensure(
            digest == expected_digest,
            "PRNS_SOURCE_ARCHIVE SHA-256 does not match canonical metadata",
        )?;
        (version, commit, Some((archive, bytes.len(), digest)))
    } else {
        (
            env::var("PRNS_BUILD_VERSION").unwrap_or(fallback_version),
            env::var("PRNS_BUILD_COMMIT").unwrap_or(fallback_commit),
            None,
        )
    };

    let shared = repo.join("assets/nnpages");
    let hopspot_head = [
        fs::read_to_string(shared.join("masthead.mu"))?,
        fs::read_to_string(manifest.join("src/node_pages/hopspot_welcome.mu"))?,
        fs::read_to_string(shared.join("nav.mu"))?,
        fs::read_to_string(shared.join("why_prns.mu"))?,
        fs::read_to_string(shared.join("license.mu"))?,
        fs::read_to_string(shared.join("quote.mu"))?,
    ]
    .concat();
    let browser_head = [
        fs::read_to_string(shared.join("masthead.mu"))?,
        fs::read_to_string(manifest.join("src/node_pages/browser_welcome.mu"))?,
        fs::read_to_string(shared.join("nav.mu"))?,
        fs::read_to_string(shared.join("why_prns.mu"))?,
        fs::read_to_string(shared.join("license.mu"))?,
        fs::read_to_string(shared.join("quote.mu"))?,
    ]
    .concat();
    let tail = [
        "\n",
        fs::read_to_string(shared.join("credits.mu"))?.as_str(),
    ]
    .concat();
    let hopspot_no_source = page(
        &hopspot_head,
        "`F999This node is a Personal Hopspot, one small piece of that future.`f\n",
        "",
        &tail,
    );
    let browser_index = page(
        &browser_head,
        "`F999This node lives in a browser tab, one small piece of that future.`f\n",
        "",
        &tail,
    );
    let source_page = match &source {
        Some((_, size, _)) => {
            let source_commit_line = format!("`F999Source commit:`f `F6eb{}`f\n\n", &commit[..12]);
            fs::read_to_string(shared.join("source_available.mu"))?
                .replacen("# prnsd:managed:source-page\n", "", 1)
                .replace("{{SIZE}}", &format_archive_size(*size as u64))
                .replace(
                    "{{CHECKSUM_LINE}}\n",
                    "`F999Verify:`f `F6eb`_`[source.zip.sha256`:/file/source.zip.sha256]`_`f\n\n",
                )
                .replace("{{SOURCE_COMMIT_LINE}}\n", &source_commit_line)
        }
        None => fs::read_to_string(manifest.join("src/node_pages/source_missing.mu"))?,
    };

    let out = path_environment("OUT_DIR")?;
    fs::write(out.join("hopspot_index_no_source.mu"), hopspot_no_source)?;
    fs::write(out.join("browser_index.mu"), browser_index)?;
    fs::write(out.join("source.mu"), source_page)?;

    let mut generated = String::new();
    generated.push_str(&format!("pub const BUILD_VERSION: &str = {version:?};\n"));
    generated.push_str(&format!("pub const BUILD_COMMIT: &str = {commit:?};\n"));
    generated.push_str(
        "pub const HOPSPOT_INDEX_PAGE_NO_SOURCE: &[u8] = \
         include_bytes!(concat!(env!(\"OUT_DIR\"), \"/hopspot_index_no_source.mu\"));\n",
    );
    generated.push_str(
        "pub const BROWSER_INDEX_PAGE: &[u8] = \
         include_bytes!(concat!(env!(\"OUT_DIR\"), \"/browser_index.mu\"));\n",
    );
    generated.push_str(
        "pub const SOURCE_PAGE: &[u8] = \
         include_bytes!(concat!(env!(\"OUT_DIR\"), \"/source.mu\"));\n\
         pub const BROWSER_SOURCE_PAGE: &[u8] = SOURCE_PAGE;\n",
    );
    if let Some((archive, size, digest)) = source {
        let checksum = format!("{digest}  source.zip\n");
        fs::write(out.join("source.zip.sha256"), checksum)?;
        fs::write(
            out.join("hopspot_index_with_source.mu"),
            page(
                &hopspot_head,
                "`F999This node is a Personal Hopspot, one small piece of that future.`f\n",
                "",
                &tail,
            ),
        )?;
        generated.push_str(&format!("pub const SOURCE_ARCHIVE_SIZE: usize = {size};\n"));
        generated.push_str(&format!(
            "pub const SOURCE_ARCHIVE_SHA256: &str = {digest:?};\n"
        ));
        generated.push_str(&format!(
            "pub static SOURCE_ARCHIVE: &[u8] = include_bytes!({:?});\n",
            archive.to_string_lossy()
        ));
        generated.push_str(
            "pub static SOURCE_CHECKSUM: &[u8] = \
             include_bytes!(concat!(env!(\"OUT_DIR\"), \"/source.zip.sha256\"));\n",
        );
        generated.push_str(
            "pub const HOPSPOT_INDEX_PAGE_WITH_SOURCE: &[u8] = \
             include_bytes!(concat!(env!(\"OUT_DIR\"), \"/hopspot_index_with_source.mu\"));\n",
        );
    }
    fs::write(out.join("node_pages_generated.rs"), generated)?;
    Ok(())
}

fn environment(name: &str) -> io::Result<String> {
    env::var(name).map_err(|error| invalid_input(format!("{name} is unavailable: {error}")))
}

fn path_environment(name: &str) -> io::Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| invalid_input(format!("{name} is unavailable")))
}

fn required(name: &str) -> io::Result<String> {
    env::var(name).map_err(|error| {
        invalid_input(format!(
            "{name} is required with feature source-archive: {error}"
        ))
    })
}

fn required_path(name: &str) -> io::Result<PathBuf> {
    let path = PathBuf::from(required(name)?);
    ensure(
        path.is_absolute(),
        format!("{name} must be an absolute path"),
    )?;
    Ok(path)
}

fn ensure(condition: bool, message: impl Into<String>) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid_input(message))
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, message.into())
}

fn git_commit(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn page(head: &str, mission: &str, source: &str, tail: &str) -> String {
    [head, mission, source, tail].concat()
}

fn format_archive_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let (scaled_tenths, unit) = if bytes < 1024 * 1024 {
        (bytes * 10 / 1024, "KB")
    } else if bytes < 1024 * 1024 * 1024 {
        (bytes * 10 / (1024 * 1024), "MB")
    } else {
        (bytes * 10 / (1024 * 1024 * 1024), "GB")
    };
    format!("{}.{} {}", scaled_tenths / 10, scaled_tenths % 10, unit)
}
