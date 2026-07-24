use personal_rns::routing::links::request::{packed_binary_len, RESPONSE_WIRE_OVERHEAD};
use personal_rns::routing::links::resources::sealed_transfer_bytes;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};

include!(concat!(env!("OUT_DIR"), "/node_pages_generated.rs"));

pub const NODE_APP_NAME: &str = "nomadnetwork";
pub const NODE_ASPECTS: &[&str] = &["node"];
pub const INDEX_PATH: &str = "/page/index.mu";
pub const SOURCE_ARCHIVE_PATH: &str = "/file/source.zip";
pub const SOURCE_CHECKSUM_PATH: &str = "/file/source.zip.sha256";

#[cfg(feature = "source-archive")]
pub const SERVES_SOURCE_ARCHIVE: bool = true;
#[cfg(not(feature = "source-archive"))]
pub const SERVES_SOURCE_ARCHIVE: bool = false;

#[cfg(feature = "source-archive")]
pub const HOPSPOT_INDEX_PAGE: &[u8] = HOPSPOT_INDEX_PAGE_WITH_SOURCE;
#[cfg(not(feature = "source-archive"))]
pub const HOPSPOT_INDEX_PAGE: &[u8] = HOPSPOT_INDEX_PAGE_NO_SOURCE;
#[cfg(feature = "source-archive")]
pub const BROWSER_INDEX_PAGE: &[u8] = BROWSER_INDEX_PAGE_WITH_SOURCE;
#[cfg(not(feature = "source-archive"))]
pub const BROWSER_INDEX_PAGE: &[u8] = BROWSER_INDEX_PAGE_NO_SOURCE;

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

pub struct NoSourceNodeIndexPage;

impl<S> RequestRoute<S> for NoSourceNodeIndexPage {
    const PATH: &'static str = INDEX_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, S>) -> Result<(), Decline> {
        context.respond_static_bytes(HOPSPOT_INDEX_PAGE_NO_SOURCE)
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceNodeIndexPage;

#[cfg(feature = "source-archive")]
impl<S> RequestRoute<S> for SourceNodeIndexPage {
    const PATH: &'static str = INDEX_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, S>) -> Result<(), Decline> {
        context.respond_static_bytes(HOPSPOT_INDEX_PAGE_WITH_SOURCE)
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceArchiveFile;

#[cfg(feature = "source-archive")]
impl<S> RequestRoute<S> for SourceArchiveFile {
    const PATH: &'static str = SOURCE_ARCHIVE_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, S>) -> Result<(), Decline> {
        context.respond_static_file("source.zip", SOURCE_ARCHIVE)
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceChecksumFile;

#[cfg(feature = "source-archive")]
impl<S> RequestRoute<S> for SourceChecksumFile {
    const PATH: &'static str = SOURCE_CHECKSUM_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(mut context: RequestContext<'_, S>) -> Result<(), Decline> {
        context.respond_static_file("source.zip.sha256", SOURCE_CHECKSUM)
    }
}

pub struct NoSourceNodePageRoutes;

impl<S> RouteSet<S> for NoSourceNodePageRoutes {
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] =
        &[(INDEX_PATH, RoutePolicy::AllowAll)];

    async fn dispatch(
        context: RequestContext<'_, S>,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        if path_hash == RequestPathHash::of(INDEX_PATH) {
            NoSourceNodeIndexPage::handle(context).await
        } else {
            Err(Decline::Ignore)
        }
    }
}

#[cfg(feature = "source-archive")]
pub struct SourceNodePageRoutes;

#[cfg(feature = "source-archive")]
impl<S> RouteSet<S> for SourceNodePageRoutes {
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] = &[
        (INDEX_PATH, RoutePolicy::AllowAll),
        (SOURCE_ARCHIVE_PATH, RoutePolicy::AllowAll),
        (SOURCE_CHECKSUM_PATH, RoutePolicy::AllowAll),
    ];

    async fn dispatch(
        context: RequestContext<'_, S>,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        if path_hash == RequestPathHash::of(INDEX_PATH) {
            SourceNodeIndexPage::handle(context).await
        } else if path_hash == RequestPathHash::of(SOURCE_ARCHIVE_PATH) {
            SourceArchiveFile::handle(context).await
        } else if path_hash == RequestPathHash::of(SOURCE_CHECKSUM_PATH) {
            SourceChecksumFile::handle(context).await
        } else {
            Err(Decline::Ignore)
        }
    }
}

pub struct NodeIndexPage;

impl<S> RequestRoute<S> for NodeIndexPage {
    const PATH: &'static str = INDEX_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(context: RequestContext<'_, S>) -> Result<(), Decline> {
        #[cfg(feature = "source-archive")]
        {
            SourceNodeIndexPage::handle(context).await
        }
        #[cfg(not(feature = "source-archive"))]
        {
            NoSourceNodeIndexPage::handle(context).await
        }
    }
}

/// The capability-bound route set used by platform recipes. It remains a constructible unit value
/// while delegating to the source-serving or constrained type selected by the build fact.
pub struct NodePageRoutes;

impl<S> RouteSet<S> for NodePageRoutes {
    #[cfg(feature = "source-archive")]
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] =
        <SourceNodePageRoutes as RouteSet<S>>::REGISTRATIONS;
    #[cfg(not(feature = "source-archive"))]
    const REGISTRATIONS: &'static [(&'static str, RoutePolicy)] =
        <NoSourceNodePageRoutes as RouteSet<S>>::REGISTRATIONS;

    async fn dispatch(
        context: RequestContext<'_, S>,
        path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        #[cfg(feature = "source-archive")]
        {
            SourceNodePageRoutes::dispatch(context, path_hash).await
        }
        #[cfg(not(feature = "source-archive"))]
        {
            NoSourceNodePageRoutes::dispatch(context, path_hash).await
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
        let routes = <NodePageRoutes as RouteSet<()>>::REGISTRATIONS;
        assert_eq!(
            routes.iter().any(|(path, _)| *path == SOURCE_ARCHIVE_PATH),
            SERVES_SOURCE_ARCHIVE
        );
        assert_eq!(
            routes.iter().any(|(path, _)| *path == SOURCE_CHECKSUM_PATH),
            SERVES_SOURCE_ARCHIVE
        );
        assert_eq!(page.contains("Download source.zip"), SERVES_SOURCE_ARCHIVE);
        assert_eq!(
            page.contains("source.zip not carried or served"),
            !SERVES_SOURCE_ARCHIVE
        );
    }

    #[test]
    fn the_index_page_is_balanced_micron() {
        for page in [HOPSPOT_INDEX_PAGE, BROWSER_INDEX_PAGE] {
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
