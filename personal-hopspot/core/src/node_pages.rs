use personal_rns::routing::links::request::{packed_binary_len, RESPONSE_WIRE_OVERHEAD};
use personal_rns::routing::links::resources::sealed_transfer_bytes;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy, RequestEndpointSet,
};
use personal_rns::runtime::PrnsNodeApi;

include!(concat!(env!("OUT_DIR"), "/node_pages_generated.rs"));

pub const NODE_APP_NAME: &str = "nomadnetwork";
pub const NODE_ASPECTS: &[&str] = &["node"];
pub const INDEX_PATH: &str = "/page/index.mu";
pub const QUICKSTART_PATH: &str = "/page/quickstart.mu";
pub const COMING_FROM_RNS_PATH: &str = "/page/coming-from-rns.mu";
pub const SOURCE_PAGE_PATH: &str = "/page/source.mu";
pub const SOURCE_ARCHIVE_PATH: &str = "/file/source.zip";
pub const SOURCE_CHECKSUM_PATH: &str = "/file/source.zip.sha256";
pub const QUICKSTART_PAGE: &[u8] = include_bytes!("node_pages/quickstart.mu");
pub const COMING_FROM_RNS_PAGE: &[u8] =
    include_bytes!("../../../assets/nnpages/coming_from_rns.mu");

#[cfg(feature = "source-archive")]
pub const SERVES_SOURCE_ARCHIVE: bool = true;
#[cfg(not(feature = "source-archive"))]
pub const SERVES_SOURCE_ARCHIVE: bool = false;

#[cfg(feature = "source-archive")]
pub const HOPSPOT_INDEX_PAGE: &[u8] = HOPSPOT_INDEX_PAGE_WITH_SOURCE;
#[cfg(not(feature = "source-archive"))]
pub const HOPSPOT_INDEX_PAGE: &[u8] = HOPSPOT_INDEX_PAGE_NO_SOURCE;

const LARGEST_INDEX_PAGE_LEN: usize = {
    let hopspot = HOPSPOT_INDEX_PAGE.len();
    let browser = BROWSER_INDEX_PAGE.len();
    if hopspot > browser {
        hopspot
    } else {
        browser
    }
};

const LARGEST_PAGE_LEN: usize = if QUICKSTART_PAGE.len() > LARGEST_INDEX_PAGE_LEN {
    QUICKSTART_PAGE.len()
} else {
    LARGEST_INDEX_PAGE_LEN
};

const LARGEST_SINGLE_WINDOW_PAGE_LEN: usize = if SOURCE_PAGE.len() > LARGEST_PAGE_LEN {
    SOURCE_PAGE.len()
} else {
    LARGEST_PAGE_LEN
};

pub const PAGE_PACKED_RESPONSE_LEN: usize = match packed_binary_len(LARGEST_SINGLE_WINDOW_PAGE_LEN)
{
    Some(len) => len,
    None => panic!("node page exceeds MessagePack binary limits"),
};
pub const PAGE_RESPONSE_TRANSFER_BYTES: usize =
    sealed_transfer_bytes(RESPONSE_WIRE_OVERHEAD + PAGE_PACKED_RESPONSE_LEN);

// Compatibility aliases for storage profiles and downstream users.
pub const INDEX_PACKED_RESPONSE_LEN: usize = PAGE_PACKED_RESPONSE_LEN;
pub const INDEX_RESPONSE_TRANSFER_BYTES: usize = PAGE_RESPONSE_TRANSFER_BYTES;

pub struct NoSourceNodeIndexPage;

impl<S> RequestEndpoint<S> for NoSourceNodeIndexPage {
    const ENDPOINT_ID: &'static str = INDEX_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond_static_messagepack_bytes(HOPSPOT_INDEX_PAGE_NO_SOURCE)
    }
}

pub struct NodeQuickstartPage;

