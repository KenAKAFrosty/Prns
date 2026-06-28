mod benchmarks;
mod browser_hopspot;
mod contributing;
mod crates;
mod flash;
mod landing;
mod not_found;
mod platforms;

pub use benchmarks::{BenchmarksHostPage, BenchmarksPage};
pub use browser_hopspot::BrowserHopspotPage;
pub use contributing::ContributingPage;
pub use crates::{CratesIndex, SingleCrate};
pub use flash::{FlashBoardPage, FlashPage};
pub use landing::Landing;
pub use not_found::NotFound;
pub use platforms::PlatformsPage;
