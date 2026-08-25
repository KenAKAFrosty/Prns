use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy, RequestEndpointSet,
};

use super::{DaemonRequestState, ListRoute, PathRoute, StatusRoute};

pub(crate) struct DaemonRequestRoutes;

impl RequestEndpointSet<DaemonRequestState> for DaemonRequestRoutes {
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[
        (StatusRoute::ENDPOINT_ID, StatusRoute::POLICY),
        (PathRoute::ENDPOINT_ID, PathRoute::POLICY),
        (ListRoute::ENDPOINT_ID, ListRoute::POLICY),
    ];

    async fn dispatch(
        context: RequestContext<'_, DaemonRequestState>,
        node: &impl personal_rns::runtime::PrnsNodeApi,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        if path_hash == RequestPathHash::of(StatusRoute::ENDPOINT_ID) {
            return StatusRoute::handle(context, node).await;
        }
        if path_hash == RequestPathHash::of(PathRoute::ENDPOINT_ID) {
            return PathRoute::handle(context, node).await;
        }
        if path_hash == RequestPathHash::of(ListRoute::ENDPOINT_ID) {
            return ListRoute::handle(context, node).await;
        }
        let nnpages = context.state.nnpages().clone();
        nnpages.respond(context, path_hash).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_routes_remain_registered() {
        assert_eq!(
            DaemonRequestRoutes::REGISTRATIONS,
            [
                (StatusRoute::ENDPOINT_ID, StatusRoute::POLICY),
                (PathRoute::ENDPOINT_ID, PathRoute::POLICY),
                (ListRoute::ENDPOINT_ID, ListRoute::POLICY),
            ]
        );
    }
}
