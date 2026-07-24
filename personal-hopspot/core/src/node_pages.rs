use personal_rns::routing::links::request::{packed_binary_len, RESPONSE_WIRE_OVERHEAD};
use personal_rns::routing::links::resources::sealed_transfer_bytes;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};

pub const NODE_APP_NAME: &str = "nomadnetwork";
pub const NODE_ASPECTS: &[&str] = &["node"];
pub const INDEX_PATH: &str = "/page/index.mu";

const INDEX_HEAD: &str = include_str!("node_pages/index_head.mu");
const INDEX_TAIL: &str = include_str!("node_pages/index_tail.mu");

pub const HOPSPOT_MISSION_LINE: &str =
    "`F999This node is a Personal Hopspot, one small piece of that future.`f\n";
pub const BROWSER_MISSION_LINE: &str =
    "`F999This node lives in a browser tab, one small piece of that future.`f\n";

pub const fn index_page_len(mission_line: &str) -> usize {
    INDEX_HEAD.len() + mission_line.len() + INDEX_TAIL.len()
}

const fn assemble_index_page<const LEN: usize>(mission_line: &str) -> [u8; LEN] {
    let mut page = [0u8; LEN];
    let mut cursor = 0;
    let head = INDEX_HEAD.as_bytes();
    let mission = mission_line.as_bytes();
    let tail = INDEX_TAIL.as_bytes();
    let mut index = 0;
    while index < head.len() {
        page[cursor] = head[index];
        cursor += 1;
        index += 1;
    }
    index = 0;
    while index < mission.len() {
        page[cursor] = mission[index];
        cursor += 1;
        index += 1;
    }
    index = 0;
    while index < tail.len() {
        page[cursor] = tail[index];
        cursor += 1;
        index += 1;
    }
    page
}

pub static HOPSPOT_INDEX_PAGE: [u8; index_page_len(HOPSPOT_MISSION_LINE)] =
    assemble_index_page(HOPSPOT_MISSION_LINE);
pub static BROWSER_INDEX_PAGE: [u8; index_page_len(BROWSER_MISSION_LINE)] =
    assemble_index_page(BROWSER_MISSION_LINE);

const LARGEST_INDEX_PAGE_LEN: usize = {
    let hopspot = HOPSPOT_INDEX_PAGE.len();
    let browser = BROWSER_INDEX_PAGE.len();
    if hopspot > browser {
        hopspot
    } else {
        browser
    }
};

pub const INDEX_PACKED_RESPONSE_LEN: usize = match packed_binary_len(LARGEST_INDEX_PAGE_LEN) {
    Some(len) => len,
    None => panic!("index page exceeds MessagePack binary limits"),
};
pub const INDEX_RESPONSE_TRANSFER_BYTES: usize =
    sealed_transfer_bytes(RESPONSE_WIRE_OVERHEAD + INDEX_PACKED_RESPONSE_LEN);

pub struct NodeIndexPage;

impl<S> RequestRoute<S> for NodeIndexPage {
    const PATH: &'static str = INDEX_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, S>) -> Result<(), Decline> {
        context.respond_static_bytes(&HOPSPOT_INDEX_PAGE)
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
    fn the_index_response_capacity_covers_both_flavors() {
        assert!(LARGEST_INDEX_PAGE_LEN >= HOPSPOT_INDEX_PAGE.len());
        assert!(LARGEST_INDEX_PAGE_LEN >= BROWSER_INDEX_PAGE.len());
        assert_eq!(
            INDEX_PACKED_RESPONSE_LEN,
            packed_binary_len(LARGEST_INDEX_PAGE_LEN).unwrap()
        );
        assert_eq!(
            INDEX_RESPONSE_TRANSFER_BYTES,
            sealed_transfer_bytes(RESPONSE_WIRE_OVERHEAD + INDEX_PACKED_RESPONSE_LEN)
        );
    }

    #[test]
    fn each_flavor_names_what_serves_it() {
        let hopspot = core::str::from_utf8(&HOPSPOT_INDEX_PAGE).unwrap();
        let browser = core::str::from_utf8(&BROWSER_INDEX_PAGE).unwrap();
        assert!(hopspot.contains("This node is a Personal Hopspot"));
        assert!(!hopspot.contains("browser tab"));
        assert!(browser.contains("This node lives in a browser tab"));
        assert!(!browser.contains("is a Personal Hopspot"));
        assert!(hopspot.contains("one small piece of that future"));
        assert!(browser.contains("one small piece of that future"));
    }

    #[test]
    fn the_index_page_is_balanced_micron() {
        for page in [&HOPSPOT_INDEX_PAGE[..], &BROWSER_INDEX_PAGE[..]] {
            let page = core::str::from_utf8(page).unwrap();
            assert!(!page.is_ascii());
            let mut formatting_toggles = 0usize;
            for line in page.lines() {
                formatting_toggles += line.matches("`!").count();
                assert!(!line.contains('\t'));
                assert!(line.len() <= 600);
            }
            assert_eq!(formatting_toggles % 2, 0);
            for color in ["`F6eb", "`F3d9", "`F999", "`F678"] {
                assert!(page.contains(color));
            }
            assert_eq!(page.matches("`c").count(), page.matches("`a").count());
        }
    }
}
