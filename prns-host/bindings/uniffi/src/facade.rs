//! Foreign ownership and scheduling only. Value meanings are generated in transport.

use std::sync::{Arc, Mutex};

use prns_host_native::owner::{HostClient, OwnedSession, SessionError};
use prns_host_native::{
    NativeEventStream, NativeEventValue, NativeResourceStream, NativeStreamError, NativeUpload,
    UploadWriteError,
};

use crate::transport::{self as value, BindingError};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn binding_error(error: SessionError) -> BindingError {
    match error {
        SessionError::Busy
        | SessionError::Snapshot(prns_host_native::NativeSnapshotError::Busy) => BindingError::Busy,
        SessionError::Stopped
        | SessionError::Snapshot(prns_host_native::NativeSnapshotError::Stopped) => {
            BindingError::Stopped
        }
        SessionError::LifecycleUnavailable => BindingError::OwnershipUnavailable {
            detail: "native lifecycle executor unavailable".into(),
        },
        error => BindingError::Backend {
            detail: format!("{error:?}"),
        },
    }
}

// Only a joined native shutdown failure is Backend here. Foreign owners can then
// release their leases while preserving the failure; unavailable ownership must
// remain retained because it does not establish that shutdown joined.
fn stop_error(error: SessionError) -> BindingError {
    match error {
        error @ SessionError::Stop(_) => binding_error(error),
        error => BindingError::OwnershipUnavailable {
            detail: format!("{error:?}"),
        },
    }
}

#[derive(uniffi::Record)]
pub struct HostBindingContract {
    pub semantic_fingerprint: String,
    pub remote_control_fingerprint: String,
    pub package_version: String,
}

#[uniffi::export]
pub fn binding_contract() -> HostBindingContract {
    HostBindingContract {
        semantic_fingerprint: value::HOST_SEMANTIC_FINGERPRINT.into(),
        remote_control_fingerprint: crate::remote_control::REMOTE_CONTROL_SEMANTIC_FINGERPRINT
            .into(),
        package_version: env!("CARGO_PKG_VERSION").into(),
    }
}

#[uniffi::export]
pub fn backend_info() -> value::BackendInfo {
    prns_host_native::native_backend_info().into()
}

#[derive(uniffi::Object)]
pub struct HostSession {
    owner: Arc<OwnedSession>,
}

#[uniffi::export]
pub async fn open_host(config: value::HostConfig) -> Result<Arc<HostSession>, BindingError> {
    let owner = OwnedSession::open(config.try_into()?)
        .await
        .map_err(binding_error)?;
    Ok(Arc::new(HostSession { owner }))
}

#[uniffi::export]
pub async fn open_host_with_remote_control(
    config: value::HostConfig,
    remote_control: crate::remote_control::RemoteControlNativeRemoteControlConfig,
) -> Result<Arc<HostSession>, BindingError> {
    let embedding = prns_host_native::NativeEmbedding {
        remote_control_config: Some(remote_control.try_into()?),
        ..Default::default()
    };
    let owner = OwnedSession::open_with_embedding(config.try_into()?, embedding)
        .await
        .map_err(binding_error)?;
    Ok(Arc::new(HostSession { owner }))
}

#[uniffi::export]
impl HostSession {
    pub fn client(&self) -> Arc<HostClientHandle> {
        HostClientHandle::borrow(self.owner.client())
    }

    pub async fn stop(&self) -> Result<(), BindingError> {
        self.owner.stop().await.map_err(stop_error)
    }
}

#[derive(uniffi::Object)]
pub struct HostClientHandle {
    pub(crate) client: HostClient,
}

impl HostClientHandle {
    /// Native application compositions return this borrowed facade to JS. It has no stop API.
    pub fn borrow(client: HostClient) -> Arc<Self> {
        Arc::new(Self { client })
    }
}

#[uniffi::export]
impl HostClientHandle {
    pub async fn execute(
        &self,
        command: value::HostCommand,
    ) -> Result<value::CommandSettlement, BindingError> {
        Ok(self
            .client
            .execute(command.try_into()?)
            .await
            .map_err(binding_error)?
            .into())
    }

    pub async fn snapshot(&self) -> Result<value::HostSnapshot, BindingError> {
        Ok(self.client.snapshot().await.map_err(binding_error)?.into())
    }

    pub fn lifecycle(&self) -> value::LifecycleSnapshot {
        self.client.events().lifecycle().into()
    }

    pub fn identity_hash(&self) -> Result<Vec<u8>, BindingError> {
        Ok(self
            .client
            .identity_hash()
            .map_err(binding_error)?
            .as_bytes()
            .to_vec())
    }

