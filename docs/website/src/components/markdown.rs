use std::collections::HashSet;

use dioxus::prelude::*;
use pulldown_cmark::{html, Event, Options, Parser, Tag, TagEnd};

#[component]
pub fn MarkdownBody(source: String) -> Element {
    let html_string = rendered_markup(&source);

    rsx! {
        article {
            class: "prose",
            dangerous_inner_html: "{html_string}",
        }
    }
}

fn rendered_markup(source: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    let mut events: Vec<Event> = Parser::new_ext(source, opts).collect();
    assign_heading_ids(&mut events);
    let mut html_string = String::with_capacity(source.len() * 2);
    html::push_html(&mut html_string, events.into_iter());
    html_string
}

fn assign_heading_ids(events: &mut [Event]) {
    let mut used = HashSet::new();
    for index in 0..events.len() {
        let Event::Start(Tag::Heading { id: None, .. }) = &events[index] else {
            continue;
        };
        let mut text = String::new();
        for event in &events[index + 1..] {
            match event {
                Event::Text(content) | Event::Code(content) => text.push_str(content),
                Event::End(TagEnd::Heading(_)) => break,
                _ => {}
            }
        }
        let Some(slug) = heading_slug(&text, &mut used) else {
            continue;
        };
        if let Event::Start(Tag::Heading { id, .. }) = &mut events[index] {
            *id = Some(slug.into());
        }
    }
}

fn heading_slug(text: &str, used: &mut HashSet<String>) -> Option<String> {
    let mut base = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            base.extend(ch.to_lowercase());
        } else if ch == ' ' || ch == '-' || ch == '_' {
            base.push(if ch == ' ' { '-' } else { ch });
        }
    }
    if base.is_empty() {
        return None;
    }
    let mut slug = base.clone();
    let mut counter = 1;
    while !used.insert(slug.clone()) {
        slug = format!("{base}-{counter}");
        counter += 1;
    }
    Some(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_receive_github_style_ids() {
        let markup = rendered_markup(
            "## Path/Route (and hops)\n\n## Packet\n\n### A packet on your phone could\n",
        );
        assert!(markup.contains("id=\"pathroute-and-hops\""));
        assert!(markup.contains("id=\"packet\""));
        assert!(markup.contains("id=\"a-packet-on-your-phone-could\""));
    }

    #[test]
    fn duplicate_headings_get_numbered_ids() {
        let markup = rendered_markup("## Packet\n\n## Packet\n\n## Packet\n");
        assert!(markup.contains("id=\"packet\""));
        assert!(markup.contains("id=\"packet-1\""));
        assert!(markup.contains("id=\"packet-2\""));
    }

    #[test]
    fn explicit_heading_attributes_are_preserved() {
        let markup = rendered_markup("## Custom {#chosen-name}\n");
        assert!(markup.contains("id=\"chosen-name\""));
        assert!(!markup.contains("id=\"custom\""));
    }
}
