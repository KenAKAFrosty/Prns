use std::time::Duration;

use prns_core::units::InstantMillis;
use tokio::time::Instant;

pub(crate) fn elapsed_millis(started: Instant) -> InstantMillis {
    InstantMillis(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
}

pub(crate) fn instant_for(started: Instant, deadline: InstantMillis) -> Option<Instant> {
    started.checked_add(Duration::from_millis(deadline.0))
}

pub(crate) async fn wait_for_deadline(started: Instant, deadline: Option<InstantMillis>) {
    let Some(deadline) = deadline else {
        std::future::pending().await
    };
    let Some(deadline) = instant_for(started, deadline) else {
        std::future::pending().await
    };
    tokio::time::sleep_until(deadline).await;
}
