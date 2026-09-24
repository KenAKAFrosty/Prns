use personal_rns::remote_control::{RemoteControlApplyOutcome, RemoteControlNodeName};
use personal_rns::runtime::RemoteControlHostCommandError;

/// Persist before publishing, and reconcile announces even when a previous write succeeded.
pub async fn apply_remote_node_name(
    durable: Option<RemoteControlNodeName>,
    requested: RemoteControlNodeName,
    mut persist: impl AsyncFnMut(RemoteControlNodeName) -> Result<(), RemoteControlHostCommandError>,
    mut announce: impl AsyncFnMut(RemoteControlNodeName) -> Result<(), RemoteControlHostCommandError>,
) -> Result<RemoteControlApplyOutcome, RemoteControlHostCommandError> {
    let outcome = if durable == Some(requested) {
        RemoteControlApplyOutcome::Unchanged
    } else {
        persist(requested).await?;
        RemoteControlApplyOutcome::Applied
    };
    announce(requested).await?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    fn complete<F: Future>(future: F) -> F::Output {
        match core::pin::pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("test operations complete immediately"),
        }
    }

    #[test]
    fn same_name_retry_repairs_either_failed_announce_without_another_flash_write() {
        let old = RemoteControlNodeName::new("Original").unwrap();
        let requested = RemoteControlNodeName::new("Rooftop").unwrap();
        for failed_destination in 0..2 {
            let durable = Cell::new(None);
            let writes = Cell::new(0);
            let announces = Cell::new([old; 2]);
            let failure = Cell::new(Some(failed_destination));
            let persist = async |name| {
                writes.set(writes.get() + 1);
                durable.set(Some(name));
                Ok(())
            };
            let announce = async |name| {
                for destination in 0..2 {
                    if failure.get() == Some(destination) {
                        failure.set(None);
                        return Err(RemoteControlHostCommandError::ApplyFailed);
                    }
                    let mut current = announces.get();
                    current[destination] = name;
                    announces.set(current);
                }
                Ok(())
            };
            assert_eq!(
                complete(apply_remote_node_name(
                    durable.get(),
                    requested,
                    persist,
                    announce
                )),
                Err(RemoteControlHostCommandError::ApplyFailed)
            );
            assert_eq!(durable.get(), Some(requested));
            assert_ne!(announces.get(), [requested; 2]);
            assert_eq!(
                complete(apply_remote_node_name(
                    durable.get(),
                    requested,
                    persist,
                    announce
                )),
                Ok(RemoteControlApplyOutcome::Unchanged)
            );
            assert_eq!((writes.get(), announces.get()), (1, [requested; 2]));
        }
    }

    #[test]
    fn failed_persistence_does_not_publish_the_new_name() {
        let requested = RemoteControlNodeName::new("Rooftop").unwrap();
        let result = complete(apply_remote_node_name(
            None,
            requested,
            async |_| Err(RemoteControlHostCommandError::PersistenceFailed),
            async |_| panic!("an uncommitted name must not be announced"),
        ));
        assert_eq!(
            result,
            Err(RemoteControlHostCommandError::PersistenceFailed)
        );
    }
}
