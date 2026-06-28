use dioxus::prelude::*;

use crate::components::Shell;
use crate::pages::{
    BenchmarksHostPage, BenchmarksPage, BrowserHopspotPage, ContributingPage, CratesIndex,
    FlashBoardPage, FlashPage, Landing, NotFound, PlatformsPage, SingleCrate,
};

#[derive(Clone, Routable, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Landing {},

        #[route("/contributing")]
        ContributingPage {},

        #[route("/crates")]
        CratesIndex {},

        #[route("/platforms")]
        PlatformsPage {},

        #[route("/browser-hopspot")]
        BrowserHopspotPage {},

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