    pub fn destination_hashes(&self) -> Result<Vec<Vec<u8>>, BindingError> {
        Ok(self
            .client
            .destination_hashes()
            .map_err(binding_error)?
            .into_iter()
            .map(|hash| hash.as_bytes().to_vec())
            .collect())
    }

    pub async fn destination_public_key(
        &self,
        destination: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, BindingError> {
        let destination = destination
            .try_into()
            .map(prns_host::DestinationHash::new)
            .map_err(|_| BindingError::InvalidInput {
                field: "destination".into(),
                detail: "expected 16 bytes".into(),
            })?;
        Ok(self
            .client
            .destination_public_key(destination)
            .await
            .map_err(binding_error)?
            .map(|public| public.to_vec()))
    }

    pub async fn remote_control_exchange(
        &self,
        link_id: Vec<u8>,
        request: crate::remote_control::RemoteControlRequest,
    ) -> Result<crate::remote_control::RemoteControlExchangeSettlement, BindingError> {
        let link_id = link_id
            .try_into()
            .map(prns_host::LinkId::new)
            .map_err(|_| BindingError::InvalidInput {
                field: "linkId".into(),
                detail: "expected 16 bytes".into(),
            })?;
        let result = self
            .client
            .remote_control_exchange(link_id, request.try_into()?)
            .await;
        Ok(prns_host_native::remote_control::RemoteControlExchangeSettlement::from(result).into())
    }

    pub async fn remote_control_target_exchange(
        &self,
        target: Vec<u8>,
        request: crate::remote_control::RemoteControlRequest,
    ) -> Result<crate::remote_control::RemoteControlExchangeSettlement, BindingError> {
        let target = target
            .try_into()
            .map(prns_host::IdentityHash::new)
            .map_err(|_| BindingError::InvalidInput {
                field: "target".into(),
                detail: "expected 16 bytes".into(),
            })?;
        let result = self
            .client
            .remote_control_target_exchange(target, request.try_into()?)
            .await;
        Ok(prns_host_native::remote_control::RemoteControlExchangeSettlement::from(result).into())
    }

    pub fn application_events(&self) -> Result<Arc<EventStream>, BindingError> {
        self.claim(prns_host::ConsumerLane::ApplicationEvents)
    }

    pub fn diagnostics(&self) -> Result<Arc<EventStream>, BindingError> {
        self.claim(prns_host::ConsumerLane::Diagnostics)
    }

    pub fn begin_resource_upload(
        &self,
        link_id: Vec<u8>,
        total_bytes: u64,
        metadata: Option<Vec<u8>>,
        compression: value::ResourceCompression,
    ) -> Result<Arc<ResourceUpload>, BindingError> {
        let link_id = link_id
            .try_into()
            .map(prns_host::LinkId::new)
            .map_err(|_| BindingError::InvalidInput {
                field: "linkId".into(),
                detail: "expected 16 bytes".into(),
            })?;
        let upload = self
            .client
            .begin_resource_upload(link_id, total_bytes, metadata, compression.try_into()?)
            .map_err(binding_error)?;
        Ok(Arc::new(ResourceUpload { upload }))
    }
}

impl HostClientHandle {
    fn claim(&self, lane: prns_host::ConsumerLane) -> Result<Arc<EventStream>, BindingError> {
        let stream = self
            .client
            .events()
            .claim_stream(lane)
            .map_err(|_| BindingError::AlreadyClaimed)?;
        Ok(Arc::new(EventStream { stream }))
    }
}

#[derive(uniffi::Enum)]
pub enum HostEvent {
    Application {
        event: value::ApplicationEvent,
        resource: Option<Arc<ResourceReader>>,
    },
    Diagnostic {
        event: value::DiagnosticEvent,
    },
    RemoteControl {
        event: crate::remote_control::RemoteControlNativeRemoteControlEvent,
    },
}

#[derive(uniffi::Object)]
pub struct EventStream {
    stream: NativeEventStream,
}

#[uniffi::export]
impl EventStream {
    /// This future never consumes an event. The foreign iterator drains after it wakes.
    pub async fn ready(&self) -> Result<bool, BindingError> {
        match self.stream.ready().await {
            Ok(()) => Ok(true),
            Err(NativeStreamError::Stopped) => Ok(false),
            Err(NativeStreamError::Interrupted) => Err(BindingError::Interrupted),
            Err(error) => Err(BindingError::OwnershipUnavailable {
                detail: format!("{error:?}"),
            }),
        }
    }