impl<S> RequestEndpoint<S> for NodeQuickstartPage {
    const ENDPOINT_ID: &'static str = QUICKSTART_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond_static_messagepack_bytes(QUICKSTART_PAGE)
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceNodeIndexPage;

#[cfg(feature = "source-archive")]
impl<S> RequestEndpoint<S> for SourceNodeIndexPage {
    const ENDPOINT_ID: &'static str = INDEX_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond_static_messagepack_bytes(HOPSPOT_INDEX_PAGE_WITH_SOURCE)
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceArchiveFile;

#[cfg(feature = "source-archive")]
impl<S> RequestEndpoint<S> for SourceArchiveFile {
    const ENDPOINT_ID: &'static str = SOURCE_ARCHIVE_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond_static_file("source.zip", SOURCE_ARCHIVE)
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceChecksumFile;

#[cfg(feature = "source-archive")]
impl<S> RequestEndpoint<S> for SourceChecksumFile {
    const ENDPOINT_ID: &'static str = SOURCE_CHECKSUM_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond_static_file("source.zip.sha256", SOURCE_CHECKSUM)
    }
}

pub struct NoSourceNodePageRoutes;

impl<S> RequestEndpointSet<S> for NoSourceNodePageRoutes {
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[
        (INDEX_PATH, RequestEndpointPolicy::AllowAll),
        (QUICKSTART_PATH, RequestEndpointPolicy::AllowAll),
        (COMING_FROM_RNS_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_PAGE_PATH, RequestEndpointPolicy::AllowAll),
    ];

