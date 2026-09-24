use super::*;
use personal_rns::persistence::FileStore;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove retained simulation state");
    }
}

#[derive(Clone)]
pub struct RetainedState(Arc<Directory>);
impl RetainedState {
    pub fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "prns-halow-recovery-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("isolated retained state");
        Self(Arc::new(Directory(path)))
    }
    pub fn directory(&self) -> &Path {
        &self.0 .0
    }
}

pub struct InlineIo;
impl PersistenceIo for InlineIo {
    fn execute(&self, task: PersistenceIoTask) -> PersistenceIoCompletion<'_> {
        Box::pin(async move {
            task.run();
            Ok(())
        })
    }
}

pub enum Persistence {
    Disabled,
    Retained(Vec<RetainedState>),
}
pub enum Selection {
    Disabled,
    Retained(NodePersistence),
}
impl PersistenceIntent for Selection {
    fn into_node_persistence(self) -> Option<NodePersistence> {
        match self {
            Self::Disabled => None,
            Self::Retained(persistence) => Some(persistence),
        }
    }
}
impl Lab<'_> {
    pub fn flush(&mut self, index: usize) {
        let Persistence::Retained(stores) = &self.persistence else {
            panic!("retained fixture");
        };
        let mut store = FileStore::new(stores[index].directory());
        let handle = self.nodes[index].handle.clone();
        self.complete(async move {
            handle
                .flush_to_store(&mut store)
                .await
                .expect("sealed snapshot publication");
        });
    }
}