    pub fn try_next(&self) -> Result<Option<HostEvent>, BindingError> {
        let event = match self.stream.try_next() {
            Ok(event) => event,
            Err(NativeStreamError::WouldBlock | NativeStreamError::Stopped) => return Ok(None),
            Err(error) => {
                return Err(BindingError::OwnershipUnavailable {
                    detail: format!("{error:?}"),
                })
            }
        };
        Ok(Some(match event.value {
            NativeEventValue::Application(value) => HostEvent::Application {
                event: value.into(),
                resource: event.resource.map(|resource| {
                    Arc::new(ResourceReader {
                        stream: Mutex::new(Some(resource)),
                    })
                }),
            },
            NativeEventValue::RemoteControl(value) => HostEvent::RemoteControl {
                event: value.into(),
            },
            NativeEventValue::Diagnostic(value) => HostEvent::Diagnostic {
                event: value.into(),
            },
            NativeEventValue::DiagnosticsDropped(count) => HostEvent::Diagnostic {
                event: value::DiagnosticEvent::DiagnosticsDropped {
                    count: count.into(),
                },
            },
        }))
    }

    pub fn close_stream(&self) {
        self.stream.close();
    }
}

#[derive(uniffi::Object)]
pub struct ResourceReader {
    stream: Mutex<Option<NativeResourceStream>>,
}

#[uniffi::export]
impl ResourceReader {
    /// Received bytes are already native-owned. Each synchronous transfer is bounded.
    pub fn read_chunk(&self, maximum_bytes: u32) -> Result<Option<Vec<u8>>, BindingError> {
        if maximum_bytes == 0 || maximum_bytes > 256 * 1024 {
            return Err(BindingError::InvalidInput {
                field: "maximumBytes".into(),
                detail: "expected 1..=262144".into(),
            });
        }
        let stream = lock(&self.stream);
        let stream = stream
            .as_ref()
            .ok_or_else(|| BindingError::OwnershipUnavailable {
                detail: "resource closed".into(),
            })?;
        Ok(stream.read_chunk(maximum_bytes as usize))
    }

    pub fn close_reader(&self) {
        lock(&self.stream).take();
    }
}

#[derive(uniffi::Object)]
pub struct ResourceUpload {
    upload: NativeUpload,
}

#[uniffi::export]
impl ResourceUpload {
    pub async fn write_chunk(&self, bytes: Vec<u8>) -> Result<(), BindingError> {
        self.upload
            .write_async(bytes)
            .await
            .map_err(|error| match error {
                UploadWriteError::ChunkTooLarge | UploadWriteError::LengthOverrun => {
                    BindingError::InvalidInput {
                        field: "chunk".into(),
                        detail: format!("{error:?}"),
                    }
                }
                error => BindingError::OwnershipUnavailable {
                    detail: format!("{error:?}"),
                },
            })
    }

    pub async fn finish(&self) -> Result<value::CommandSettlement, BindingError> {
        match self.upload.finish().wait_async().await {
            prns_host_native::CommandWait::Completed(result) => Ok(result.into()),
            error => Err(BindingError::OwnershipUnavailable {
                detail: format!("{error:?}"),
            }),
        }
    }

    pub fn abort(&self) {
        self.upload.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_host_native::{NativeSnapshotError, NativeStopError};

    #[test]
    fn snapshot_admission_failures_keep_their_binding_categories() {
        assert!(matches!(
            binding_error(SessionError::Snapshot(NativeSnapshotError::Busy)),
            BindingError::Busy
        ));
        assert!(matches!(
            binding_error(SessionError::Snapshot(NativeSnapshotError::Stopped)),
            BindingError::Stopped
        ));
        assert!(matches!(
            binding_error(SessionError::Snapshot(NativeSnapshotError::TimedOut)),
            BindingError::Backend { .. }
        ));
    }

    #[test]
    fn stop_failure_projection_distinguishes_joined_from_unavailable() {
        for failure in [
            NativeStopError::WorkerPanicked,
            NativeStopError::EventBackpressure,
        ] {
            assert!(matches!(
                stop_error(SessionError::Stop(failure)),
                BindingError::Backend { .. }
            ));
        }
        assert!(matches!(
            stop_error(SessionError::LifecycleUnavailable),
            BindingError::OwnershipUnavailable { .. }
        ));
        assert!(matches!(
            binding_error(SessionError::LifecycleUnavailable),
            BindingError::OwnershipUnavailable { .. }
        ));
        // Future/unexpected session errors must not authorize foreign cleanup.
        assert!(matches!(
            stop_error(SessionError::Stopped),
            BindingError::OwnershipUnavailable { .. }
        ));
    }
}
