//! Debug tool: load a page, resolve styles, and print the computed
//! `display`/`float`/`position`/`width` of every element (or one tag).
//! Usage: dump_style <url> [tag-filter]

use incognidium_css::parse_css;
use incognidium_html::parse_html;
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

fn fmt_metrics(st: &incognidium_style::ComputedStyle) -> String {
    let pt_pct = st
        .padding_top_percent
        .map(|p| format!("({}%)", p * 100.0))
        .unwrap_or_default();
    let pb_pct = st
        .padding_bottom_percent
        .map(|p| format!("({}%)", p * 100.0))
        .unwrap_or_default();
    let pl_pct = st
        .padding_left_percent
        .map(|p| format!("({}%)", p * 100.0))
        .unwrap_or_default();
    let pr_pct = st
        .padding_right_percent
        .map(|p| format!("({}%)", p * 100.0))
        .unwrap_or_default();
    format!(
        "h={:?} min-h={:?} pt={}{} pb={}{} pl={}{} pr={}{} mt={} mb={} top={:?}",
        st.height,
        st.min_height,
        st.padding_top,
        pt_pct,
        st.padding_bottom,
        pb_pct,
        st.padding_left,
        pl_pct,
        st.padding_right,
        pr_pct,
        st.margin_top,
        st.margin_bottom,
        st.top,
    )
}

fn print_node(
    doc: &incognidium_dom::Document,
    node_id: incognidium_dom::NodeId,
    styles: &std::collections::HashMap<incognidium_dom::NodeId, incognidium_style::ComputedStyle>,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }
    let node = doc.node(node_id);
    if let incognidium_dom::NodeData::Element(ref el) = node.data {
        if let Some(st) = styles.get(&node_id) {
            let cls = el.get_attr("class").unwrap_or_default();
            let grid_cols = if st.grid_template_columns.is_empty() {
                "none".to_string()
            } else {
                format!("{} tracks", st.grid_template_columns.len())
            };
            let grid_col = st
                .grid_column_start
                .as_ref()
                .map(|_| "start")
                .unwrap_or("")
                .to_string()
                + st.grid_column_end.as_ref().map(|_| "end").unwrap_or("");
            let grid_col = if grid_col.is_empty() { "" } else { &grid_col };
            let metrics = fmt_metrics(st);
            println!(
                "{:indent$}node={} tag={} class=[{}] display={:?} float={:?} pos={:?} width={:?} grid={} gc={} span={} {}",
                "",
                node_id,
                el.tag_name,
                cls,
                st.display,
                st.float,
                st.position,
                st.width,
                grid_cols,
                grid_col,
                st.grid_column_span.map(|s| s.to_string()).unwrap_or_else(|| "-".to_string()),
                metrics,
                indent = depth * 2,
            );
        }
    }
    if depth < max_depth {
        for &child_id in &node.children {
            print_node(doc, child_id, styles, depth + 1, max_depth);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: dump_style <url> [tag-filter|node-id] [--subtree N]");
        return;
    }
    let url = args[1].clone();
    let mut tag_filter: Option<String> = None;
    let mut subtree_root: Option<incognidium_dom::NodeId> = None;
    let mut subtree_depth: usize = 0;
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--subtree" {
            if i + 2 < args.len() {
                subtree_root = args[i + 1].parse().ok();
                subtree_depth = args[i + 2].parse().unwrap_or(3);
                i += 3;
                continue;
            }
        }
        if tag_filter.is_none() && subtree_root.is_none() {
            if let Ok(nid) = args[i].parse() {
                subtree_root = Some(nid);
                subtree_depth = 3;
            } else {
                tag_filter = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let resp = fetch_url(&url).expect("fetch failed");
    let doc = parse_html(&resp.body);
    let mut css_text = fetch_external_css(&doc, &url);
    css_text.push_str(&doc.collect_style_text());
    let css_text = incognidium_shell::strip_dark_mode_media_queries(&css_text);
    let stylesheet = parse_css(&css_text);
    eprintln!("Parsed {} CSS rules", stylesheet.rules.len());
    let viewport_width = 1024.0f32;
    let styles = resolve_styles(&doc, &stylesheet, viewport_width, 2000.0);

    if let Some(root) = subtree_root {
        // Print ancestors first
        let mut ancestors = Vec::new();
        let mut cur = Some(root);
        while let Some(id) = cur {
            ancestors.push(id);
            cur = doc.node(id).parent;
        }
        for &aid in ancestors.iter().rev() {
            print_node(&doc, aid, &styles, 0, 0);
        }
        println!("--- subtree ---");
        print_node(&doc, root, &styles, 0, subtree_depth);
        return;
    }

    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if let Some(ref f) = tag_filter {
                if el.tag_name != *f {
                    continue;
                }
            }
            if let Some(st) = styles.get(&node.id) {
                let cls = el.get_attr("class").unwrap_or_default();
                let grid_cols = if st.grid_template_columns.is_empty() {
                    "none".to_string()
                } else {
                    format!("{} tracks", st.grid_template_columns.len())
                };
                let grid_col = st
                    .grid_column_start
                    .as_ref()
                    .map(|_| "start")
                    .unwrap_or("")
                    .to_string()
                    + st.grid_column_end.as_ref().map(|_| "end").unwrap_or("");
                let grid_col = if grid_col.is_empty() { "" } else { &grid_col };
                let metrics = fmt_metrics(st);
                println!(
                    "node={} tag={} class=[{}] display={:?} float={:?} pos={:?} width={:?} grid={} gc={} span={} {}",
                    node.id, el.tag_name, cls, st.display, st.float, st.position, st.width,
                    grid_cols, grid_col,
                    st.grid_column_span.map(|s| s.to_string()).unwrap_or_else(|| "-".to_string()),
                    metrics,
                );
            }
        }
    }
}
