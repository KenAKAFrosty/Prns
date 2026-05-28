//! reticulum.rs — the public docs site for the Personal Reticulum Suite.
//!
//! Dioxus 0.7 SSG + dioxus-i18n. Each route pre-renders to HTML at build time;
//! the client hydrates for interactivity (language switcher, in-page nav).

use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::langid;

mod components;
mod pages;
mod routes;

use routes::Route;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    use_init_i18n(|| {
        I18nConfig::new(langid!("en-US"))
            .with_locale(Locale::new_static(
                langid!("en-US"),
                include_str!("../i18n/en-US.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("de-DE"),
                include_str!("../i18n/de-DE.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("es-ES"),
                include_str!("../i18n/es-ES.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("fr-FR"),
                include_str!("../i18n/fr-FR.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("ja-JP"),
                include_str!("../i18n/ja-JP.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("pt-BR"),
                include_str!("../i18n/pt-BR.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("zh-CN"),
                include_str!("../i18n/zh-CN.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("da-DK"),
                include_str!("../i18n/da-DK.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("it-IT"),
                include_str!("../i18n/it-IT.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("ko-KR"),
                include_str!("../i18n/ko-KR.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("nb-NO"),
                include_str!("../i18n/nb-NO.ftl"),
            ))
            .with_locale(Locale::new_static(
                langid!("sv-SE"),
                include_str!("../i18n/sv-SE.ftl"),
            ))
    });

    rsx! {
        document::Meta { name: "viewport", content: "width=device-width, initial-scale=1" }
        document::Meta {
            name: "description",
            content: "A production-grade Rust port of Reticulum (RNS). Truly pure no_std, alloc-free core. Drop-in for rnsd. First-class Kotlin, Swift, and Python bindings. Built for microcontrollers, mobile, and cloud.",
        }
        Router::<Route> {}
    }
}
