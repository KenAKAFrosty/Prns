mod benchmarks;
mod contributing;
mod crates;
mod landing;
mod not_found;
mod platforms;

pub use benchmarks::{BenchmarksHostPage, BenchmarksPage};
pub use contributing::ContributingPage;
pub use crates::{CratesIndex, SingleCrate};
pub use landing::Landing;
pub use not_found::NotFound;
pub use platforms::PlatformsPage;
