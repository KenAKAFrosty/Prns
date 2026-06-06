use dioxus::prelude::*;
use pulldown_cmark::{html, Options, Parser};

#[component]
pub fn MarkdownBody(source: String) -> Element {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    let parser = Parser::new_ext(&source, opts);
    let mut html_string = String::with_capacity(source.len() * 2);
    html::push_html(&mut html_string, parser);

    rsx! {
        article {
            class: "prose",
            dangerous_inner_html: "{html_string}",
        }
    }
}
