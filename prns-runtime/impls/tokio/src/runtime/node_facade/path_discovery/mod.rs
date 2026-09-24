use crate::engine::{
    PathFound, PathRequestId, PrnsCommand, RequestPath, Settlement, PATH_REQUEST_ID_LEN,
};
use crate::wire::DestinationHash;

use super::{PrnsNodeHandle, RequestPathError};

impl PrnsNodeHandle {
    pub async fn request_path(
        &self,
        destination: DestinationHash,
    ) -> Result<PathFound, RequestPathError> {
        let mut request_id = [0; PATH_REQUEST_ID_LEN];
        self.entropy
            .try_fill_path_id(&mut request_id)
            .map_err(|_| RequestPathError::EntropyUnavailable)?;
        let timing = self.path_command_timing().await;
        match self
            .settle_with_timing(
                PrnsCommand::RequestPath(RequestPath {
                    destination,
                    id: PathRequestId::new(request_id),
                }),
                timing,
            )
            .await
        {
            Some(Settlement::RequestPath(result)) => result.map_err(RequestPathError::Failed),
            Some(_) | None => Err(RequestPathError::NodeStopped),
        }
    }
}

#[cfg(test)]
mod tests;
