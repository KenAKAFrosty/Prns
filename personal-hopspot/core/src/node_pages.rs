use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};

pub const NODE_APP_NAME: &str = "nomadnetwork";
pub const NODE_ASPECTS: &[&str] = &["node"];
pub const INDEX_PATH: &str = "/page/index.mu";
pub const INDEX_PAGE: &str = include_str!("node_pages/index.mu");

pub const INDEX_RESPONSE_LEN: usize = 3 + INDEX_PAGE.len();
pub static INDEX_RESPONSE: [u8; INDEX_RESPONSE_LEN] = index_response();

const MSGPACK_BIN16: u8 = 0xC5;

const _: () = assert!(INDEX_PAGE.len() > u8::MAX as usize);
const _: () = assert!(INDEX_PAGE.len() <= u16::MAX as usize);

const fn index_response() -> [u8; INDEX_RESPONSE_LEN] {
    let page = INDEX_PAGE.as_bytes();
    let mut response = [0u8; INDEX_RESPONSE_LEN];
    response[0] = MSGPACK_BIN16;
    response[1] = (page.len() >> 8) as u8;
    response[2] = page.len() as u8;
    let mut index = 0;
    while index < page.len() {
        response[3 + index] = page[index];
        index += 1;
    }
    response
}

pub struct NodeIndexPage;

impl<S> RequestRoute<S> for NodeIndexPage {
    const PATH: &'static str = INDEX_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, S>) -> Result<(), Decline> {
        context.respond_borrowed(&INDEX_RESPONSE)
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
    fn the_index_response_is_the_page_as_one_msgpack_bin() {
        assert_eq!(INDEX_RESPONSE[0], MSGPACK_BIN16);
        assert_eq!(
            u16::from_be_bytes([INDEX_RESPONSE[1], INDEX_RESPONSE[2]]) as usize,
            INDEX_PAGE.len()
        );
        assert_eq!(&INDEX_RESPONSE[3..], INDEX_PAGE.as_bytes());
    }

    #[test]
    fn the_index_page_is_balanced_micron() {
        assert!(INDEX_PAGE.is_ascii() == false);
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
