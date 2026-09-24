use portable_atomic::AtomicU64;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

pub(crate) struct TestDirectory(PathBuf);

impl TestDirectory {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "prns-authorization-storage-{}-{id}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create test directory: {error}"),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove owned test directory");
    }
}
