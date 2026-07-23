use personal_rns::routing::links::request::{packed_binary_len, RESPONSE_WIRE_OVERHEAD};
use personal_rns::routing::links::resources::sealed_transfer_len;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};

pub const NODE_APP_NAME: &str = "nomadnetwork";
pub const NODE_ASPECTS: &[&str] = &["node"];
pub const INDEX_PATH: &str = "/page/index.mu";
pub const INDEX_PAGE: &str = include_str!("node_pages/index.mu");

pub const INDEX_PACKED_RESPONSE_LEN: usize = match packed_binary_len(INDEX_PAGE.len()) {
    Some(len) => len,
    None => panic!("index page exceeds MessagePack binary limits"),
};
pub const INDEX_RESPONSE_TRANSFER_BYTES: usize =
    sealed_transfer_len(RESPONSE_WIRE_OVERHEAD + INDEX_PACKED_RESPONSE_LEN);

pub struct NodeIndexPage;

impl<S> RequestRoute<S> for NodeIndexPage {
    const PATH: &'static str = INDEX_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, S>) -> Result<(), Decline> {
        context.respond_static_bytes(INDEX_PAGE.as_bytes())
    }
}

pub struct NodePageRoutes;

impl<S> RouteSet<S> for NodePageRoutes {
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] =
        &[(INDEX_PATH, RoutePolicy::AllowAll)];

    async fn dispatch(
        context: RequestContext<'_, S>,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        if path_hash == RequestPathHash::of(INDEX_PATH) {
            NodeIndexPage::handle(context).await
        } else {
            Err(Decline::Ignore)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_response_capacity_matches_the_page() {
        assert_eq!(
            INDEX_PACKED_RESPONSE_LEN,
            packed_binary_len(INDEX_PAGE.len()).unwrap()
        );
        assert_eq!(
            INDEX_RESPONSE_TRANSFER_BYTES,
            sealed_transfer_len(RESPONSE_WIRE_OVERHEAD + INDEX_PACKED_RESPONSE_LEN)
        );
    }

    #[test]
    fn the_index_page_is_balanced_micron() {
        assert!(!INDEX_PAGE.is_ascii());
        let mut formatting_toggles = 0usize;
        for line in INDEX_PAGE.lines() {
            formatting_toggles += line.matches("`!").count();
            assert!(!line.contains('\t'));
            assert!(line.len() <= 220);
        }
        assert_eq!(formatting_toggles % 2, 0);
        for color in ["`F6eb", "`F3d9", "`F999", "`F678"] {
            assert!(INDEX_PAGE.contains(color));
        }
        assert_eq!(
            INDEX_PAGE.matches("`c").count(),
            INDEX_PAGE.matches("`a").count()
        );
    }
}
