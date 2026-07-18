use std::time::Duration;

use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::wire::DestinationHash;

const INITIAL_DELAY: Duration = Duration::from_secs(15);
const INTERVAL: Duration = Duration::from_secs(2 * 60 * 60);

pub struct ManagementAnnounceTask(tokio::task::JoinHandle<()>);

impl ManagementAnnounceTask {
    pub async fn shutdown(self) {
        self.0.abort();
        let _ = self.0.await;
    }
}

pub fn spawn(
    handle: PrnsNodeHandle,
    destinations: Vec<DestinationHash>,
) -> Option<ManagementAnnounceTask> {
    if destinations.is_empty() {
        return None;
    }
    Some(ManagementAnnounceTask(tokio::spawn(async move {
        let start = tokio::time::Instant::now() + INITIAL_DELAY;
        let mut interval = tokio::time::interval_at(start, INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            for destination in &destinations {
                if let Err(error) = handle.announce_now(announce_for(*destination)).await {
                    tracing::warn!(
                        event = "management_announce_failed",
                        destination = ?destination.as_bytes(),
                        error = ?error,
                    );
                }
            }
        }
    })))
}

fn announce_for(destination: DestinationHash) -> AnnounceNow {
    AnnounceNow {
        destination,
        target: AnnounceTarget::AllInterfaces,
        app_data: AnnounceAppData::Registered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_announces_use_registered_data_on_every_interface() {
        let destination = DestinationHash::new([0xA5; 16]);

        assert_eq!(
            announce_for(destination),
            AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }
        );
    }
}
