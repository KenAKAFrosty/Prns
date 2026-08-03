use dioxus::prelude::*;

use crate::components::Shell;
use crate::pages::{
    BenchmarksHostPage, BenchmarksPage, FlashBoardPage, FlashPage, Landing, NotFound, PlatformsPage,
};

#[derive(Clone, Routable, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Landing {},

        #[route("/platforms")]
        PlatformsPage {},

        #[route("/flash")]
        FlashPage {},

        #[route("/flash/:board")]
        FlashBoardPage { board: String },

        #[route("/benchmarks")]
        BenchmarksPage {},

        #[route("/benchmarks/:host")]
        BenchmarksHostPage { host: String },

        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}