    async fn dispatch(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        if path_hash == RequestPathHash::of(INDEX_PATH) {
            context.respond_static_messagepack_bytes(HOPSPOT_INDEX_PAGE_NO_SOURCE)
        } else if path_hash == RequestPathHash::of(QUICKSTART_PATH) {
            context.respond_static_messagepack_bytes(QUICKSTART_PAGE)
        } else if path_hash == RequestPathHash::of(COMING_FROM_RNS_PATH) {
            context.respond_static_messagepack_bytes(COMING_FROM_RNS_PAGE)
        } else if path_hash == RequestPathHash::of(SOURCE_PAGE_PATH) {
            context.respond_static_messagepack_bytes(SOURCE_PAGE)
        } else {
            Err(Decline::Ignore)
        }
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceNodePageRoutes;

#[cfg(feature = "source-archive")]
impl<S> RequestEndpointSet<S> for SourceNodePageRoutes {
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[
        (INDEX_PATH, RequestEndpointPolicy::AllowAll),
        (QUICKSTART_PATH, RequestEndpointPolicy::AllowAll),
        (COMING_FROM_RNS_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_PAGE_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_ARCHIVE_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_CHECKSUM_PATH, RequestEndpointPolicy::AllowAll),
    ];

    async fn dispatch(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        if path_hash == RequestPathHash::of(INDEX_PATH) {
            context.respond_static_messagepack_bytes(HOPSPOT_INDEX_PAGE_WITH_SOURCE)
        } else if path_hash == RequestPathHash::of(QUICKSTART_PATH) {
            context.respond_static_messagepack_bytes(QUICKSTART_PAGE)
        } else if path_hash == RequestPathHash::of(COMING_FROM_RNS_PATH) {
            context.respond_static_messagepack_bytes(COMING_FROM_RNS_PAGE)
        } else if path_hash == RequestPathHash::of(SOURCE_PAGE_PATH) {
            context.respond_static_messagepack_bytes(SOURCE_PAGE)
        } else if path_hash == RequestPathHash::of(SOURCE_ARCHIVE_PATH) {
            context.respond_static_file("source.zip", SOURCE_ARCHIVE)
        } else if path_hash == RequestPathHash::of(SOURCE_CHECKSUM_PATH) {
            context.respond_static_file("source.zip.sha256", SOURCE_CHECKSUM)
        } else {
            Err(Decline::Ignore)
        }
    }
}

pub struct BrowserNodePageRoutes;

impl<S> RequestEndpointSet<S> for BrowserNodePageRoutes {
    #[cfg(feature = "source-archive")]
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[
        (INDEX_PATH, RequestEndpointPolicy::AllowAll),
        (QUICKSTART_PATH, RequestEndpointPolicy::AllowAll),
        (COMING_FROM_RNS_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_PAGE_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_ARCHIVE_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_CHECKSUM_PATH, RequestEndpointPolicy::AllowAll),
    ];
    #[cfg(not(feature = "source-archive"))]
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[
        (INDEX_PATH, RequestEndpointPolicy::AllowAll),
        (QUICKSTART_PATH, RequestEndpointPolicy::AllowAll),
        (COMING_FROM_RNS_PATH, RequestEndpointPolicy::AllowAll),
        (SOURCE_PAGE_PATH, RequestEndpointPolicy::AllowAll),
    ];

    async fn dispatch(
        mut context: RequestContext<'_, S>,
        _node: &impl PrnsNodeApi,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        if path_hash == RequestPathHash::of(INDEX_PATH) {
            return context.respond_static_messagepack_bytes(BROWSER_INDEX_PAGE);
        }
        if path_hash == RequestPathHash::of(QUICKSTART_PATH) {
            return context.respond_static_messagepack_bytes(QUICKSTART_PAGE);
        }
        if path_hash == RequestPathHash::of(COMING_FROM_RNS_PATH) {
            return context.respond_static_messagepack_bytes(COMING_FROM_RNS_PAGE);
        }
        if path_hash == RequestPathHash::of(SOURCE_PAGE_PATH) {
            return context.respond_static_messagepack_bytes(SOURCE_PAGE);
        }
        #[cfg(feature = "source-archive")]
        {
            if path_hash == RequestPathHash::of(SOURCE_ARCHIVE_PATH) {
                return context.respond_static_file("source.zip", SOURCE_ARCHIVE);
            }
            if path_hash == RequestPathHash::of(SOURCE_CHECKSUM_PATH) {
                return context.respond_static_file("source.zip.sha256", SOURCE_CHECKSUM);
            }
        }
        Err(Decline::Ignore)
    }
}

pub struct NodeIndexPage;

impl<S> RequestEndpoint<S> for NodeIndexPage {
    const ENDPOINT_ID: &'static str = INDEX_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        context: RequestContext<'_, S>,
        node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        #[cfg(feature = "source-archive")]
        {
            SourceNodeIndexPage::handle(context, node).await
        }
        #[cfg(not(feature = "source-archive"))]
        {
            NoSourceNodeIndexPage::handle(context, node).await
        }
    }
}

/// The capability-bound route set used by platform recipes. It remains a constructible unit value
/// while delegating to the source-serving or constrained type selected by the build fact.
pub struct NodePageRoutes;

impl<S> RequestEndpointSet<S> for NodePageRoutes {
    #[cfg(feature = "source-archive")]
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
        <SourceNodePageRoutes as RequestEndpointSet<S>>::REGISTRATIONS;
    #[cfg(not(feature = "source-archive"))]
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
        <NoSourceNodePageRoutes as RequestEndpointSet<S>>::REGISTRATIONS;

