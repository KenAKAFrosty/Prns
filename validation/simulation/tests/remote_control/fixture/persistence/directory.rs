use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const RESERVATION_LIMIT: usize = 10_000;
static DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

pub(super) struct Directory(pub(super) PathBuf);

impl Directory {
    pub(super) fn new() -> Self {
        Self::reserve(
            &std::env::temp_dir(),
            std::process::id(),
            &DIRECTORY_SEQUENCE,
            RESERVATION_LIMIT,
        )
        .expect("isolated scenario storage")
    }

    fn reserve(
        parent: &Path,
        process_id: u32,
        sequence: &AtomicUsize,
        max_attempts: usize,
    ) -> io::Result<Self> {
        for _ in 0..max_attempts {
            let sequence = sequence.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!("prns-rc-sim-{process_id}-{sequence}"));
            match std::fs::create_dir(&directory) {
                Ok(()) => return Ok(Self(directory)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "scenario storage reservation limit exhausted",
        ))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove scenario storage");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PROCESS_ID: u32 = 42;
    const TEST_RESERVATION_LIMIT: usize = 3;

    #[test]
    fn occupied_namespace_is_preserved_while_a_fresh_directory_is_reserved() {
        let parent = Directory::new();
        let occupied = parent.0.join(format!("prns-rc-sim-{TEST_PROCESS_ID}-0"));
        std::fs::create_dir(&occupied).expect("occupied scenario namespace");
        let sentinel = occupied.join("retained-evidence");
        std::fs::write(&sentinel, b"prior scenario").expect("prior scenario evidence");
        let sequence = AtomicUsize::new(0);
        let fresh = Directory::reserve(
            &parent.0,
            TEST_PROCESS_ID,
            &sequence,
            TEST_RESERVATION_LIMIT,
        )
        .expect("reserve a fresh scenario namespace");
        assert_eq!(
            fresh.0,
            parent.0.join(format!("prns-rc-sim-{TEST_PROCESS_ID}-1"))
        );
        assert_eq!(
            std::fs::read(&sentinel).expect("retained evidence"),
            b"prior scenario"
        );
        let fresh_path = fresh.0.clone();
        drop(fresh);
        assert!(!fresh_path.exists());
        assert_eq!(
            std::fs::read(sentinel).expect("retained evidence after release"),
            b"prior scenario"
        );
    }

    #[test]
    fn an_exhausted_namespace_is_bounded_and_preserves_occupied_paths() {
        const ATTEMPT_LIMIT: usize = 2;
        let parent = Directory::new();
        let occupied = parent.0.join(format!("prns-rc-sim-{TEST_PROCESS_ID}-0"));
        std::fs::create_dir(&occupied).expect("occupied directory");
        let occupied_file = parent.0.join(format!("prns-rc-sim-{TEST_PROCESS_ID}-1"));
        std::fs::write(&occupied_file, b"retained file").expect("occupied file");
        let sequence = AtomicUsize::new(0);
        let error = Directory::reserve(&parent.0, TEST_PROCESS_ID, &sequence, ATTEMPT_LIMIT)
            .err()
            .expect("bounded namespace exhaustion");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(sequence.load(Ordering::Relaxed), ATTEMPT_LIMIT);
        assert!(occupied.is_dir());
        assert_eq!(
            std::fs::read(occupied_file).expect("retained file"),
            b"retained file"
        );
    }

    #[test]
    fn unrelated_filesystem_errors_are_preserved() {
        let parent = Directory::new();
        let sequence = AtomicUsize::new(0);
        let error = Directory::reserve(
            &parent.0.join("missing-parent"),
            TEST_PROCESS_ID,
            &sequence,
            TEST_RESERVATION_LIMIT,
        )
        .err()
        .expect("missing parent must fail");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(sequence.load(Ordering::Relaxed), 1);
    }
}
