use dioxus::prelude::*;

use crate::components::Shell;
use crate::pages::{
    BenchmarksHostPage, BenchmarksPage, ContributingPage, CratesIndex, FlashBoardPage, FlashPage,
    GuidesIndex, Landing, NotFound, PlatformsPage, SingleCrate,
};

#[derive(Clone, Routable, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Landing {},

        #[route("/contributing")]
        ContributingPage {},

        #[route("/guides")]
        GuidesIndex {},

        #[route("/crates")]
        CratesIndex {},

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

        #[route("/crates/:name")]
        SingleCrate { name: String },

        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}