    async fn dispatch(
        context: RequestContext<'_, S>,
        node: &impl PrnsNodeApi,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        #[cfg(feature = "source-archive")]
        {
            SourceNodePageRoutes::dispatch(context, node, path_hash).await
        }
        #[cfg(not(feature = "source-archive"))]
        {
            NoSourceNodePageRoutes::dispatch(context, node, path_hash).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_single_window_capacity_covers_every_primary_page() {
        assert!(LARGEST_SINGLE_WINDOW_PAGE_LEN >= HOPSPOT_INDEX_PAGE.len());
        assert!(LARGEST_SINGLE_WINDOW_PAGE_LEN >= BROWSER_INDEX_PAGE.len());
        assert!(LARGEST_SINGLE_WINDOW_PAGE_LEN >= QUICKSTART_PAGE.len());
        assert!(LARGEST_SINGLE_WINDOW_PAGE_LEN >= SOURCE_PAGE.len());
        assert!(COMING_FROM_RNS_PAGE.len() > LARGEST_SINGLE_WINDOW_PAGE_LEN);
        assert_eq!(
            PAGE_PACKED_RESPONSE_LEN,
            packed_binary_len(LARGEST_SINGLE_WINDOW_PAGE_LEN).unwrap()
        );
        assert_eq!(
            PAGE_RESPONSE_TRANSFER_BYTES,
            sealed_transfer_bytes(RESPONSE_WIRE_OVERHEAD + PAGE_PACKED_RESPONSE_LEN)
        );
        assert_eq!(INDEX_PACKED_RESPONSE_LEN, PAGE_PACKED_RESPONSE_LEN);
        assert_eq!(INDEX_RESPONSE_TRANSFER_BYTES, PAGE_RESPONSE_TRANSFER_BYTES);
    }

    #[test]
    fn each_flavor_names_what_serves_it() {
        let hopspot = core::str::from_utf8(HOPSPOT_INDEX_PAGE).unwrap();
        let browser = core::str::from_utf8(BROWSER_INDEX_PAGE).unwrap();
        assert!(hopspot.contains("This node is a Personal Hopspot"));
        assert!(!hopspot.contains("browser tab"));
        assert!(browser.contains("This node lives in a browser tab"));
        assert!(!browser.contains("is a Personal Hopspot"));
        assert!(hopspot.contains("one small piece of that future"));
        assert!(browser.contains("one small piece of that future"));
    }

    #[test]
    fn route_registration_and_page_language_share_one_capability() {
        let page = core::str::from_utf8(HOPSPOT_INDEX_PAGE).unwrap();
        let routes = <NodePageRoutes as RequestEndpointSet<()>>::REGISTRATIONS;
        assert!(routes.iter().any(|(path, _)| *path == INDEX_PATH));
        assert!(routes.iter().any(|(path, _)| *path == QUICKSTART_PATH));
        assert!(routes.iter().any(|(path, _)| *path == COMING_FROM_RNS_PATH));
        assert!(routes.iter().any(|(path, _)| *path == SOURCE_PAGE_PATH));
        assert!(page.contains("`[Coming from RNS?`:/page/coming-from-rns.mu]"));
        assert!(page.contains("`[Download the source`:/page/source.mu]"));
        assert!(!page.contains("Offline quickstart"));
        assert!(page.contains("Mesh networking that's yours"));
        assert!(page.contains(">>`!Why Prns?`!"));
        assert!(page.contains("`[Get the source`:/page/source.mu]"));
        assert_eq!(
            routes.iter().any(|(path, _)| *path == SOURCE_ARCHIVE_PATH),
            SERVES_SOURCE_ARCHIVE
        );
        assert_eq!(
            routes.iter().any(|(path, _)| *path == SOURCE_CHECKSUM_PATH),
            SERVES_SOURCE_ARCHIVE
        );
        let source_page = core::str::from_utf8(SOURCE_PAGE).unwrap();
        assert_eq!(
            source_page.contains("`[source.zip ("),
            SERVES_SOURCE_ARCHIVE
        );
        assert_eq!(
            source_page.contains(
                "Embedded release firmware does not carry the multi-megabyte source archive",
            ),
            !SERVES_SOURCE_ARCHIVE
        );
        assert_eq!(
            source_page.contains("Source commit:"),
            SERVES_SOURCE_ARCHIVE
        );
        if SERVES_SOURCE_ARCHIVE {
            assert!(source_page.contains(&BUILD_COMMIT[..12]));
        }

        let coming_from_rns = core::str::from_utf8(COMING_FROM_RNS_PAGE).unwrap();
        assert!(coming_from_rns.contains(">Coming from RNS"));
        assert!(coming_from_rns.contains("Your config, your identity file, and your apps"));
        assert!(coming_from_rns.contains("`[Back to index`:/page/index.mu]"));
    }

    #[test]
    fn the_browser_face_carries_the_shared_nav_and_pages() {
        let browser = core::str::from_utf8(BROWSER_INDEX_PAGE).unwrap();
        assert!(browser.contains("`[Coming from RNS?`:/page/coming-from-rns.mu]"));
        assert!(browser.contains("`[Download the source`:/page/source.mu]"));
        assert!(browser.contains("`[Get the source`:/page/source.mu]"));
        assert!(!browser.contains("Open the offline Prns quickstart"));
        assert!(!browser.contains("source.zip not carried or served"));

        let routes = <BrowserNodePageRoutes as RequestEndpointSet<()>>::REGISTRATIONS;
        assert!(routes.iter().any(|(path, _)| *path == COMING_FROM_RNS_PATH));
        assert!(routes.iter().any(|(path, _)| *path == SOURCE_PAGE_PATH));
        assert_eq!(
            routes.iter().any(|(path, _)| *path == SOURCE_ARCHIVE_PATH),
            SERVES_SOURCE_ARCHIVE
        );

        let source_page = core::str::from_utf8(SOURCE_PAGE).unwrap();
        assert_eq!(
            source_page.contains("`[source.zip ("),
            SERVES_SOURCE_ARCHIVE
        );
        assert_eq!(
            source_page.contains(
                "Embedded release firmware does not carry the multi-megabyte source archive",
            ),
            !SERVES_SOURCE_ARCHIVE
        );
        assert_eq!(
            source_page.contains("Source commit:"),
            SERVES_SOURCE_ARCHIVE
        );
        if SERVES_SOURCE_ARCHIVE {
            assert!(source_page.contains(&BUILD_COMMIT[..12]));
        }
        assert!(!source_page.contains("{{"));
        assert!(!source_page.contains("prnsd:managed"));

        let coming_from_rns = core::str::from_utf8(COMING_FROM_RNS_PAGE).unwrap();
        assert!(coming_from_rns.contains(">Coming from RNS"));
    }

    #[test]
    fn the_pages_are_balanced_micron() {
        for page in [HOPSPOT_INDEX_PAGE, BROWSER_INDEX_PAGE, QUICKSTART_PAGE] {
            let page = core::str::from_utf8(page).unwrap();
            assert!(!page.is_ascii());
            assert!(page.lines().all(|line| line.len() <= 600));
        }
        for page in [
            HOPSPOT_INDEX_PAGE,
            BROWSER_INDEX_PAGE,
            QUICKSTART_PAGE,
            COMING_FROM_RNS_PAGE,
            SOURCE_PAGE,
        ] {
            let page = core::str::from_utf8(page).unwrap();
            let mut formatting_toggles = 0usize;
            for line in page.lines() {
                formatting_toggles += line.matches("`!").count();
                assert!(!line.contains('\t'));
            }
            assert_eq!(formatting_toggles % 2, 0);
            assert_eq!(page.matches("`c").count(), page.matches("`a").count());
        }
    }

    #[test]
    fn the_quickstart_covers_each_first_outcome() {
        let page = core::str::from_utf8(QUICKSTART_PAGE).unwrap();
        for expected in [
            "cargo prnsd",
            "cargo tools guide rust-basics",
            "cargo c6 --locked",
            "cargo test --locked",
            "cargo benchmark --smoke",
            "cargo run -p docs",
        ] {
            assert!(page.contains(expected), "quickstart is missing {expected}");
        }
        assert!(page.contains("same node-recipe API"));
        assert!(page.contains("source clone or source.zip"));
        assert!(page.contains("Both built-in page variants include this guide"));
        assert!(page.contains("compact nodes may provide it without carrying source.zip"));
        assert!(page.contains(INDEX_PATH));
        assert!(QUICKSTART_PAGE.len() < LARGEST_INDEX_PAGE_LEN);
    }
}
