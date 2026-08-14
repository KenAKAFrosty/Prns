mod benchmarks;
mod configure;
mod flash;
mod landing;
mod not_found;
mod platforms;

pub use benchmarks::{BenchmarksHostPage, BenchmarksPage};
pub use configure::ConfigurePage;
pub use flash::{FlashBoardPage, FlashPage};
pub use landing::Landing;
pub use not_found::NotFound;
pub use platforms::PlatformsPage;
