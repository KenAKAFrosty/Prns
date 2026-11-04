use super::*;
use prns_core::routing::announce::emit::AnnounceAppDataBytes;
use std::sync::atomic::{AtomicBool, Ordering};

type Name = core::BoundedText<128>;
#[derive(Default)]
pub(super) struct Advertisement {
    pub name: Option<Name>,
    pub installed: Option<Name>,
    pub task: Option<Publication>,
}
pub(super) struct Publication {
    name: Name,
    pub task: Task,
    current: Arc<AtomicBool>,
    published: Arc<AtomicBool>,
}
impl Publication {
    pub fn supersede(&self) {
        self.current.store(false, Ordering::Release);
    }
}
impl Advertisement {
    pub fn current(&self) -> bool {
        self.name.is_some() && self.name == self.installed && self.task.is_none()
    }
}
pub(super) fn card(entry: &Resources) -> Result<Vec<u8>, DriverError> {
    let key = entry.key.as_ref().ok_or(DriverError::MissingOwner)?;
    let identity = entry.identity.ok_or(DriverError::MissingOwner)?;
    if key.material().identity_hash().as_bytes() != &identity.public_hash.0 {
        return Err(DriverError::WrongOwner);
    }
    let name = entry.contact.name.ok_or(DriverError::MissingOwner)?;
    // Existing legal identity metadata can be outside the optional card policy.
    // Keep messaging usable, without truncating or inventing a public name.
    if prns_identities::contact_card::validate_name(name.as_str()).is_err() {
        return Ok(Vec::new());
    }
    key.contact_card(
        transport::contact_card::enrollment_service_name_hash(),
        name.as_str(),
    )
    .map_err(DriverError::Contact)
}
impl<const I: usize, const S: usize, J: Send + 'static> Driver<I, S, J> {
    pub(super) fn poll_contact_publications(&mut self) {
        for entry in &mut self.owners {
            if let Some(publication) = &mut entry.contact.task {
                match publication.task.poll(&self.wake) {
                    None => continue,
                    Some(Ok(())) => {
                        if publication.published.load(Ordering::Acquire) {
                            entry.contact.installed = Some(publication.name);
                        }
                    }
                    Some(Err(error)) => {
                        entry.failure = Some(error);
                        entry.fence();
                    }
                }
                entry.contact.task = None;
            }
            if entry.contact.current()
                || !entry.opened
                || !entry.dispatch.is_open()
                || entry.quiesced
                || entry.failure.is_some()
                || !entry
                    .ingress_ready
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                continue;
            }
            let (Some(context), Some(route), Some(key), Some(name)) =
                (&self.network, &entry.route, &entry.key, entry.contact.name)
            else {
                continue;
            };
            let bytes = match card(entry) {
                Ok(bytes) => bytes,
                Err(DriverError::Contact(error)) => {
                    entry.failure = Some(Arc::new(Failure::Contact(error)));
                    entry.fence();
                    continue;
                }
                Err(_) => {
                    entry.failure = Some(Arc::new(Failure::WrongDestination));
                    entry.fence();
                    continue;
                }
            };
            let Ok(app_data) = AnnounceAppDataBytes::from_slice(&bytes) else {
                entry.failure = Some(Arc::new(Failure::Contact(core::ApiError::InvalidInput)));
                entry.fence();
                continue;
            };
            let scope = route.scope().clone();
            let destination =
                prns_core::wire::DestinationHash::new(route.enrollment_destination().into_bytes());
            let key = key.clone();
            let dispatch = entry.dispatch.clone();
            let current = Arc::new(AtomicBool::new(true));
            let published = Arc::new(AtomicBool::new(false));
            let task_current = current.clone();
            let task_published = published.clone();
            entry.contact.task = Some(Publication {
                name,
                current,
                published,
                task: Task::spawn(
                    context,
                    entry.owner,
                    Operation::ContactPublication,
                    async move {
                        let _key = key;
                        loop {
                            if !dispatch.is_open() || !task_current.load(Ordering::Acquire) {
                                return Ok(());
                            }
                            let app_data = app_data.clone();
                            let current = task_current.clone();
                            let published = task_published.clone();
                            let result = scope
                                .protocol_operation(move |handle| async move {
                                    if !current.load(Ordering::Acquire) {
                                        return Ok(());
                                    }
                                    let result = handle
                                        .set_registered_announce_app_data(
                                            prns_core::engine::SetRegisteredAnnounceAppData {
                                                destination,
                                                app_data,
                                            },
                                        )
                                        .await;
                                    if result.is_ok() {
                                        published.store(true, Ordering::Release);
                                    }
                                    result
                                })
                                .await;
                            match result {
                                Ok(result) => return result.map_err(Failure::AnnounceData),
                                // No operation was admitted for these exact outcomes.
                                // Keep this one bounded owner task and queued announce
                                // intent while ordinary sync releases capacity.
                                Err(
                                    HostServiceScopeError::Capacity
                                    | HostServiceScopeError::Session(SessionError::Busy),
                                ) => {
                                    tokio::time::sleep(Duration::from_millis(50)).await;
                                }
                                Err(HostServiceScopeError::Fenced) if !dispatch.is_open() => {
                                    return Ok(())
                                }
                                Err(error) => return Err(Failure::Scope(error)),
                            }
                        }
                    },
                ),
            });
        }
    }
}
