use dioxus::prelude::*;

#[cfg(all(feature = "browser-test-fixture", feature = "local-dev-flasher"))]
compile_error!("local-dev-flasher is forbidden in browser-test-fixture builds");

use dioxus_i18n::prelude::*;
use unic_langid::langid;

mod components;
mod links;
mod local_development;
mod pages;
mod platforms;
mod repository_docs;
mod routes;
pub mod seo;

use routes::Route;

#[server(endpoint = "static_routes", output = server_fn::codec::Json)]
async fn static_routes_endpoint() -> Result<Vec<String>, ServerFnError> {
    Ok(seo::static_route_paths())
}

#[component]
pub fn App() -> Element {
    use_init_i18n(|| {
        I18nConfig::new(langid!("en-US"))
            .with_fallback(langid!("en-US"))
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
        Router::<Route> {}
    }
}
