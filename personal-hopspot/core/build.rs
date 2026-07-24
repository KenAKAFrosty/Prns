use sha2::{Digest, Sha256};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE_ENV: &[&str] = &[
    "PRNS_SOURCE_ARCHIVE",
    "PRNS_SOURCE_VERSION",
    "PRNS_SOURCE_COMMIT",
    "PRNS_SOURCE_SIZE",
    "PRNS_SOURCE_SHA256",
];

fn main() {
    println!("cargo:rerun-if-changed=src/node_pages/index_head.mu");
    println!("cargo:rerun-if-changed=src/node_pages/index_tail.mu");
    println!("cargo:rerun-if-changed=../../VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PRNS_BUILD_COMMIT");
    for name in SOURCE_ENV {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let repo = manifest.join("../..");
    let source_enabled = env::var_os("CARGO_FEATURE_SOURCE_ARCHIVE").is_some();
    let fallback_version = fs::read_to_string(repo.join("VERSION"))
        .unwrap_or_else(|_| env::var("CARGO_PKG_VERSION").expect("package version"))
        .trim()
        .to_owned();
    let fallback_commit = git_commit(&repo).unwrap_or_else(|| "development".to_owned());

    let (version, commit, source) = if source_enabled {
        let archive = required_path("PRNS_SOURCE_ARCHIVE");
        println!("cargo:rerun-if-changed={}", archive.display());
        let bytes = fs::read(&archive).expect("read PRNS_SOURCE_ARCHIVE");
        let version = required("PRNS_SOURCE_VERSION");
        let commit = required("PRNS_SOURCE_COMMIT");
        assert_eq!(
            version, fallback_version,
            "PRNS_SOURCE_VERSION must match the repository VERSION"
        );
        assert!(
            commit.len() == 40
                && commit
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "PRNS_SOURCE_COMMIT must be a lowercase full Git commit"
        );
        assert_eq!(
            git_commit(&repo).as_deref(),
            Some(commit.as_str()),
            "PRNS_SOURCE_COMMIT must match repository HEAD"
        );
        let expected_size: usize = required("PRNS_SOURCE_SIZE")
            .parse()
            .expect("PRNS_SOURCE_SIZE must be an integer");
        assert_eq!(
            bytes.len(),
            expected_size,
            "PRNS_SOURCE_ARCHIVE size does not match canonical metadata"
        );
        let digest = hex_digest(&bytes);
        let expected_digest = required("PRNS_SOURCE_SHA256");
        assert!(
            expected_digest.len() == 64
                && expected_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "PRNS_SOURCE_SHA256 must be a lowercase SHA-256 digest"
        );
        assert_eq!(
            digest, expected_digest,
            "PRNS_SOURCE_ARCHIVE SHA-256 does not match canonical metadata"
        );
        (version, commit, Some((archive, bytes.len(), digest)))
    } else {
        (
            env::var("PRNS_BUILD_VERSION").unwrap_or(fallback_version),
            env::var("PRNS_BUILD_COMMIT").unwrap_or(fallback_commit),
            None,
        )
    };

    let head =
        fs::read_to_string(manifest.join("src/node_pages/index_head.mu")).expect("read index head");
    let tail =
        fs::read_to_string(manifest.join("src/node_pages/index_tail.mu")).expect("read index tail");
    let short_commit = &commit[..commit.len().min(12)];
    let no_source = format!(
        "\nSource {version} {short_commit}: compact build; source.zip not carried or served.\n"
    );
    let with_source = source.as_ref().map(|(_, size, digest)| {
        format!(
            "\n>>`!Release source`!\n\nVersion: {version}\n\nCommit: {short_commit}\n\n\
             Archive: {size} bytes\n\nSHA-256: {digest}\n\n\
             `[Download source.zip`:/file/source.zip]\n\n\
             `[Download checksum`:/file/source.zip.sha256]\n\n"
        )
    });
    let hopspot_no_source = page(
        &head,
        "`F999This node is a Personal Hopspot, one small piece of that future.`f\n",
        &no_source,
        &tail,
    );
    let browser_no_source = page(
        &head,
        "`F999This node lives in a browser tab, one small piece of that future.`f\n",
        &no_source,
        &tail,
    );
    let hopspot_with_source = with_source.as_ref().map(|status| {
        page(
            &head,
            "`F999This node is a Personal Hopspot, one small piece of that future.`f\n",
            status,
            &tail,
        )
    });
    let browser_with_source = with_source.as_ref().map(|status| {
        page(
            &head,
            "`F999This node lives in a browser tab, one small piece of that future.`f\n",
            status,
            &tail,
        )
    });

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("hopspot_index_no_source.mu"), hopspot_no_source)
        .expect("write no-source hopspot page");
    fs::write(out.join("browser_index_no_source.mu"), browser_no_source)
        .expect("write no-source browser page");

    let mut generated = String::new();
    writeln!(generated, "pub const BUILD_VERSION: &str = {version:?};").unwrap();
    writeln!(generated, "pub const BUILD_COMMIT: &str = {commit:?};").unwrap();
    writeln!(
        generated,
        "pub const HOPSPOT_INDEX_PAGE_NO_SOURCE: &[u8] = \
         include_bytes!(concat!(env!(\"OUT_DIR\"), \"/hopspot_index_no_source.mu\"));"
    )
    .unwrap();
    writeln!(
        generated,
        "pub const BROWSER_INDEX_PAGE_NO_SOURCE: &[u8] = \
         include_bytes!(concat!(env!(\"OUT_DIR\"), \"/browser_index_no_source.mu\"));"
    )
    .unwrap();
    if let Some((archive, size, digest)) = source {
        let checksum = format!("{digest}  source.zip\n");
        fs::write(out.join("source.zip.sha256"), checksum).expect("write source checksum");
        fs::write(
            out.join("hopspot_index_with_source.mu"),
            hopspot_with_source.expect("source page"),
        )
        .expect("write source hopspot page");
        fs::write(
            out.join("browser_index_with_source.mu"),
            browser_with_source.expect("source page"),
        )
        .expect("write source browser page");
        writeln!(generated, "pub const SOURCE_ARCHIVE_SIZE: usize = {size};").unwrap();
        writeln!(
            generated,
            "pub const SOURCE_ARCHIVE_SHA256: &str = {digest:?};"
        )
        .unwrap();
        writeln!(
            generated,
            "pub static SOURCE_ARCHIVE: &[u8] = include_bytes!({:?});",
            archive.to_string_lossy()
        )
        .unwrap();
        writeln!(
            generated,
            "pub static SOURCE_CHECKSUM: &[u8] = \
             include_bytes!(concat!(env!(\"OUT_DIR\"), \"/source.zip.sha256\"));"
        )
        .unwrap();
        writeln!(
            generated,
            "pub const HOPSPOT_INDEX_PAGE_WITH_SOURCE: &[u8] = \
             include_bytes!(concat!(env!(\"OUT_DIR\"), \"/hopspot_index_with_source.mu\"));"
        )
        .unwrap();
        writeln!(
            generated,
            "pub const BROWSER_INDEX_PAGE_WITH_SOURCE: &[u8] = \
             include_bytes!(concat!(env!(\"OUT_DIR\"), \"/browser_index_with_source.mu\"));"
        )
        .unwrap();
    }
    fs::write(out.join("node_pages_generated.rs"), generated).expect("write generated node pages");
}

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is required with feature source-archive"))
}

fn required_path(name: &str) -> PathBuf {
    let path = PathBuf::from(required(name));
    assert!(path.is_absolute(), "{name} must be an absolute path");
    path
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
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

fn page(head: &str, mission: &str, source: &str, tail: &str) -> String {
    [head, mission, source, tail].concat()
}
