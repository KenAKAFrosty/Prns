use dioxus::prelude::*;

/// The Prns mark: an announcing node centered in a reticle ring (reticle ×
/// announce). Rendered inline so it stays crisp at nav size and ships with the
/// initial HTML. Keep in sync with `public/assets/prns-mark.svg`.
#[component]
pub fn PrnsMark(#[props(default = 22)] size: u32) -> Element {
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            view_box: "0 0 100 100",
            role: "img",
            "aria-label": "Prns",
            circle { cx: "50", cy: "50", r: "37", fill: "none", stroke: "#6ee7b7", stroke_width: "3" }
            g { stroke: "#6ee7b7", stroke_width: "3", stroke_linecap: "round", transform: "rotate(46 50 50)",
                line { x1: "50", y1: "7", x2: "50", y2: "16" }
                line { x1: "50", y1: "84", x2: "50", y2: "93" }
                line { x1: "7", y1: "50", x2: "16", y2: "50" }
                line { x1: "84", y1: "50", x2: "93", y2: "50" }
            }
            g { fill: "none", stroke: "#6ee7b7", stroke_linecap: "round", stroke_width: "2.6",
                path { d: "M57.46 39.35 A13 13 0 0 1 57.46 60.65", opacity: "0.9" }
                path { d: "M62.04 32.8 A21 21 0 0 1 62.04 67.2", opacity: "0.45" }
                path { d: "M42.54 39.35 A13 13 0 0 0 42.54 60.65", opacity: "0.9" }
                path { d: "M37.96 32.8 A21 21 0 0 0 37.96 67.2", opacity: "0.45" }
            }
            circle { cx: "50", cy: "50", r: "6", fill: "#34d399" }
        }
    }
}
