use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::LanguageIdentifier;

const LANGUAGES: &[(&str, &str, &str)] = &[
    ("en-US", "EN", "English"),
    ("de-DE", "DE", "Deutsch"),
    ("es-ES", "ES", "Español"),
    ("fr-FR", "FR", "Français"),
    ("ja-JP", "JA", "日本語"),
    ("pt-BR", "PT", "Português"),
    ("zh-CN", "ZH", "简体中文"),
];

fn matches_current(code: &str, current: &LanguageIdentifier) -> bool {
    code.parse::<LanguageIdentifier>().ok().as_ref() == Some(current)
}

fn short_for(current: &LanguageIdentifier) -> &'static str {
    LANGUAGES
        .iter()
        .find_map(|(code, short, _)| matches_current(code, current).then_some(*short))
        .unwrap_or("EN")
}

#[component]
pub fn LanguageSwitcher() -> Element {
    let mut i18n = i18n();
    let mut menu_open = use_signal(|| false);

    let current = i18n.language();
    let current_short = short_for(&current);

    rsx! {
        div { class: "relative",
            button {
                r#type: "button",
                class: "inline-flex items-center gap-2 rounded-full border border-line/70 bg-layer/60 px-3 py-1.5 text-xs font-semibold tracking-wider text-soft hover:text-accent hover:border-accent/40 transition-colors",
                onclick: move |_| menu_open.set(!menu_open()),
                span { "🌐" }
                span { "{current_short}" }
                span { class: "text-mid", if menu_open() { "▴" } else { "▾" } }
            }
            if menu_open() {
                div { class: "absolute right-0 top-[calc(100%+0.5rem)] min-w-48 flex flex-col rounded-xl border border-line/70 bg-surface/95 backdrop-blur-md shadow-card py-1 z-40",
                    for (code, short, label) in LANGUAGES.iter() {
                        {
                            let code = *code;
                            let is_current = matches_current(code, &current);
                            let row_class = if is_current {
                                "flex items-center gap-3 px-3 py-2 text-sm text-accent bg-accent/10"
                            } else {
                                "flex items-center gap-3 px-3 py-2 text-sm text-soft hover:text-paper hover:bg-layer/80 transition-colors"
                            };
                            rsx! {
                                button {
                                    key: "{code}",
                                    r#type: "button",
                                    class: row_class,
                                    onclick: move |_| {
                                        if let Ok(lang) = code.parse::<LanguageIdentifier>() {
                                            i18n.set_language(lang);
                                            menu_open.set(false);
                                        }
                                    },
                                    span { class: "w-6 text-[0.7rem] font-bold tracking-widest", "{short}" }
                                    span { class: "flex-1 text-left", "{label}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
