use dioxus::prelude::*;
use pulldown_cmark::{html, Options, Parser};

#[component]
pub fn MarkdownBody(source: &'static str) -> Element {
    let html_string = use_memo(move || {
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_FOOTNOTES);
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
        let parser = Parser::new_ext(source, opts);
        let mut out = String::with_capacity(source.len() * 2);
        html::push_html(&mut out, parser);
        out
    });

    rsx! {
        article {
            class: "prose",
            dangerous_inner_html: "{html_string}",
        }
    }
}
