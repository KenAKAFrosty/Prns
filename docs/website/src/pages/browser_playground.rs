use dioxus::prelude::*;
#[component]
pub fn BrowserPlaygroundPage() -> Element {
    rsx! {
        section { class: "browser-playground-frame -mx-4 md:-mx-8 lg:-mx-10",
            iframe {
                title: "Prns Browser Node Playground console",
                src: "/browser-node-playground-console/",
                allow: "usb; bluetooth",
                style: "height: calc(100vh - 8rem); min-height: 38rem;",
                class: "block w-full border-0 bg-ink",
            }
        }
    }
}
