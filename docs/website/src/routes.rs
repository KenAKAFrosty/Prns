use dioxus::prelude::*;

use crate::components::Shell;
use crate::pages::{
    BenchmarksHostPage, BenchmarksPage, ContributingPage, CratesIndex, Landing, NotFound,
    PlatformsPage, SingleCrate,
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

        #[route("/benchmarks")]
        BenchmarksPage {},

        #[route("/benchmarks/:host")]
        BenchmarksHostPage { host: String },

        #[route("/crates/:name")]
        SingleCrate { name: String },

        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}
