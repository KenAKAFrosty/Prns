use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_endpoints::{Decline, RequestContext};

use crate::services::DaemonRequestState;

pub(crate) const DIRECTORY_NAME: &str = "pages";
pub(crate) const INDEX_FILE_NAME: &str = "index.mu";
pub(crate) const DEFAULT_INDEX_PAGE: &[u8] = include_bytes!("../assets/pages/index.mu");

const REQUEST_PREFIX: &str = "/page/";
const MAX_PAGE_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct NodePageCatalog {
    root: Arc<PathBuf>,
    pages: Arc<Vec<PublishedPage>>,
}

#[derive(Debug, Clone)]
struct PublishedPage {
    request_path: String,
    path_hash: RequestPathHash,
    disk_path: PathBuf,
}

impl NodePageCatalog {
    pub(crate) fn discover(config_dir: &Path) -> io::Result<Self> {
        let root = page_root(config_dir);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::empty(root));
            }
            Err(error) => return Err(error),
        };
        let mut pages = Vec::new();
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }
            let Some(file_name) = safe_page_name(&entry.file_name()) else {
                continue;
            };
            let request_path = format!("{REQUEST_PREFIX}{file_name}");
            pages.push(PublishedPage {
                path_hash: RequestPathHash::of(&request_path),
                disk_path: entry.path(),
                request_path,
            });
        }
        pages.sort_by(|left, right| left.request_path.cmp(&right.request_path));
        Ok(Self {
            root: Arc::new(root),
            pages: Arc::new(pages),
        })
    }

    pub(crate) fn empty(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
            pages: Arc::new(Vec::new()),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub(crate) fn request_paths(&self) -> impl Iterator<Item = &str> {
        self.pages.iter().map(|page| page.request_path.as_str())
    }

    pub(crate) fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE_NAME)
    }

    pub(crate) async fn respond(
        &self,
        mut context: RequestContext<'_, DaemonRequestState>,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        let Some(page) = self.pages.iter().find(|page| page.path_hash == path_hash) else {
            return Err(Decline::Ignore);
        };
        let disk_path = page.disk_path.clone();
        let request_path = page.request_path.clone();
        match tokio::task::spawn_blocking(move || read_page(&disk_path)).await {
            Ok(Ok(bytes)) => context.respond_bytes(&bytes),
            Ok(Err(PageReadError::Unavailable)) => Err(Decline::Ignore),
            Ok(Err(PageReadError::TooLarge)) => {
                tracing::warn!(
                    event = "node_page_too_large",
                    path = request_path,
                    maximum_bytes = MAX_PAGE_BYTES,
                );
                Err(Decline::Ignore)
            }
            Ok(Err(PageReadError::Read(error))) => {
                tracing::warn!(
                    event = "node_page_read_failed",
                    path = request_path,
                    error = %error,
                );
                Err(Decline::Ignore)
            }
            Err(error) => {
                tracing::warn!(
                    event = "node_page_reader_failed",
                    path = request_path,
                    error = %error,
                );
                Err(Decline::Ignore)
            }
        }
    }
}

pub(crate) fn page_root(config_dir: &Path) -> PathBuf {
    config_dir.join(DIRECTORY_NAME)
}

pub(crate) fn is_available(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn safe_page_name(name: &std::ffi::OsStr) -> Option<String> {
    let name = name.to_str()?;
    if name.starts_with('.')
        || !name.ends_with(".mu")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(name.to_owned())
}

#[derive(Debug)]
enum PageReadError {
    Unavailable,
    TooLarge,
    Read(io::Error),
}

fn read_page(path: &Path) -> Result<Vec<u8>, PageReadError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PageReadError::Unavailable
        } else {
            PageReadError::Read(error)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(PageReadError::Unavailable);
    }
    if metadata.len() > MAX_PAGE_BYTES {
        return Err(PageReadError::TooLarge);
    }
    let file = File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PageReadError::Unavailable
        } else {
            PageReadError::Read(error)
        }
    })?;
    let mut bytes =
        Vec::with_capacity(usize::try_from(metadata.len().min(MAX_PAGE_BYTES)).unwrap_or_default());
    file.take(MAX_PAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(PageReadError::Read)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PAGE_BYTES {
        return Err(PageReadError::TooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_indexes_only_safe_top_level_mu_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir(&root).expect("page root");
        fs::write(root.join("index.mu"), b"index").expect("index");
        fs::write(root.join("about-us_2.mu"), b"about").expect("about");
        fs::write(root.join("ignored.txt"), b"ignored").expect("ignored");
        fs::write(root.join(".private.mu"), b"private").expect("private");
        fs::create_dir(root.join("nested.mu")).expect("nested directory");

        let catalog = NodePageCatalog::discover(directory.path()).expect("catalog");

        assert_eq!(
            catalog.request_paths().collect::<Vec<_>>(),
            ["/page/about-us_2.mu", "/page/index.mu"]
        );
    }

    #[test]
    fn page_bytes_are_read_fresh_and_deletion_is_unavailable() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(INDEX_FILE_NAME);
        fs::write(&path, b"first").expect("first page");
        assert_eq!(read_page(&path).expect("first read"), b"first");

        fs::write(&path, b"second").expect("second page");
        assert_eq!(read_page(&path).expect("second read"), b"second");

        fs::remove_file(&path).expect("delete page");
        assert!(matches!(read_page(&path), Err(PageReadError::Unavailable)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_published_or_served() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = page_root(directory.path());
        fs::create_dir(&root).expect("page root");
        let source = directory.path().join("source.mu");
        fs::write(&source, b"outside").expect("source");
        let linked = root.join("linked.mu");
        symlink(&source, &linked).expect("symlink");

        let catalog = NodePageCatalog::discover(directory.path()).expect("catalog");
        assert!(catalog.is_empty());
        assert!(matches!(
            read_page(&linked),
            Err(PageReadError::Unavailable)
        ));
    }
}
