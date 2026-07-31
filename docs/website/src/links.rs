pub const BUILD_VERSION: &str = env!("PRNS_BUILD_VERSION");
pub const BUILD_COMMIT: &str = env!("PRNS_GIT_COMMIT");
pub const BUILD_COMMIT_SHORT: &str = env!("PRNS_GIT_COMMIT_SHORT");
pub const SOURCE_ZIP_SHA256_HREF: &str = "/source.zip.sha256";
pub const SOURCE_ZIP_HREF: &str = "/source.zip";
pub const API_DOCS_HREF: &str = "/api/";

pub fn source_archive_available() -> bool {
    env!("PRNS_SOURCE_ARCHIVE_AVAILABLE") == "true"
}

pub fn api_docs_available() -> bool {
    env!("PRNS_API_DOCS_AVAILABLE") == "true"
}

pub fn source_zip_download_name() -> String {
    let commit = BUILD_COMMIT_SHORT.trim();
    if commit.is_empty() || commit == "unknown" {
        "prns-source.zip".to_string()
    } else {
        format!("prns-source-{commit}.zip")
    }
}

pub fn source_zip_sha256_download_name() -> String {
    format!("{}.sha256", source_zip_download_name())
}
