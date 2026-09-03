//! Debug tool: load a page, resolve styles, and print the computed
//! `display`/`float`/`position`/`width` of every element (or one tag).
//! Usage: dump_style <url> [tag-filter]

use std::collections::HashMap;

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{layout_with_images, ImageSizes};
use incognidium_net::{fetch_url, resolve_url};
use incognidium_style::resolve_styles;

fn fetch_external_css(doc: &incognidium_dom::Document, base_url: &str) -> String {
    let mut css = String::new();
    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "link" {
                let is_stylesheet = el
                    .get_attr("rel")
                    .map(|r| {
                        r.split_whitespace()
                            .any(|t| t.eq_ignore_ascii_case("stylesheet"))
                    })
                    .unwrap_or(false);
                if is_stylesheet {
                    // Wrap gated rules in their @media gate so they evaluate
                    // exactly like an @media rule.
                    let gate = el
                        .get_attr("media")
                        .map(|m| m.trim())
                        .filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("all"))
                        .map(|m| m.to_string());
                    if let Some(href) = el.get_attr("href") {
                        if let Ok(resolved) = resolve_url(base_url, href) {
                            if let Ok(resp) = fetch_url(&resolved) {
                                match gate.as_deref() {
                                    Some(m) => {
                                        css.push_str(&format!("@media {} {{\n", m));
                                        css.push_str(&resp.body);
                                        css.push_str("\n}\n");
                                    }
                                    None => {
                                        css.push_str(&resp.body);
                                        css.push('\n');
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    css
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args[1].clone();
    let tag_filter = args.get(2).cloned();

    let resp = fetch_url(&url).expect("fetch failed");
    let doc = parse_html(&resp.body);
    let mut css_text = fetch_external_css(&doc, &url);
    css_text.push_str(&doc.collect_style_text());
    let css_text = incognidium_shell::strip_dark_mode_media_queries(&css_text);
    let stylesheet = parse_css(&css_text);
    eprintln!("Parsed {} CSS rules", stylesheet.rules.len());
    let viewport_width = 1024.0f32;
    let styles = resolve_styles(&doc, &stylesheet, viewport_width, 2000.0);

    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if let Some(ref f) = tag_filter {
                if el.tag_name != *f {
                    continue;
                }
            }
            if let Some(st) = styles.get(&node.id) {
                let cls = el.get_attr("class").unwrap_or_default();
                println!(
                    "node={} tag={} class=[{}] display={:?} float={:?} pos={:?} width={:?}",
                    node.id, el.tag_name, cls, st.display, st.float, st.position, st.width,
                );
            }
        }
    }
}
