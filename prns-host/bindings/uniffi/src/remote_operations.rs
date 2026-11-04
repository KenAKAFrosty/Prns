//! Foreign ownership for the shared process-local operation retainer. All state
//! and result values are generated from the native/protocol declarations.
use std::sync::Arc;

use crate::{remote_control as value, transport::BindingError, HostClientHandle};
use prns_host_runtime::{
    remote_control::RemoteControlOperations as RuntimeOperations, HostSubmitError,
};

#[derive(uniffi::Object)]
pub struct RemoteControlOperations {
    operations: RuntimeOperations,
}

impl RemoteControlOperations {
    /// Native embeddings may retain this object independently of foreign leases.
    pub fn from_operations(operations: RuntimeOperations) -> Arc<Self> {
        Arc::new(Self { operations })
    }
}

fn admission(error: HostSubmitError) -> BindingError {
    match error {
        HostSubmitError::Busy => BindingError::Busy,
        HostSubmitError::Stopped => BindingError::Stopped,
    }
}

#[uniffi::export]
impl RemoteControlOperations {
    #[uniffi::constructor]
    pub fn new(capacity: u32) -> Result<Arc<Self>, BindingError> {
        let operations =
            RuntimeOperations::new(capacity).map_err(|error| BindingError::InvalidInput {
                field: "capacity".into(),
                detail: format!(
                    "expected 1 through {} retained operations",
                    error.maximum_operations
                ),
            })?;
        Ok(Self::from_operations(operations))
    }

    pub fn submit_target(
        &self,
        client: Arc<HostClientHandle>,
        target: Vec<u8>,
        request: value::RemoteControlRequest,
    ) -> Result<u64, BindingError> {
        let target = target
            .try_into()
            .map(prns_host::IdentityHash::new)
            .map_err(|_| BindingError::InvalidInput {
                field: "target".into(),
                detail: "expected 16 bytes".into(),
            })?;
        self.operations
            .submit_target(&client.client, target, request.try_into()?)
            .map_err(admission)
    }

    pub fn submit_link(
        &self,
        client: Arc<HostClientHandle>,
        link_id: Vec<u8>,
        request: value::RemoteControlRequest,
    ) -> Result<u64, BindingError> {
        let link = link_id
            .try_into()
            .map(prns_host::LinkId::new)
            .map_err(|_| BindingError::InvalidInput {
                field: "linkId".into(),
                detail: "expected 16 bytes".into(),
            })?;
        self.operations
            .submit_link(&client.client, link, request.try_into()?)
            .map_err(admission)
    }

    pub fn operation(&self, operation_id: u64) -> Option<value::RemoteControlOperationSnapshot> {
        self.operations.operation(operation_id).map(Into::into)
    }

    pub fn operations(&self) -> Vec<value::RemoteControlOperationSnapshot> {
        self.operations
            .operations()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    pub async fn wait(&self, operation_id: u64) -> Option<value::RemoteControlOperationSnapshot> {
        self.operations.wait(operation_id).await.map(Into::into)
    }

    pub fn cancel(&self, operation_id: u64) -> value::RemoteControlOperationCancellation {
        self.operations.cancel(operation_id).into()
    }

    pub fn retire(&self, operation_id: u64) -> value::RemoteControlOperationRetirement {
        self.operations.retire(operation_id).into()
    }
}
