use std::time::Duration;

use prns_core::units::InstantMillis;
use tokio::time::Instant;

pub(crate) fn elapsed_millis(started: Instant) -> InstantMillis {
    InstantMillis(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
}

pub(crate) async fn wait_for_deadline(started: Instant, deadline: Option<InstantMillis>) {
    let Some(deadline) = deadline else {
        std::future::pending().await
    };
    let Some(deadline) = started.checked_add(Duration::from_millis(deadline.0)) else {
        std::future::pending().await
    };
    tokio::time::sleep_until(deadline).await;
}
