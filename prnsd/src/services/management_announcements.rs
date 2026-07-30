use std::path::{Path, PathBuf};
use std::time::Duration;

use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
use personal_rns::routing::announce::emit::AnnounceAppDataBytes;
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::wire::DestinationHash;

use crate::nnpages::{self, NnPagesSettings};

const INITIAL_DELAY: Duration = Duration::from_secs(15);

pub struct ManagementAnnounceTask(Vec<tokio::task::JoinHandle<()>>);

pub(crate) struct AnnouncedDestination {
    pub(crate) hash: DestinationHash,
    pub(crate) available_when: Option<PathBuf>,
    pub(crate) name_file: Option<PathBuf>,
    pub(crate) schedule: AnnouncementSchedule,
}

pub(crate) enum AnnouncementSchedule {
    Fixed(Duration),
    NnPages(tokio::sync::watch::Receiver<NnPagesSettings>),
}

impl ManagementAnnounceTask {
    pub async fn shutdown(self) {
        for task in &self.0 {
            task.abort();
        }
        for task in self.0 {
            let _ = task.await;
        }
    }
}

pub fn spawn(
    handle: PrnsNodeHandle,
    destinations: Vec<AnnouncedDestination>,
) -> Option<ManagementAnnounceTask> {
    if destinations.is_empty() {
        return None;
    }
    let tasks = destinations
        .into_iter()
        .map(|destination| {
            let handle = handle.clone();
            tokio::spawn(run_announcement_loop(handle, destination))
        })
        .collect();
    Some(ManagementAnnounceTask(tasks))
}

async fn run_announcement_loop(handle: PrnsNodeHandle, destination: AnnouncedDestination) {
    let AnnouncedDestination {
        hash,
        available_when,
        name_file,
        schedule,
    } = destination;
    match schedule {
        AnnouncementSchedule::Fixed(interval) => {
            let start = tokio::time::Instant::now() + INITIAL_DELAY;
            let mut ticker = tokio::time::interval_at(start, interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                announce_if_available(
                    &handle,
                    hash,
                    available_when.as_deref(),
                    name_file.as_deref(),
                )
                .await;
            }
        }
        AnnouncementSchedule::NnPages(mut settings) => {
            let mut active = *settings.borrow_and_update();
            let mut deadline = active
                .announce()
                .then(|| tokio::time::Instant::now() + INITIAL_DELAY);
            loop {
                match deadline {
                    Some(at) => {
                        tokio::select! {
                            () = tokio::time::sleep_until(at) => {
                                announce_if_available(
                                    &handle,
                                    hash,
                                    available_when.as_deref(),
                                    name_file.as_deref(),
                                ).await;
                                deadline = Some(
                                    tokio::time::Instant::now() + active.announce_interval()
                                );
                            }
                            changed = settings.changed() => {
                                if changed.is_err() {
                                    return;
                                }
                                let replacement = *settings.borrow_and_update();
                                deadline = updated_deadline(
                                    active,
                                    replacement,
                                    deadline,
                                    tokio::time::Instant::now(),
                                );
                                active = replacement;
                            }
                        }
                    }
                    None => {
                        if settings.changed().await.is_err() {
                            return;
                        }
                        let replacement = *settings.borrow_and_update();
                        deadline = updated_deadline(
                            active,
                            replacement,
                            None,
                            tokio::time::Instant::now(),
                        );
                        active = replacement;
                    }
                }
            }
        }
    }
}

fn updated_deadline(
    previous: NnPagesSettings,
    replacement: NnPagesSettings,
    current: Option<tokio::time::Instant>,
    now: tokio::time::Instant,
) -> Option<tokio::time::Instant> {
    if previous == replacement {
        return current;
    }
    if !replacement.announce() {
        return None;
    }
    if !previous.announce() {
        return Some(now + INITIAL_DELAY);
    }
    Some(now + replacement.announce_interval())
}

async fn announce_if_available(
    handle: &PrnsNodeHandle,
    hash: DestinationHash,
    available_when: Option<&Path>,
    name_file: Option<&Path>,
) {
    if available_when.is_some_and(|path| !nnpages::is_page_available(path)) {
        return;
    }
    let announce = announce_for(hash, name_file);
    if let Err(error) = handle.announce_now(announce).await {
        tracing::warn!(
            event = "management_announce_failed",
            destination = ?hash.as_bytes(),
            error = ?error,
        );
    }
}

pub(crate) fn announce_for(destination: DestinationHash, name_file: Option<&Path>) -> AnnounceNow {
    let app_data = name_file
        .and_then(nnpages::read_node_name)
        .and_then(|name| AnnounceAppDataBytes::from_slice(name.as_bytes()).ok())
        .map(AnnounceAppData::Data)
        .unwrap_or(AnnounceAppData::Registered);
    AnnounceNow {
        destination,
        target: AnnounceTarget::AllInterfaces,
        app_data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_announces_use_registered_data_on_every_interface() {
        let destination = DestinationHash::new([0xA5; 16]);

        assert_eq!(
            announce_for(destination, None),
            AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }
        );
    }

    #[test]
    fn a_node_name_file_overrides_the_registered_app_data() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("node_name");
        std::fs::write(&path, "Frosty Relay\n").expect("name");

        let announce = announce_for(DestinationHash::new([1; 16]), Some(&path));
        assert!(matches!(
            announce.app_data,
            AnnounceAppData::Data(ref bytes) if &bytes[..] == b"Frosty Relay"
        ));

        let absent = directory.path().join("missing");
        assert_eq!(
            announce_for(DestinationHash::new([1; 16]), Some(&absent)).app_data,
            AnnounceAppData::Registered
        );
    }

    #[test]
    fn missing_file_disables_conditional_announcement() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("index.mu");
        assert!(!nnpages::is_page_available(&path));
        std::fs::write(&path, b"page").expect("page");
        assert!(nnpages::is_page_available(&path));
        std::fs::remove_file(&path).expect("delete page");
        assert!(!nnpages::is_page_available(&path));

        let oversized = std::fs::File::create(&path).expect("oversized page");
        oversized
            .set_len(nnpages::MAX_PAGE_BYTES + 1)
            .expect("oversized page length");
        assert!(!nnpages::is_page_available(&path));
    }

    #[tokio::test]
    async fn nnpages_policy_changes_reschedule_without_disturbing_unchanged_deadlines() {
        let now = tokio::time::Instant::now();
        let enabled = NnPagesSettings::default();
        let original = Some(now + Duration::from_secs(90));
        assert_eq!(updated_deadline(enabled, enabled, original, now), original);

        let disabled = NnPagesSettings::new(false, 360).expect("disabled policy");
        assert_eq!(updated_deadline(enabled, disabled, original, now), None);
        assert_eq!(
            updated_deadline(disabled, enabled, None, now),
            Some(now + INITIAL_DELAY)
        );

        let faster = NnPagesSettings::new(true, 45).expect("changed interval");
        assert_eq!(
            updated_deadline(enabled, faster, original, now),
            Some(now + Duration::from_secs(45 * 60))
        );
    }
}
