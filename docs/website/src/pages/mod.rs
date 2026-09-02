mod benchmarks;
mod flash;
mod landing;
mod not_found;
mod platforms;

pub(crate) use benchmarks::HOST_PAGES;
pub use benchmarks::{BenchmarksHostPage, BenchmarksPage};
pub use flash::{FlashBoardPage, FlashPage};
pub use landing::Landing;
pub use not_found::NotFound;
pub use platforms::PlatformsPage;
