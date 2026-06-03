use dioxus::prelude::*;

use crate::components::Shell;
use crate::pages::{
    BenchmarksPage, CratesIndex, EthosPage, Landing, NotFound, PlatformsPage, SingleCrate,
};

#[derive(Clone, Routable, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Landing {},

        #[route("/ethos")]
        EthosPage {},

        #[route("/crates")]
        CratesIndex {},

        #[route("/platforms")]
        PlatformsPage {},

        #[route("/benchmarks")]
        BenchmarksPage {},

        #[route("/crates/:name")]
        SingleCrate { name: String },

        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}
