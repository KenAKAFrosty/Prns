pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const COMMIT: &str = env!("PRNS_GIT_COMMIT");
#[cfg(test)]
pub(crate) const SHORT_COMMIT: &str = env!("PRNS_GIT_COMMIT_SHORT");
pub(crate) const NNPAGES_ANNOUNCE_TITLE: &str = concat!(
    "Prns: High-performance Reticulum · v",
    env!("CARGO_PKG_VERSION"),
    " · ",
    env!("PRNS_GIT_COMMIT_SHORT")
);
