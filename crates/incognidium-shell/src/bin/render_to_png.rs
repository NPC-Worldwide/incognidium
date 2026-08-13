/// Render a URL to a PNG file for debugging
use std::collections::HashMap;

use image::GenericImageView;
use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::{fetch_bytes, fetch_url, resolve_url};
use incognidium_paint::{paint_with_images, ImageData};
use incognidium_style::{resolve_styles, resolve_styles_with_containers, ContainerType};

use incognidium_shell::{
    collect_scripts, execute_scripts_on_doc, is_inline_svg_url, rasterize_inline_svgs,
    save_png_compressed,
};

/// Largest dimension we keep for decoded raster images. Downsizing huge source
/// images (e.g. 3840px wide photos on TIME/Vox) saves memory and paint time
/// without affecting a 1024px-wide headless render.
const MAX_IMAGE_DIMENSION: u32 = 2048;

/// True when a flat box is positioned entirely outside the viewport
/// horizontally. Off-canvas hidden menus (e.g. `translateX(-500%)`) and
/// accessibility-only skip links (`left: -10000px`) should not count toward
/// extracted text.
fn is_box_offscreen(fbox: &incognidium_layout::FlatBox, viewport_width: f32) -> bool {
    // Off-canvas to the right: starts beyond the viewport.
    let off_right = fbox.x >= viewport_width;
    // Off-canvas to the left: more than a small margin past the left edge.
    // We keep boxes that are only slightly clipped (e.g. overflow menus placed
    // just off-screen by layout approximations) while dropping true hidden
    // menus and skip links at extreme negative positions.
    let off_left = fbox.x + fbox.width <= -100.0;
    off_right || off_left
}

/// Fallback DOM text extraction used when the layout engine produces very few
/// text boxes. Walks the visible DOM tree in document order, collects text node
/// content, and uses meaningful accessibility attributes (aria-label, title,
/// alt, placeholder) when an element has no rendered child text. Block-level
/// elements are separated by newlines so the result remains readable.
fn extract_dom_text(
    doc: &incognidium_dom::Document,
    styles: &incognidium_style::StyleMap,
    flat_boxes: &[incognidium_layout::FlatBox],
    viewport_width: f32,
) -> String {
    use incognidium_dom::NodeData;
    use incognidium_style::{Display, Visibility};

    // Precompute which nodes have on-screen text boxes. A node that only has
    // off-screen flat boxes is treated as hidden (e.g. off-canvas menus, skip
    // links), so its text does not pollute the fallback extraction.
    let mut node_has_text_box: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();
    let mut node_offscreen_all: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();
    let mut node_text_seen: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();
    for fb in flat_boxes.iter() {
        if fb.text.is_none() || fb.box_type == incognidium_layout::BoxType::Image {
            continue;
        }
        let onscreen = !is_box_offscreen(fb, viewport_width);
        let id = fb.node_id;
        node_has_text_box.insert(id);
        if onscreen {
            node_offscreen_all.remove(&id);
        } else if !node_text_seen.contains(&id) {
            // First time we see this node: mark off-screen unless already on-screen.
            node_offscreen_all.insert(id);
        }
        node_text_seen.insert(id);
    }

    fn is_hidden(styles: &incognidium_style::StyleMap, node_id: incognidium_dom::NodeId) -> bool {
        styles.get(&node_id).map_or(false, |s| {
            s.display == Display::None || s.visibility == Visibility::Hidden
        })
    }

    fn should_skip_tag(tag: &str) -> bool {
        matches!(
            tag,
            "script"
                | "style"
                | "noscript"
                | "template"
                | "head"
                | "meta"
                | "link"
                | "iframe"
                | "object"
                | "embed"
                | "audio"
                | "video"
                | "track"
                | "source"
                | "canvas"
                | "svg"
        )
    }

    fn is_block(tag: &str) -> bool {
        matches!(
            tag,
            "address"
                | "article"
                | "aside"
                | "blockquote"
                | "body"
                | "dd"
                | "div"
                | "dl"
                | "dt"
                | "fieldset"
                | "figcaption"
                | "figure"
                | "footer"
                | "form"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "header"
                | "hr"
                | "li"
                | "main"
                | "nav"
                | "ol"
                | "p"
                | "pre"
                | "section"
                | "table"
                | "tbody"
                | "td"
                | "tfoot"
                | "th"
                | "thead"
                | "tr"
                | "ul"
        )
    }

    fn attr_text(el: &incognidium_dom::ElementData, names: &[&str]) -> Option<String> {
        for name in names {
            if let Some(v) = el.attributes.get(*name) {
                let t = v.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        None
    }

    fn attribute_label(el: &incognidium_dom::ElementData) -> Option<String> {
        let tag = el.tag_name.as_str();
        match tag {
            "img" => attr_text(el, &["alt", "aria-label", "title"]),
            "area" => attr_text(el, &["alt", "aria-label", "title"]),
            "input" | "textarea" | "button" | "select" => {
                attr_text(el, &["placeholder", "aria-label", "title", "value"])
            }
            _ => attr_text(el, &["aria-label", "title"]),
        }
    }

    fn collect_node(
        doc: &incognidium_dom::Document,
        styles: &incognidium_style::StyleMap,
        node_id: incognidium_dom::NodeId,
        in_hidden: bool,
        offscreen_all: &std::collections::HashSet<incognidium_dom::NodeId>,
    ) -> Vec<String> {
        if in_hidden {
            return Vec::new();
        }
        // If this node produced only off-screen text boxes, treat it as hidden.
        if offscreen_all.contains(&node_id) {
            return Vec::new();
        }
        let node = &doc.nodes[node_id];
        match &node.data {
            NodeData::Element(el) => {
                let tag = el.tag_name.as_str();
                if should_skip_tag(tag) {
                    return Vec::new();
                }
                let hidden = in_hidden || is_hidden(styles, node_id);
                let mut parts: Vec<String> = Vec::new();
                for &child in &node.children {
                    parts.extend(collect_node(doc, styles, child, hidden, offscreen_all));
                }
                if parts.is_empty() {
                    if let Some(t) = attribute_label(el) {
                        parts.push(t);
                    }
                }
                if is_block(tag) && !parts.is_empty() {
                    // Collapse inline descendants into one paragraph for this block.
                    let joined = parts.join(" ");
                    if !joined.is_empty() {
                        return vec![joined];
                    }
                }
                parts
            }
            NodeData::Text(t) => {
                let s = t.content.trim();
                if !s.is_empty() {
                    vec![s.to_string()]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    let mut parts = Vec::new();
    if let Some(html_id) = doc.document_element() {
        parts = collect_node(doc, styles, html_id, false, &node_offscreen_all);
    }
    parts.join("\n")
}

/// Walk a layout tree and record the content-box size of every node that
/// establishes a container context.  The sizes are used by a second style-
/// resolution pass to evaluate real `@container` queries instead of guessing.
fn collect_container_sizes(
    layout_box: &incognidium_layout::LayoutBox,
    styles: &incognidium_style::StyleMap,
    map: &mut HashMap<incognidium_dom::NodeId, (f32, f32)>,
) {
    if let Some(style) = styles.get(&layout_box.node_id) {
        if matches!(
            style.container_type,
            ContainerType::Size | ContainerType::InlineSize
        ) {
            map.insert(
                layout_box.node_id,
                (
                    layout_box.content_width.max(0.0),
                    layout_box.content_height.max(0.0),
                ),
            );
        }
    }
    for child in &layout_box.children {
        collect_container_sizes(child, styles, map);
    }
}

/// Recursively print the layout tree for debugging layout collapse.
fn dump_layout_tree(
    layout_box: &incognidium_layout::LayoutBox,
    doc: &incognidium_dom::Document,
    styles: &incognidium_style::StyleMap,
    depth: usize,
) {
    let indent = "  ".repeat(depth);
    let (tag, _cls) = if layout_box.node_id >= doc.nodes.len() {
        (String::from("::pseudo"), String::new())
    } else {
        match &doc.nodes[layout_box.node_id].data {
            incognidium_dom::NodeData::Element(ref e) => {
                let mut tag = e.tag_name.clone();
                if let Some(id) = e.get_attr("id") {
                    tag.push('#');
                    tag.push_str(id);
                }
                (tag, e.get_attr("class").unwrap_or("").to_string())
            }
            _ => (String::from("#text"), String::new()),
        }
    };
    let text_preview = layout_box
        .text
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(40)
        .collect::<String>();
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    let pos = format!("{:?}", style.position).to_lowercase();
    let transform_info = if style.transform.is_empty() {
        String::new()
    } else {
        format!(" transform={:?}", style.transform)
    };
    let bg_info = if style.background_color.a == 0 {
        String::new()
    } else {
        format!(
            " bg=rgba({},{},{},{})",
            style.background_color.r,
            style.background_color.g,
            style.background_color.b,
            style.background_color.a
        )
    };
    eprintln!(
        "{}{} node={} [{:.0},{:.0} {}x{}] {:?} pos={} top={:?} bottom={:?} margin=({:.0},{:.0},{:.0},{:.0}){}{} text=\"{}\"",
        indent,
        tag,
        layout_box.node_id,
        layout_box.x,
        layout_box.y,
        layout_box.width,
        layout_box.height,
        layout_box.box_type,
        pos,
        style.top,
        style.bottom,
        style.margin_top,
        style.margin_right,
        style.margin_bottom,
        style.margin_left,
        transform_info,
        bg_info,
        text_preview.replace('\n', " ")
    );
    for child in &layout_box.children {
        dump_layout_tree(child, doc, styles, depth + 1);
    }
}

/// Extract page metadata that should be included when visible text is sparse.
fn extract_page_metadata(doc: &incognidium_dom::Document) -> Vec<String> {
    let mut out = Vec::new();
    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            match el.tag_name.as_str() {
                "title" => {
                    for &child in &node.children {
                        if let incognidium_dom::NodeData::Text(ref t) = doc.nodes[child].data {
                            let s = t.content.trim();
                            if !s.is_empty() {
                                out.push(s.to_string());
                            }
                        }
                    }
                }
                "meta" => {
                    let name = el.get_attr("name").unwrap_or_default().to_lowercase();
                    if name == "description" || name == "og:description" {
                        if let Some(v) = el.get_attr("content") {
                            let s = v.trim();
                            if !s.is_empty() {
                                out.push(s.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "https://en.wikipedia.org/wiki/Main_Page".into());
    let output = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "/tmp/incognidium_render.png".into());
    // Optional: --text <path> to dump extracted text
    let text_output = args
        .iter()
        .position(|a| a == "--text")
        .and_then(|i| args.get(i + 1).cloned());
    // Optional: --dump-html <path> to dump post-JS DOM as HTML
    let html_output = args
        .iter()
        .position(|a| a == "--dump-html")
        .and_then(|i| args.get(i + 1).cloned());
    // Optional: --dump-css <path> to dump combined CSS used for styling
    let css_output = args
        .iter()
        .position(|a| a == "--dump-css")
        .and_then(|i| args.get(i + 1).cloned());
    // Optional: --dump-styles <path> to dump resolved computed styles per element
    let styles_output = args
        .iter()
        .position(|a| a == "--dump-styles")
        .and_then(|i| args.get(i + 1).cloned());
    // Optional: --dump-boxes <path> to dump all flat boxes with coordinates/text
    let boxes_output = args
        .iter()
        .position(|a| a == "--dump-boxes")
        .and_then(|i| args.get(i + 1).cloned());
    // Optional: --wait <ms> to wait for JS rendering
    let wait_ms: u64 = args
        .iter()
        .position(|a| a == "--wait")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Optional: --no-js to skip JavaScript execution. Useful when JS engines
    // crash on a site and the server-rendered HTML is sufficient.
    let no_js =
        args.iter().any(|a| a == "--no-js") || std::env::var("INCOGNIDIUM_DISABLE_JS").is_ok();

    // Check if input is a file path (starts with / or . or ends with .html but is not a URL)
    let is_file = (input.starts_with('/') || input.starts_with('.')) && !input.starts_with("http")
        || (input.ends_with(".html") && !input.starts_with("http"));

    let (mut body, mut base_url) = if is_file {
        eprintln!("Reading file {input}...");
        let path = std::path::Path::new(&input);
        let body = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Failed to read file: {e}");
            std::process::exit(2);
        });
        // Use file:// URL as base for resolving relative URLs
        let base = path
            .canonicalize()
            .ok()
            .map(|p| format!("file://{}", p.to_string_lossy()))
            .unwrap_or_else(|| "file:///".into());
        (body, base)
    } else {
        eprintln!("Fetching {input}...");
        let resp = match fetch_url(&input) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("fetch failed: {e}");
                std::process::exit(2);
            }
        };
        (resp.body, input.clone())
    };
    eprintln!("Got {} bytes of HTML", body.len());

    let mut doc = parse_html(&body);
    // Pass the URL fragment to the CSS :target matcher so rules like
    // `.modal-overlay:target~*` do not match every element when no fragment is
    // present.
    if let Some((_, fragment)) = input.rsplit_once('#') {
        if !fragment.is_empty() {
            doc.target_id = Some(fragment.to_string());
        }
    }

    // Follow a single noscript <meta http-equiv="refresh"> redirect before
    // executing scripts. This lets language-redirector homepages such as
    // ruby-lang.org render their real content instead of the redirect page.
    if !is_file {
        if let Some(target) = incognidium_shell::meta_refresh_target(&body, &base_url) {
            eprintln!("Following meta refresh to {target}...");
            match fetch_url(&target) {
                Ok(resp) => {
                    base_url = resp.url.clone();
                    body = resp.body;
                    doc = parse_html(&body);
                    if let Some((_, fragment)) = input.rsplit_once('#') {
                        if !fragment.is_empty() {
                            doc.target_id = Some(fragment.to_string());
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to follow meta refresh {target}: {e}");
                }
            }
        }
    }

    eprintln!("DOM: {} nodes", doc.nodes.len());

    // Collect scripts (inline + external)
    let scripts = collect_scripts(&doc, &base_url);
    eprintln!("Scripts: {} found", scripts.len());
    if no_js {
        eprintln!("JS execution disabled by --no-js / INCOGNIDIUM_DISABLE_JS");
    }

    // Helper: count element children of <body> (or document root) to detect when
    // JS execution stripped the server-rendered content and we should fall back.
    fn count_body_element_children(doc: &incognidium_dom::Document) -> usize {
        doc.body()
            .map(|body_id| {
                doc.node(body_id)
                    .children
                    .iter()
                    .filter(|&&cid| {
                        matches!(&doc.node(cid).data, incognidium_dom::NodeData::Element(_))
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    // Execute scripts with a hard 15-second timeout
    let mut image_cache: HashMap<String, ImageData> = HashMap::new();
    let doc = if !scripts.is_empty() && !no_js {
        // Clone doc before moving into thread for fallback
        let doc_for_thread = doc.clone();
        let scripts_clone: Vec<_> = scripts
            .iter()
            .map(|s| incognidium_shell::ScriptEntry {
                source: s.source.clone(),
                origin: s.origin.clone(),
            })
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        // Give the JS thread a very generous stack; modern bundles and V8 can
        // recurse deeply and overflow the default 2 MB Rust thread stack.
        std::thread::Builder::new()
            .stack_size(128 * 1024 * 1024)
            .spawn(move || {
                let mut ic = HashMap::new();
                let modified = execute_scripts_on_doc(doc_for_thread, &scripts_clone, &mut ic);
                let _ = tx.send((modified, ic));
            })
            .expect("spawn js thread");
        match rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok((modified_doc, js_images)) => {
                for (k, v) in js_images {
                    image_cache.insert(k, v);
                }
                eprintln!(
                    "JS executed, modified DOM: {} nodes",
                    modified_doc.nodes.len()
                );
                if let Some(ref html_path) = html_output {
                    let html = serialize_document_to_html(&modified_doc);
                    std::fs::write(html_path, html).expect("write html dump");
                    eprintln!("DOM HTML dumped to {html_path}");
                }
                // Some scripts (e.g. CSS-Tricks' Jetpack search bundle) clear the
                // server-rendered body when they fail to hydrate. Fall back to the
                // original pre-JS DOM if JS left the body empty but the original page
                // had real content.
                let modified_body_children = count_body_element_children(&modified_doc);
                let original_body_children = count_body_element_children(&doc);
                if modified_body_children == 0 && original_body_children > 0 {
                    eprintln!(
                        "JS stripped body content ({} -> {} element children); using original DOM",
                        original_body_children, modified_body_children
                    );
                    doc
                } else {
                    modified_doc
                }
            }
            Err(_) => {
                eprintln!("JS timed out after 15s, using original DOM");
                if let Some(ref html_path) = html_output {
                    let html = serialize_document_to_html(&doc);
                    std::fs::write(html_path, html).expect("write html dump");
                    eprintln!("DOM HTML dumped to {html_path}");
                }
                // Use original parsed DOM instead of re-parsing
                doc
            }
        }
    } else {
        doc
    };

    // Repair any cycles / broken parent pointers introduced by JS DOM manipulation
    // so that downstream layout can safely recurse.
    let mut doc = doc;
    doc.sanitize_tree();

    // DEBUG: dump DOM children of the NBA hero layer and shadow node.
    if std::env::var("DUMP_NBA_HERO_DOM").is_ok() {
        for (id, node) in doc.nodes.iter().enumerate() {
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                let cls = el.get_attr("class").unwrap_or("");
                if cls.contains("HomeHero_heroLayer__") || cls.contains("HomeHero_heroShadow__") {
                    let tag = el.tag_name.as_str();
                    let parent_tag = node.parent.and_then(|pid| {
                        doc.nodes.get(pid).and_then(|p| match &p.data {
                            incognidium_dom::NodeData::Element(ref pe) => {
                                Some(pe.tag_name.as_str())
                            }
                            _ => None,
                        })
                    });
                    let child_tags: Vec<String> = node
                        .children
                        .iter()
                        .map(|cid| {
                            doc.nodes
                                .get(*cid)
                                .map(|c| match &c.data {
                                    incognidium_dom::NodeData::Element(ref ce) => format!(
                                        "{}#{} class={}",
                                        ce.tag_name,
                                        cid,
                                        ce.get_attr("class").unwrap_or("")
                                    ),
                                    incognidium_dom::NodeData::Text(_) => format!("#text#{}", cid),
                                    _ => format!("?#{}", cid),
                                })
                                .unwrap_or_else(|| format!("missing#{}", cid))
                        })
                        .collect();
                    eprintln!(
                        "DOM node={} tag={} class={} parent={:?} children=[{}]",
                        id,
                        tag,
                        cls,
                        parent_tag,
                        child_tags.join(", ")
                    );
                }
            }
        }
    }

    // Trim Brightspot load-more lists to their declared visible item count. Without
    // this the server-rendered HTML includes every item and the page renders far
    // taller than the JS-enhanced browser view.
    incognidium_shell::trim_bsp_list_loadmore(&mut doc);

    // AOL/Yahoo-specific fixes: subgrid fallback, ad slot removal, lazy-image
    // skeleton stripping, and stream-card skeleton removal. These must run before
    // the generic placeholder trimmer.
    incognidium_shell::fix_aol_yahoo_subgrid(&mut doc, &base_url);
    incognidium_shell::remove_aol_yahoo_ad_slots(&mut doc);
    incognidium_shell::trim_yahoo_stream_skeletons(&mut doc, &base_url);
    incognidium_shell::fix_wikipedia_client_nojs(&mut doc, &base_url);
    incognidium_shell::strip_lazy_image_skeletons(&mut doc);
    incognidium_shell::promote_lazy_image_sources(&mut doc);
    incognidium_shell::remove_hidden_login_dropdowns(&mut doc, &base_url);
    incognidium_shell::remove_adchoices_overlays(&mut doc);

    // mdBook populates its sidebar through a custom element that Incognidium's
    // JS engine cannot upgrade. Restore the server-generated TOC from toc.html
    // *before* the generic placeholder trimmer runs, because the empty sidebar is
    // initially marked `aria-hidden="true"` and would otherwise be pruned as a
    // placeholder before we can inject the chapter list.
    incognidium_shell::trim_mdbook_sidebar(&mut doc, &base_url);

    // Drop empty placeholder/ad containers that the real browser hides/fills via JS.
    // These still consume CSS height in the headless renderer even though they have
    // no visible content.
    incognidium_shell::remove_empty_placeholders(&mut doc);

    // NBC News multi-storyline packages leave empty article-card wrappers behind
    // when their JS hides a card or its lazy media is absent. They bloat the rail
    // by thousands of pixels; drop the empty ones.
    incognidium_shell::trim_nbc_empty_headline_placeholders(&mut doc, &base_url);

    // Remove visible cookie / GDPR / consent banners that server-render before the
    // site's consent JS can dismiss them. They otherwise dominate the viewport.
    incognidium_shell::remove_consent_banners(&mut doc);

    // Remove "unsupported browser" / "upgrade your browser" banners that some
    // sites (NBC News, nature.com) inject when they do not recognize the UA.
    incognidium_shell::remove_unsupported_browser_banners(&mut doc);

    // Remove US government Touchpoints customer-feedback forms and their
    // triggers. The modal is hidden in real browsers until the user clicks the
    // feedback button; without the Touchpoints script it renders inline and
    // dominates the page height on .gov sites such as FDA.gov.
    incognidium_shell::remove_touchpoints_forms(&mut doc);

    // Collapse USWDS government site banners to their header bar. The
    // explanatory content block is hidden in real browsers until toggled, but
    // our renderer shows it because the toggle JS does not run.
    incognidium_shell::collapse_usa_banner(&mut doc);

    // Trim horizontally-snapping carousels to their declared visible item count.
    // Our layout engine does not implement overflow scroll / snap, so these
    // containers otherwise render every item vertically.
    incognidium_shell::trim_scroll_snap_carousels(&mut doc);

    // Stratechery's homepage server-renders full paywalled articles inside
    // `.entry-content.is-style-continue-reading` blocks. The visible state keeps
    // only the first few children; trim the rest to avoid a ~75 kpx homepage.
    incognidium_shell::trim_stratechery_continue_reading(&mut doc, &base_url);

    // AP News, Metacritic, Kottke, and The Intercept homepage lists render far
    // more items than the visible browser surface. Trim them to a representative
    // subset.
    incognidium_shell::trim_apnews_pagelist_items(&mut doc, &base_url);
    incognidium_shell::trim_apnews_hamburger(&mut doc, &base_url);
    incognidium_shell::trim_foxnews_collections(&mut doc, &base_url);
    incognidium_shell::trim_metacritic_carousel_items(&mut doc, &base_url);
    incognidium_shell::trim_kottke_posts(&mut doc, &base_url);
    incognidium_shell::trim_theintercept_cards(&mut doc, &base_url);

    // Responsive images: the fallback `src` attribute is sometimes invalid
    // (e.g. PBS's hero uses a non-integer resize height that the CDN rejects),
    // while `srcset` contains valid integer-sized alternatives. Pick the best
    // srcset candidate for our 1024px viewport and use it as the effective src
    // for both fetching and layout.
    select_srcset_images(&mut doc, &base_url, 1024.0);

    // Fetch images from the page
    let fetched_images = fetch_page_images(&doc, &base_url);
    eprintln!("Images: {} fetched", fetched_images.len());
    for (src, data) in &fetched_images {
        image_cache.insert(src.clone(), data.clone());
    }

    // Fetch external CSS from <link rel="stylesheet"> tags
    let mut css_text = fetch_external_css(&doc, &base_url);

    // Add <style> block CSS from the (possibly modified) DOM
    let style_css = doc.collect_style_text();
    css_text.push_str(&style_css);

    // Force light mode: sites like Wikipedia hide dark variable sets inside
    // `prefers-color-scheme: dark` media queries. Our renderer doesn't report a
    // real preference, so those blocks can match and render a black page.
    css_text = incognidium_shell::strip_dark_mode_media_queries(&css_text);

    // Extract data URI images from CSS background-image properties
    // This needs to happen before parsing CSS so they're in the image cache
    eprintln!(
        "About to extract CSS data URI images from {} bytes",
        css_text.len()
    );
    let css_data_uri_images = extract_css_data_uri_images(&css_text);
    eprintln!(
        "CSS Images: {} data URIs extracted",
        css_data_uri_images.len()
    );
    for (src, data) in css_data_uri_images {
        image_cache.insert(src, data);
    }

    // Legacy <center> elements (e.g. Hacker News) center their block-level
    // children in quirks mode. Approximate that with auto side margins so
    // width-constrained tables sit in the middle of the page instead of
    // hugging the left edge.
    css_text.push_str("center { text-align: center; }\n");
    css_text.push_str("center > table { margin-left: auto; margin-right: auto; }\n");
    // With our JS engine enabled, <noscript> fallback content (often an iframe)
    // should not be rendered. Hide it to prevent full-viewport white overlays.
    if !no_js {
        css_text.push_str("noscript { display: none !important; }\n");
    }
    // ABC's light header depends on JS adding .navigation--dark or
    // .navigation--has-takeOver, which neutralizes a default brightness(.1) filter
    // on the logo and swaps in a light logo SVG. Without JS the logo and icons render
    // almost black. Reset the filter and force the light logo so the header looks
    // like the server-rendered light theme.
    if base_url.as_str().contains("abcnews.go.com") {
        css_text.push_str(".navLogo__icon { filter: none !important; background-image: url(https://s.abcnews.com/assets/dtci/icomoon/svg/logo.svg) !important; }\n");
        // The dark header theme is added by JS; without it the nav renders as a
        // white bar with black text that blends into/overlaps the light page.
        // Force the dark theme colors so the header is readable.
        css_text.push_str(".navigation { background-color: #00081a !important; }\n");
        css_text.push_str(
            ".navigation .navMenu__text, .navigation .navMenu__link { color: #fff !important; }\n",
        );
    }
    // AP News keeps its desktop category nav in a flex row where the <ul> has
    // `max-width: fit-content` and a "More" button wrapper contains a span with
    // `width: 100%`. Without JS interaction the <ul> stays at its fit-content
    // width, the 100%-wide span blows the More button's base size up to the
    // container width, and the category links crowd together while the More
    // button claims the leftover space. Let the <ul> grow and keep the More
    // button at its text width so the nav resembles the Firefox layout.
    if base_url.as_str().contains("apnews.com") {
        css_text.push_str(".MainNavigation-items { max-width: none !important; }\n");
        css_text.push_str(".MainNavigationItem-more { width: auto !important; }\n");
    }
    // Smashing Magazine's header is a CSS Grid where the search column is sized
    // `minmax(100px, 350px)` at our 1024px viewport. Incognidium's grid track
    // sizing currently lets the search input's placeholder/width blow the column
    // past that 350px maximum, so the search box crowds the navigation items.
    // Clamp the search container and input so the header stays balanced.
    if base_url.as_str().contains("smashingmagazine.com") {
        css_text.push_str(
            ".header .search { max-width: 350px !important; }
",
        );
        css_text.push_str(
            ".search-form { max-width: 350px !important; }
",
        );
        css_text.push_str(
            ".search-input { max-width: 350px !important; }
.search-button { display: none !important; }
",
        );
    }
    // Slate keeps its theme-picker dropdown (`Light / Dark / Auto`) and the strapline
    // search form in the DOM without an open-state class. The dropdown renders inline
    // below the search box and overlaps the masthead/article grid. Hide the dropdown
    // content; the search input itself remains usable.
    if base_url.as_str().contains("slate.com") {
        css_text.push_str(".theme-picker .dropdown__content { display: none !important; }\n");
        css_text.push_str(".strapline__search .theme-picker { display: none !important; }\n");
    }
    // Al Jazeera homepage: the liveblog hero image wrapper uses `width: 100vw`
    // and the inner `.responsive-image` has `height: 100%`, but without a
    // definite containing height the photo collapses and the headline sits on a
    // blank red background. Force a normal width and a real aspect ratio so the
    // hero photo renders behind the title overlay. Also hide the empty top
    // leaderboard ad slot.
    if base_url.as_str().contains("aljazeera.com") {
        // The liveblog hero card's image wrapper is given `width: 100vw` and the
        // inner `.responsive-image` is set to `height: 100%`, but the wrapper has
        // no definite height, so the image collapses to zero and the headline sits
        // on a blank red background. Give the image container a normal width and
        // restore the responsive-image aspect ratio so the hero photo is visible
        // behind the title overlay.
        css_text.push_str(".container--three-col-layout-wrapper #featured-news-container article.article-card.article-card__liveblog.top-story-article .article-card__liveblog-img-container { width: 100% !important; }\n");
        css_text.push_str(".container--three-col-layout-wrapper #featured-news-container article.article-card.article-card__liveblog.top-story-article .article-card__liveblog-img-container .article-card__image-wrap { width: 100% !important; height: auto !important; }\n");
        css_text.push_str(".container--three-col-layout-wrapper #featured-news-container article.article-card.article-card__liveblog.top-story-article .article-card__liveblog-img-container .article-card__image-wrap .responsive-image { height: auto !important; aspect-ratio: 16/9 !important; }\n");
        // Hide the empty leaderboard ad slot at the top of the page so the real
        // header starts at the top like it does in Firefox.
        css_text.push_str(".container--ads-leaderboard-atf { display: none !important; }\n");
    }
    // ProPublica's mobile navigation overlay and sticky header clone are kept in the
    // DOM for JS interactivity but render as an open search bar and duplicated header
    // when no interaction happens. Hide them so only the primary full header renders.
    if base_url.as_str().contains("propublica.org") {
        css_text
            .push_str(".site-header-overlay, .site-header-fixed { display: none !important; }\n");
        // Without JS-driven CSS, ProPublica renders both its mobile and desktop
        // header layouts simultaneously. Hide the mobile-specific rows at 1024px
        // so only the desktop header remains.
        css_text.push_str(".site-header--full__mobile-top, .site-header--full__mobile-wordmark, .site-header--full__icon-btns { display: none !important; }\n");
        // The logo SVG uses currentColor and falls back to the default link blue.
        // Force it to black like the rendered desktop theme.
        css_text.push_str(".site-header--full__wordmark { color: #000 !important; }\n");
        // ProPublica's responsive design debug overlay (`.grid-overlay`) is a fixed,
        // full-viewport grid of tinted columns. It is not content and its container
        // opacity is not always honored by the renderer, so it can paint a red
        // tint over the page. Remove it entirely.
        css_text.push_str(".grid-overlay, .grid-overlay--hide { display: none !important; }\n");
    }
    // CNET's "curated content block" sidebars (Best Products, Today's Deals, etc.)
    // render as bright yellow/red tinted columns. They are non-article modules and
    // visually dominate the screenshot, so hide the list-style curated blocks.
    if base_url.as_str().contains("cnet.com") {
        // NOTE: previously hid `.c-ccb` curated-content sidebars because their
        // tinted backgrounds dominated the page, but that removed the left "BEST"
        // rail on the homepage and broke the top hero layout. Keep the rail
        // visible so the render matches Firefox.
        // css_text.push_str(".c-ccb, .wp-block-column.has-background:has(.c-ccb), .wp-block-column.has-background:has(.ccb-header) { display: none !important; }\n");
        // CNET appends an AdChoices SVG (`#adchoicesBtn`) at the end of `<body>`.
        // Without the ad script to size and position it, our SVG rasterization uses
        // the huge viewBox dimensions and the icon covers the entire top-left of
        // the page. Hide it so the real content starts at the top.
        css_text.push_str("svg#adchoicesBtn { display: none !important; }\n");
        // CNET uses CSS container queries to switch its category card lists from a
        // vertical stack to a horizontal row at large container widths. Incognidium
        // does not implement container queries, so the `.ccb-list__layout` flex
        // container stays `flex-direction: column` and every category section (Mobile,
        // Hardware, Tech Tips, etc.) stacks its header and article list vertically,
        // producing a page ~4-5x taller than a real browser. Force the desktop row
        // layout for CNET's curated content blocks.
        // NOTE: the `.ccb-list__layout` in the left sidebar (x=32, width=136) should
        // remain vertical. The main content area `.entry-list` uses Grid layout which
        // works correctly. The flex-direction override was incorrectly affecting the
        // sidebar, so it has been removed.
        // css_text.push_str(".ccb-list__layout { flex-direction: row !important; flex-wrap: wrap !important; }\n");
        // css_text.push_str(".ccb-list__layout > * { flex: 0 0 auto !important; width: auto !important; }\n");
    }
    // NPR's global navigation keeps every submenu in the DOM and hides them with
    // `visibility:hidden`/`opacity:0`. Incognidium does not suppress those
    // properties, so collapsed `.submenu` panels render as tall vertical grids
    // that push the real homepage content down. Hide them unless they are
    // explicitly expanded via the `.is-expanded` class.
    if base_url.as_str().contains("npr.org") {
        css_text.push_str(".submenu:not(.is-expanded) { display: none !important; }\n");
    }
    // The Intercept renders its mobile/off-canvas navigation (`offcanvas`) as a
    // tall sibling at 1024px because the JS that collapses it never runs.
    // `.offcanvas-wrapper` also wraps the main `#page` element, so hide only the
    // off-canvas menu panel itself, not the wrapper.
    if base_url.as_str().contains("theintercept.com") {
        css_text.push_str(".offcanvas { display: none !important; }\n");
    }
    // MDN keeps its theme-picker and language-switcher dropdown content in the DOM
    // without an open-state class. The dropdown panels render inline below the
    // header controls and overlap the article content. Hide the dropdown panels;
    // the toggle buttons themselves remain visible.
    if base_url.as_str().contains("developer.mozilla.org") {
        css_text.push_str(
            ".color-theme__dropdown, .language-switcher__dropdown { display: none !important; }\n",
        );
    }
    // Python docs' pydoctheme uses display:flex on div.document, but the DOM
    // order is body then sidebar and the stylesheet has no order property, so
    // Incognidium places the sticky sidebar on the right. The reference Firefox
    // render uses the classic float-based two-column layout with the sidebar on
    // the left. Revert the document and sidebar wrappers to block flow so the
    // classic float rules (documentwrapper width:100%, sphinxsidebar float:left
    // with margin-left:-100%) place the sidebar beside the body content.
    if base_url.as_str().contains("docs.python.org") {
        // Python docs' pydoctheme uses display:flex on div.document with the sidebar
        // after the body in the DOM and no order property, so Incognidium places the
        // sidebar on the right while Firefox places it on the left. Move the sidebar
        // first in the flex line and drop the bodywrapper offset so the body fills the
        // remaining space rather than leaving a double gutter.
        css_text.push_str("div.sphinxsidebar { order: -1 !important; width: 256px !important; flex-shrink: 0 !important; }\n");
        css_text.push_str("div.bodywrapper { margin-left: 0 !important; }\n");
    }
    // Salon keeps its mobile hamburger menu in the DOM as a tall white
    // off-canvas panel. At our 1024px desktop viewport it is hidden by
    // interactive JS, but without that interaction it covers the whole page.
    // The body also uses `display: flex`, and stray whitespace text nodes in the
    // body are treated as flex items that push the real content far down the page.
    if base_url.as_str().contains("salon.com") {
        css_text.push_str(".button__burger, .navigation__burger, .navigation__mobile, .menu-burger-menu-container { display: none !important; }\n");
        // Salon uses `body { display: flex; height: 100vh; }`. At our viewport the
        // whitespace-only anonymous flex items each claim the full viewport height,
        // pushing the real content two screens down. Force normal block flow with
        // auto height so the header and main content start at the top.
        css_text.push_str("body { display: block !important; height: auto !important; min-height: 0 !important; }\n");
    }
    // Mother Jones renders its full hamburger dropdown menu in the right rail
    // because the JS that collapses it never runs. Hide the dropdown containers
    // so the right rail article list is visible.
    if base_url.as_str().contains("motherjones.com") {
        css_text.push_str(".menu-top-nav-container, .menu-floating-navbar-container { display: none !important; }\n");
    }
    // Condé Nast sites (Wired, Vanity Fair, The New Yorker, GQ, Vogue) keep the
    // OneNav hamburger/search menus as tall inert siblings in the DOM. Without the
    // JS interaction that hides them, their 100vh menu overlay covers or pushes
    // the real page content down. Hide the focus-trap wrappers so the homepage
    // renders at the top of the viewport.
    let is_conde_onenav = [
        "wired.com",
        "vanityfair.com",
        "newyorker.com",
        "gq.com",
        "vogue.com",
    ]
    .iter()
    .any(|d| base_url.as_str().contains(d));
    if is_conde_onenav {
        // The hashed class is a single token like `FocusTrapContainer-bGnOHb`, so
        // a plain class selector does not match. Use a substring attribute selector
        // to hide every focus-trap wrapper regardless of the styled-components hash.
        css_text.push_str("[class*=\"FocusTrapContainer\"] { display: none !important; }\n");
    }
    // Bootstrap dropdown menus are hidden by real browsers via opacity and
    // pointer-events until the toggle is clicked. Incognidium does not model
    // pointer-events and only applies opacity to backgrounds/borders, so closed
    // `.dropdown-menu` panels (login panels, account settings, navigation mega
    // menus) render inline and dominate the page. Hide them unless the dropdown
    // is explicitly open (`.show` on the dropdown or on the menu itself). The
    // account/hamburger icon toggle is preserved because it is not a
    // `.dropdown-menu`.
    css_text
        .push_str(".dropdown:not(.show) .dropdown-menu:not(.show) { display: none !important; }\n");
    // Bootstrap modals are also hidden by default and only shown when JS adds
    // `.show`. Without that interaction the modal shell renders as an empty or
    // semi-transparent overlay that can cover content.
    css_text.push_str(".modal:not(.show) { display: none !important; }\n");
    // Many news/mega-menu navs keep submenu panels in the DOM with
    // `visibility:hidden` / `opacity:0` / `height:0` and reveal them only when a
    // parent item has an open-state class such as `.menu__item--submenu-open`.
    // Incognidium does not fully honor those properties, so the hidden submenu
    // items render inline and can overflow or overlap the header. Hide them
    // unless the open-state class is present.
    css_text.push_str(
        ".menu__item:not(.menu__item--submenu-open) .submenu_wrapper, \
         .menu__item:not(.menu__item--submenu-open) > .menu__submenu { display: none !important; }\n",
    );
    // Al Jazeera (and several other news sites) rely on a `::before` pseudo-element
    // with `padding-bottom: 56.25%` to give `.responsive-image` wrappers their
    // aspect ratio. Incognidium does not render pseudo-elements as real boxes, so
    // the wrapper collapses to zero height and the absolutely-positioned cover
    // image is stretched across the entire viewport. Force the same 16:9 ratio
    // directly on the wrapper so the image has a real containing block to fill.
    css_text.push_str(
        ".responsive-image:not(.responsive-image--disableIntrinsicHeight) { aspect-ratio: 16/9 !important; }\n",
    );
    // Al Jazeera's cover images use `position:absolute; inset:-9999px; margin:auto;
    // min-width:100%; min-height:100%; max-width:100%` without a max-height. With
    // our simplified absolute sizing that stretches an auto-height box to the
    // inset distance, the image becomes tens of thousands of pixels tall and covers
    // the page. Cap the stretched height so it stays inside the responsive-image
    // wrapper, letting `object-fit:cover` draw the visible portion correctly.
    css_text.push_str(
        ".article-card__image, .gc__image, .responsive-image > img { max-height: 100% !important; }\n",
    );
    // Google homepage renders its logo SVG with `width:auto`, `max-height:100%`,
    // and `height:auto`. The surrounding grid row has no definite height at the
    // point our layout engine resolves the percentage max-height, so the logo
    // collapses to 0 px tall and the whole search bar is pushed to the top of
    // the page. Force the logo to its intrinsic aspect-ratio height and disable
    // the percentage max-height constraint for the main logo.
    if base_url.as_str().contains("google.com") && !base_url.as_str().contains("scholar.google.com")
    {
        css_text.push_str(".lnXdpd { max-height: none !important; height: auto !important; }\n");
    }
    // CNET uses CSS container queries to switch its category card lists from a
    // vertical stack to a horizontal row at large container widths. Incognidium
    // does not implement container queries, so the `.ccb-list__layout` flex
    // container stays `flex-direction: column` and every category section (Mobile,
    // Hardware, Tech Tips, etc.) stacks its header and article list vertically,
    // producing a page ~4-5x taller than a real browser. Force the desktop row
    // layout for CNET's curated content blocks.
    // Google Scholar keeps its advanced-search modal and dropdown menus in the
    // DOM with `visibility:hidden` and `transform:scale(0,0)`. Our layout engine
    // does not honor those properties, so the modal shell covers the homepage.
    // Hide modal wrappers and dropdowns unless JS has opened them with `.gs_vis`.
    if base_url.as_str().contains("scholar.google.com") {
        // Scholar keeps its advanced-search modal and dropdown menus in the DOM
        // with `visibility:hidden`/`transform:scale(0,0)`. Our engine does not
        // honor those properties, so hide modal wrappers unless JS opened them.
        css_text.push_str(".gs_md_wnw:not(.gs_vis), .gs_md_ulr:not(.gs_vis), .gs_md_d:not(.gs_vis) { display: none !important; }\n");

        // The Scholar homepage lays the search bar/logo out as an absolute layer
        // inside a fixed-height header, with a large bottom margin to reserve
        // space. Our block layout ignores that absolute layer, so the logo and
        // form overflow the header and overlap the Articles/Case law tabs below.
        // Only rewrite the header on the homepage so search-result pages keep
        // their compact header layout.
        let scholar_homepage =
            body.contains("id=\"gs_hp_main\"") || body.contains("id='gs_hp_main'");
        if scholar_homepage {
            // Let the flex header grow to contain the now-static search block.
            css_text.push_str("#gs_hdr { height: auto !important; min-height: auto !important; margin-bottom: 0 !important; }\n");
            // The header middle column also has a percentage/fixed height tied to
            // the original 63px header; release it so the search block can expand.
            // Keep it as the flexible main-axis item so the Sign-in link on the
            // right stays inside the viewport instead of being pushed off-canvas.
            css_text
                .push_str("#gs_hdr_md { height: auto !important; flex: 1 1 auto !important; }\n");
            // Remove the absolute offsets and let the search layer sit in flow.
            css_text.push_str("#gs_hdr_srch { position: static !important; top: auto !important; left: auto !important; right: auto !important; width: 100% !important; max-width: none !important; }\n");
            // Stack the form inputs as a visible flex row and give the logo
            // wrapper a sane block height so it no longer overlaps the form.
            css_text.push_str("#gs_hdr_frm { display: flex !important; flex-direction: row !important; align-items: center !important; height: auto !important; min-height: 44px !important; }\n");
            css_text.push_str("#gs_hdr_hp_lgow { margin: 0 !important; height: auto !important; line-height: normal !important; }\n");
            // The homepage body is a `display:table`; keep it as a block so it
            // sits under the static header and center its content.
            css_text.push_str("#gs_bdy { display: block !important; }\n");
            css_text.push_str(
                "#gs_bdy_ccl { display: block !important; text-align: center !important; }\n",
            );
        }
    }
    // AP News body/content modules rely on CSS custom properties set inline, but
    // our resolver evaluates matched stylesheet rules before collecting inline
    // custom properties, so `[data-module] { background-color:var(...) }` still
    // resolves to black and the whole page body paints solid black. Stamp a
    // transparent background inline on the affected container/module elements so
    // the white page body shows through.
    if base_url.contains("apnews.com") {
        // AP modules set --color-module-background:transparent inline, but our
        // resolver evaluates matched stylesheet rules before collecting inline
        // custom properties, so [data-module] { background-color:var(...) } still
        // resolves to black. Stamp a transparent background inline on the affected
        // container/module elements so they paint correctly.
        for node in doc.nodes.iter_mut() {
            if let incognidium_dom::NodeData::Element(ref mut el) = node.data {
                let cls = el.get_attr("class").unwrap_or("");
                let classes: std::collections::HashSet<&str> = cls.split_whitespace().collect();
                let has_data_module = el.attributes.contains_key("data-module");
                let is_container = classes.contains("TwoColumnContainer7030")
                    || classes.contains("PageListStandardE");
                let is_inverse = el.attributes.contains_key("data-inverse-colors")
                    || el.attributes.contains_key("data-inverse-container-colors");
                if (has_data_module || is_container) && !is_inverse {
                    let style_attr = el
                        .attributes
                        .entry("style".to_string())
                        .or_insert_with(String::new);
                    if !style_attr.is_empty() && !style_attr.ends_with(';') {
                        style_attr.push(';');
                    }
                    style_attr.push_str("background-color: transparent;");
                }
            }
        }
    }

    // The NYTimes homepage video feed is a horizontal carousel built with CSS
    // container queries and `grid-auto-flow: column`. Our layout engine does not
    // implement container queries or implicit grid columns, so the feed items
    // stack vertically as full-width 2/3-aspect-ratio cards and inflate the page
    // by ~6000 px. Convert the feed into a compact wrapping row of fixed-width
    // cards so the headlines stay visible without dominating the static render.
    if base_url.contains("nytimes.com") {
        css_text.push_str(
            r#"
nyt-video-feed { display: block !important; }
nyt-video-feed [class*="_feed-promo_"] { display: flex !important; flex-wrap: wrap !important; }
nyt-video-feed [class*="_feed_"] { display: flex !important; flex-wrap: wrap !important; height: auto !important; }
nyt-video-feed article { display: inline-block !important; width: 220px !important; height: auto !important; padding-right: 16px !important; }
nyt-video-feed [class*="_player-container_"] { height: 140px !important; }
nyt-video-feed nyt-betamax-poster img { max-height: 140px !important; width: auto !important; }
"#,
        );
    }

    let mut stylesheet = parse_css(&css_text);
    let mut styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

    // Fetch CSS background-image URLs (e.g. article-card covers on TIME/Vox).
    // These are not <img> tags, so fetch_page_images misses them.
    let bg_images = fetch_background_images(&styles, &base_url, &image_cache);
    eprintln!("Background images: {} fetched", bg_images.len());
    for (src, data) in bg_images {
        image_cache.insert(src, data);
    }

    // Some sites (e.g. Politico / The Guardian) serve HTML with the root or body
    // hidden (`display: none` or `visibility: hidden`) as an anti-bot or
    // hydration measure. Counter it by injecting a high-specificity CSS rule
    // and also stamping an inline style override on the affected root elements.
    let html_id = doc.document_element();
    let body_id = doc.body();
    let mut hidden_root = false;
    let mut stamp_style = |id: incognidium_dom::NodeId| {
        let node = doc.node_mut(id);
        if let incognidium_dom::NodeData::Element(ref mut el) = node.data {
            let style_attr = el
                .attributes
                .entry("style".to_string())
                .or_insert_with(String::new);
            if !style_attr.is_empty() && !style_attr.ends_with(';') {
                style_attr.push(';');
            }
            style_attr.push_str("display:block !important; visibility:visible !important;");
        }
    };
    if let Some(id) = html_id {
        if let Some(style) = styles.get(&id) {
            let hidden_by_visibility =
                !matches!(style.visibility, incognidium_style::Visibility::Visible);
            let hidden_by_display = matches!(style.display, incognidium_style::Display::None);
            if hidden_by_visibility || hidden_by_display {
                eprintln!(
                    "Detected html display={:?} visibility={:?}; injecting visible override",
                    style.display, style.visibility
                );
                stamp_style(id);
                hidden_root = true;
            }
        }
    }
    if let Some(id) = body_id {
        if let Some(style) = styles.get(&id) {
            let hidden_by_visibility =
                !matches!(style.visibility, incognidium_style::Visibility::Visible);
            let hidden_by_display = matches!(style.display, incognidium_style::Display::None);
            if hidden_by_visibility || hidden_by_display {
                eprintln!(
                    "Detected body display={:?} visibility={:?}; injecting visible override",
                    style.display, style.visibility
                );
                stamp_style(id);
                hidden_root = true;
            }
        }
    }
    if hidden_root {
        css_text
            .push_str("\nhtml { visibility: visible !important; display: block !important; }\n");
        css_text.push_str(
            "\nhtml body { visibility: visible !important; display: block !important; }\n",
        );
        stylesheet = parse_css(&css_text);
        styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
    }

    // DuckDuckGo's client-side hydration sometimes leaves the search form hidden
    // (no ssg-ai-searchbox-mode-* class on <html>). Force the hero and searchbox
    // wrappers into the flow so we can extract their text even when the page JS
    // hasn't fully upgraded the SSR shell.
    if base_url.contains("duckduckgo.com") {
        css_text.push_str(
            "\n[data-testid=\"home-hero\"], [data-testid=\"searchbox-form\"] { display: block !important; visibility: visible !important; }\n",
        );
        css_text.push_str(
            "\n[data-testid=\"home-hero\"] *, [data-testid=\"searchbox-form\"] * { display: block !important; visibility: visible !important; }\n",
        );
        stylesheet = parse_css(&css_text);
        styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
    }

    // Fox News server-rendered markup hides the header "Log In" button via
    // `.site-header .button.user-login { visibility:hidden }` and expects JS to
    // add a `.show` class. In our headless run that class is never added, so the
    // button is missing from the rendered top nav compared to Firefox. Force it
    // visible so the right-side meta bar matches the browser.
    if base_url.contains("foxnews.com") {
        css_text
            .push_str("\n.site-header .button.user-login { visibility: visible !important; }\n");
        stylesheet = parse_css(&css_text);
        styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
    }

    // The Guardian's top "highlights" carousel uses 300px-wide cards inside a
    // six-column grid at our 1024px viewport. Our grid layout resolves the
    // tracks to 1fr (~140px) and the fixed-width cards overflow their tracks,
    // so the cards paint on top of each other. Make the tracks match the card
    // width so the carousel lays out as a single horizontal row, matching the
    // visible portion in Firefox before horizontal scrolling.
    if base_url.contains("theguardian.com") {
        css_text.push_str(
            "\n.dcr-ymwzpl .dcr-wde3dn { grid-template-columns: repeat(6, 300px) !important; }\n",
        );
        // The veggie-burger menu is server-rendered expanded and only collapsed by
        // a checkbox once JS runs. Hide the expanded menu root so it does not
        // overlay the masthead and headline area.
        css_text.push_str("#header-expanded-menu-root, #header-expanded-menu, #header-veggie-burger, #header-nav-input-checkbox { display: none !important; }\n");
        stylesheet = parse_css(&css_text);
        styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
    }

    // 9to5Mac's desktop header has a blue navigation bar inside
    // `.header-bottom` that the site's CSS keeps at `height: 0` and
    // `position: absolute` for the mobile drawer state. A matching
    // `[aria-hidden=false]` attribute selector then forces `display: flex`, so the
    // bar's children overflow onto the white header without any visible
    // background. At the 1024px desktop viewport we force the bar into normal
    // visible flow so the desktop navigation renders as a real blue bar.
    if base_url.contains("9to5mac.com") {
        css_text.push_str(
            ".header-bottom { position: relative !important; height: auto !important; display: block !important; overflow: visible !important; }\n",
        );
        css_text.push_str(".header-bottom .primary-menu-ul { display: flex !important; }\n");
        stylesheet = parse_css(&css_text);
        styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
    }

    if let Some(ref css_path) = css_output {
        std::fs::write(css_path, &css_text).expect("write css dump");
        eprintln!("Combined CSS dumped to {css_path}");
    }

    // Dump resolved computed styles for diagnostic inspection of layout collapse.
    if let Some(ref styles_path) = styles_output {
        let mut entries: Vec<(incognidium_dom::NodeId, &incognidium_style::ComputedStyle)> =
            styles.iter().map(|(id, s)| (*id, s)).collect();
        entries.sort_by_key(|e| e.0);
        let mut out = String::new();
        for (id, s) in entries {
            let (tag, cls) = if let Some(node) = doc.nodes.get(id) {
                match &node.data {
                    incognidium_dom::NodeData::Element(ref e) => (
                        e.tag_name.clone(),
                        e.get_attr("class").unwrap_or("").to_string(),
                    ),
                    _ => (String::from("#text"), String::new()),
                }
            } else {
                (String::new(), String::new())
            };
            out.push_str(&format!(
                "node={} tag={} class={} display={:?} pos={:?} float={:?} width={:?} height={:?} max_h={:?} min_h={:?} max_w={:?} min_w={:?} flex_grow={:.2} flex_shrink={:.2} flex_basis={:?} top={:?} left={:?} right={:?} bottom={:?} margin_left={:.1}(auto={}) margin_right={:.1}(auto={}) padding_left={:.1} padding_right={:.1} box_sizing={:?} grid_area={:?} transform={:?} opacity={:.2} color={:?} bg={:?} bg_img={:?} grid_cols={:?} grid_rows={:?} grid_auto_cols={:?} grid_auto_flow={:?} col_gap={:.1} row_gap={:.1} col_start={:?} col_end={:?} col_span={:?} row_start={:?} row_end={:?} row_span={:?} flex_direction={:?} font_size={:.1} line_height={:.2}\n",
                id,
                tag,
                cls.chars().take(60).collect::<String>(),
                s.display,
                s.position,
                s.float,
                s.width,
                s.height,
                s.max_height,
                s.min_height,
                s.max_width,
                s.min_width,
                s.flex_grow,
                s.flex_shrink,
                s.flex_basis,
                s.top,
                s.left,
                s.right,
                s.bottom,
                s.margin_left,
                s.margin_left_auto,
                s.margin_right,
                s.margin_right_auto,
                s.padding_left,
                s.padding_right,
                s.box_sizing,
                s.grid_area,
                s.transform,
                s.opacity,
                s.color,
                s.background_color,
                s.background_image,
                s.grid_template_columns,
                s.grid_template_rows,
                s.grid_auto_columns,
                s.grid_auto_flow,
                s.column_gap,
                s.row_gap,
                s.grid_column_start,
                s.grid_column_end,
                s.grid_column_span,
                s.grid_row_start,
                s.grid_row_end,
                s.grid_row_span,
                s.flex_direction,
                s.font_size,
                s.line_height
            ));
        }
        std::fs::write(styles_path, out).expect("write styles dump");
        eprintln!("Computed styles dumped to {styles_path}");
    }

    // Rasterize simple inline SVG icons/logos now that styles are resolved so
    // `currentColor` can be substituted with the computed element color.
    rasterize_inline_svgs(
        &mut doc,
        &mut image_cache,
        Some(&mut styles),
        1024.0,
        768.0,
        Some(&base_url),
    );

    // The SVG placeholders are now <img> elements, so re-resolve styles so that
    // author rules targeting `img` (e.g. `max-height: 100%`) apply to them.
    styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

    // Build image sizes map for layout
    let mut image_sizes = ImageSizes::new();
    for (src, img) in &image_cache {
        image_sizes.insert(src.clone(), (img.width, img.height));
    }

    let mut layout_root = layout_with_images(&doc, &styles, 1024.0, 768.0, &image_sizes);

    // Container-query pass: measure the containers from the first layout, then
    // re-resolve styles with real container sizes and lay out again.  This is
    // required for modern card layouts (AP News, BBC, etc.) that choose a grid
    // based on their parent container rather than the viewport.
    let mut container_sizes = HashMap::new();
    collect_container_sizes(&layout_root, &styles, &mut container_sizes);
    if !container_sizes.is_empty() {
        styles = resolve_styles_with_containers(&doc, &stylesheet, 1024.0, 768.0, &container_sizes);
        // Re-rasterize inline SVGs with the updated styles and refresh the
        // image-size map for the final layout.
        rasterize_inline_svgs(
            &mut doc,
            &mut image_cache,
            Some(&mut styles),
            1024.0,
            768.0,
            Some(&base_url),
        );
        image_sizes.clear();
        for (src, img) in &image_cache {
            image_sizes.insert(src.clone(), (img.width, img.height));
        }
        layout_root = layout_with_images(&doc, &styles, 1024.0, 768.0, &image_sizes);
    }

    if std::env::var("DUMP_IMAGE_SRC").is_ok() {
        fn walk_images(
            b: &incognidium_layout::LayoutBox,
            doc: &incognidium_dom::Document,
            image_sizes: &incognidium_layout::ImageSizes,
        ) {
            if b.box_type == incognidium_layout::BoxType::Image {
                let src = b.image_src.as_deref().unwrap_or("(none)");
                let tag = match &doc.nodes[b.node_id].data {
                    incognidium_dom::NodeData::Element(ref e) => e.tag_name.clone(),
                    _ => String::new(),
                };
                let dims = image_sizes.get(src);
                eprintln!(
                    "IMAGE node={} tag={} src={} dims={:?}",
                    b.node_id, tag, src, dims
                );
            }
            for c in &b.children {
                walk_images(c, doc, image_sizes);
            }
        }
        eprintln!("IMAGE SRC DEBUG:");
        walk_images(&layout_root, &doc, &image_sizes);
    }

    if std::env::var("DUMP_LAYOUT_TREE").is_ok() {
        dump_layout_tree(&layout_root, &doc, &styles, 0);
    }

    let flat_boxes = flatten_layout(&layout_root, 0.0, 0.0, &styles);
    eprintln!("{} flat boxes", flat_boxes.len());

    // Debug: print all flat boxes when very few are produced (layout collapse diagnosis)
    if flat_boxes.len() <= 5 || std::env::var("DUMP_BOXES").is_ok() {
        eprintln!("All flat boxes:");
        for fb in &flat_boxes {
            let preview = fb.text.as_deref().unwrap_or("(no text)");
            let (tag, cls) = if fb.node_id >= doc.nodes.len() {
                (String::from("::pseudo"), String::new())
            } else {
                match &doc.nodes[fb.node_id].data {
                    incognidium_dom::NodeData::Element(ref e) => (
                        e.tag_name.clone(),
                        e.get_attr("class").unwrap_or("").to_string(),
                    ),
                    _ => (String::from("#text"), String::new()),
                }
            };
            eprintln!(
                "  node={} [{:.0},{:.0} {}x{}] type={:?} tag={} class={} clip={:?} first={:?} root={:?} text={}",
                fb.node_id,
                fb.x,
                fb.y,
                fb.width,
                fb.height,
                fb.box_type,
                tag,
                &cls[..cls.len().min(60)],
                fb.clip,
                fb.first_letter_len,
                fb.stacking_context_root,
                preview.chars().take(60).collect::<String>()
            );
        }
    }

    // Count text boxes (exclude images - alt text should not render). Also drop
    // boxes that are positioned entirely off-screen horizontally; off-canvas
    // menus and skip links should not inflate the text-extraction signal.
    let viewport_width_for_text: f32 = 1024.0;
    let text_boxes: Vec<_> = flat_boxes
        .iter()
        .filter(|b| {
            b.text.is_some()
                && b.box_type != incognidium_layout::BoxType::Image
                && !is_box_offscreen(b, viewport_width_for_text)
        })
        .collect();
    eprintln!("{} text boxes", text_boxes.len());
    for tb in text_boxes.iter().take(10) {
        if let Some(ref t) = tb.text {
            let preview: String = t.chars().take(80).collect();
            eprintln!(
                "  [{:.0},{:.0} {}x{}] \"{}\"",
                tb.x, tb.y, tb.width, tb.height, preview
            );
        }
    }
    // Count images
    let img_count = flat_boxes
        .iter()
        .filter(|b| b.box_type == incognidium_layout::BoxType::Image)
        .count();
    eprintln!("{} image boxes", img_count);

    // Dump all flat boxes (with text, if any) for debugging layout/text drops.
    if let Some(ref path) = boxes_output {
        let mut out = String::new();
        for fb in flat_boxes.iter() {
            let text = fb.text.as_ref().map(|t| t.as_str()).unwrap_or("");
            let (tag, cls) = if fb.node_id >= doc.nodes.len() {
                ("::pseudo", "")
            } else {
                match doc.node(fb.node_id).data {
                    incognidium_dom::NodeData::Element(ref e) => {
                        (e.tag_name.as_str(), e.get_attr("class").unwrap_or(""))
                    }
                    _ => ("#text", ""),
                }
            };
            let bg = styles
                .get(&fb.node_id)
                .map(|s| format!("{:?}", s.background_color))
                .unwrap_or_default();
            out.push_str(&format!(
                "node={} tag={} class={} [{:.1},{:.1} {:.1}x{:.1}] type={:?} bg={} text={}\n",
                fb.node_id,
                tag,
                cls,
                fb.x,
                fb.y,
                fb.width,
                fb.height,
                fb.box_type,
                bg,
                text.replace('\n', " ")
            ));
        }
        std::fs::write(path, out).expect("write boxes dump");
        eprintln!("Flat boxes dumped to {path}");
    }

    // Size height to fit content, but keep output practical. Very long pages
    // (e.g. Wikipedia articles) can produce 100k+ px images that OOM the PNG
    // encoder. Default cap keeps normal article pages intact while preventing
    // extreme full-page captures; allow override via INCOGNIDIUM_MAX_PNG_HEIGHT.
    let max_png_height: u32 = std::env::var("INCOGNIDIUM_MAX_PNG_HEIGHT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(40_000)
        .max(200);
    // Fixed-positioned subtrees are viewport-relative and do not contribute
    // to the normal-flow document height. Subtrees that are positioned entirely
    // outside the 1024px viewport horizontally (off-canvas menus, right rails
    // placed past the viewport by grid bugs, etc.) should not extend the
    // screenshot either. Keep boxes that are at least partially visible, including
    // visible absolute boxes (xkcd.com's centered body, GitHub's mispositioned
    // body) when they overlap the viewport.
    let viewport_width: f32 = 1024.0;
    let content_height = flat_boxes
        .iter()
        .filter(|b| {
            let off_screen = b.x >= viewport_width || b.x + b.width <= 0.0;
            let hidden = styles
                .get(&b.node_id)
                .map(|s| matches!(s.visibility, incognidium_style::Visibility::Hidden))
                .unwrap_or(false);
            !b.in_fixed_subtree && !off_screen && !(b.in_absolute_subtree && hidden)
        })
        .map(|b| (b.y + b.height) as u32)
        .max()
        .unwrap_or(768)
        .max(200)
        + 20;
    let render_height = content_height.min(max_png_height);

    // Optional wait for JS rendering
    if wait_ms > 0 {
        eprintln!("Waiting {}ms for JS rendering...", wait_ms);
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));
    }

    let pixmap = paint_with_images(&flat_boxes, &styles, 1024, render_height, &image_cache);
    save_png_compressed(&pixmap, std::path::Path::new(&output)).expect("save png");
    eprintln!("Saved to {output} ({}x{})", 1024, render_height);

    // Extract and save text content
    let mut all_text: Vec<(f32, f32, String)> = Vec::new();
    for fbox in &flat_boxes {
        // Skip image boxes - alt text should not be rendered as content
        if fbox.box_type == incognidium_layout::BoxType::Image {
            continue;
        }
        // Skip hidden/collapsed text (e.g. ::before/::after accessibility helpers)
        let vis = styles
            .get(&fbox.node_id)
            .map(|s| s.visibility)
            .unwrap_or(incognidium_style::Visibility::Visible);
        if !matches!(vis, incognidium_style::Visibility::Visible) {
            continue;
        }
        // Skip off-canvas text boxes (hidden menus, skip links).
        if is_box_offscreen(fbox, viewport_width) {
            continue;
        }
        let mut added = false;
        if let Some(ref t) = fbox.text {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                all_text.push((fbox.y, fbox.x, trimmed.to_string()));
                added = true;
            }
        }
        if !added {
            if fbox.node_id >= doc.nodes.len() {
                continue;
            }
            if let incognidium_dom::NodeData::Element(ref el) = doc.nodes[fbox.node_id].data {
                // Inputs and buttons often carry their labels as placeholder or ARIA
                // attributes instead of child text nodes, so include those too.
                if matches!(
                    el.tag_name.as_str(),
                    "input" | "textarea" | "button" | "select" | "a" | "area"
                ) {
                    for attr in ["placeholder", "aria-label", "title", "value", "alt"] {
                        if let Some(val) = el.attributes.get(attr) {
                            let trimmed = val.trim();
                            if !trimmed.is_empty() {
                                all_text.push((fbox.y, fbox.x, trimmed.to_string()));
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    // Sort by position (top to bottom, left to right)
    all_text.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap()
            .then(a.1.partial_cmp(&b.1).unwrap())
    });

    // Merge into readable paragraphs (group text at same Y position into lines)
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut last_y: f32 = -100.0;
    for (y, _x, text) in &all_text {
        if (y - last_y).abs() > 4.0 {
            if !current_line.is_empty() {
                lines.push(std::mem::take(&mut current_line));
            }
        } else if !current_line.is_empty() {
            current_line.push(' ');
        }
        current_line.push_str(text);
        last_y = *y;
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    let flat_text = lines.join("\n");
    let flat_words = flat_text.split_whitespace().count();

    // If the layout engine produced very little visible text, fall back to a DOM
    // traversal that respects computed display/visibility. This catches pages where
    // CSS positioning or absolute/fixed layout prevents boxes from forming.
    let dom_text = extract_dom_text(&doc, &styles, &flat_boxes, viewport_width);
    let dom_words = dom_text.split_whitespace().count();

    let mut extracted_text = if dom_words > flat_words {
        eprintln!(
            "Using DOM fallback text ({} words) over flat boxes ({} words)",
            dom_words, flat_words
        );
        dom_text
    } else {
        flat_text
    };

    // When visible text is still sparse, prepend page metadata so the extraction
    // isn't completely empty for blocked or JS-heavy sites.
    if extracted_text.split_whitespace().count() < 30 {
        let meta = extract_page_metadata(&doc);
        if !meta.is_empty() {
            let meta_text = meta.join("\n");
            extracted_text = format!("{}\n{}", meta_text, extracted_text);
        }
    }

    eprintln!(
        "Extracted {} lines of text ({} text fragments; {} words)",
        lines.len(),
        all_text.len(),
        extracted_text.split_whitespace().count()
    );

    // Always print to stderr for piping
    if let Some(ref text_path) = text_output {
        std::fs::write(text_path, &extracted_text).expect("write text file");
        eprintln!("Text saved to {text_path}");
    }

    // Only print text to stdout when no --text path is provided. When --text is
    // used the extracted text is already written to a file, and dumping the same
    // content to stdout can fill OS pipe buffers and deadlock callers on large
    // pages (e.g. Wikipedia).
    if text_output.is_none() {
        println!("{}", extracted_text);
    }
}

/// Fetch CSS from <link rel="stylesheet"> tags and follow @import rules.
fn fetch_external_css(doc: &incognidium_dom::Document, base_url: &str) -> String {
    const MAX_STYLESHEETS: usize = 60;
    const MAX_CSS_SIZE: usize = 4 * 1024 * 1024; // 4MB per stylesheet
    let mut css = String::new();
    let mut fetched = 0usize;
    let mut to_fetch: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    // First collect all <link> stylesheets in document order. Pages built with
    // preloaded stylesheet patterns (e.g. TownNews/Bootstrap) can reference 30+
    // stylesheets; processing them FIFO keeps critical base styles like Bootstrap
    // from being dropped by a small LIFO limit.
    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "link" {
                let rel = el.get_attr("rel").unwrap_or_default().to_ascii_lowercase();
                let as_attr = el.get_attr("as").unwrap_or_default().to_ascii_lowercase();
                let is_stylesheet = rel
                    .split_whitespace()
                    .any(|t| t.eq_ignore_ascii_case("stylesheet"))
                    || (rel
                        .split_whitespace()
                        .any(|t| t.eq_ignore_ascii_case("preload"))
                        && as_attr.eq_ignore_ascii_case("style"));
                if is_stylesheet {
                    // Skip print-only stylesheets unless the link has an onload
                    // handler that will flip the media to "all" (common perf pattern:
                    // <link rel="stylesheet" href="..." media="print" onload="this.media='all'">).
                    let mut skip_print = true;
                    if let Some(media) = el.get_attr("media") {
                        let media_lower = media.to_ascii_lowercase();
                        if media_lower.eq("print") {
                            if let Some(onload) = el.get_attr("onload") {
                                let lower = onload.to_lowercase();
                                if lower.contains("this.media")
                                    && (lower.contains("'all'") || lower.contains("\"all\""))
                                {
                                    skip_print = false;
                                }
                            }
                            if skip_print {
                                continue;
                            }
                        }
                        // Drop stylesheets that target only the dark color scheme. If a
                        // stylesheet covers light or no-preference as well (e.g. Nature's
                        // `media="..., (prefers-color-scheme: light), ..."`) we need it for the
                        // light-theme comparison and only the inner dark @media blocks should be
                        // stripped later.
                        if media_lower.contains("prefers-color-scheme")
                            && media_lower.contains("dark")
                            && !media_lower.contains("light")
                            && !media_lower.contains("no-preference")
                        {
                            continue;
                        }
                    }
                    if let Some(href) = el.get_attr("href") {
                        if let Ok(resolved) = resolve_url(base_url, href) {
                            to_fetch.push_back(resolved);
                        }
                    }
                }
            }
        }
    }

    let link_stylesheets = to_fetch.len();

    // Fetch stylesheets and follow @import rules
    let mut fetched_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(url) = to_fetch.pop_front() {
        if fetched >= MAX_STYLESHEETS {
            let remaining = to_fetch.len();
            eprintln!(
                "CSS fetch limit reached ({} stylesheets); {} remaining link/imports skipped",
                fetched, remaining
            );
            break;
        }
        if fetched_urls.contains(&url) {
            continue;
        }
        fetched_urls.insert(url.clone());

        if let Ok(resp) = fetch_url(&url) {
            if resp.status < 200 || resp.status >= 300 {
                eprintln!(
                    "Skipping CSS {}: HTTP {} ({} bytes)",
                    url,
                    resp.status,
                    resp.body.len()
                );
                continue;
            }
            eprintln!("Fetched CSS: {} ({} bytes)", url, resp.body.len());
            if resp.body.len() <= MAX_CSS_SIZE {
                // Extract @import rules and fetch them after the current link queue
                // so document-order stylesheets take priority.
                let imports = extract_imports(&resp.body);
                for import_url in imports {
                    if let Ok(resolved) = resolve_url(&url, &import_url) {
                        if !fetched_urls.contains(&resolved) {
                            to_fetch.push_back(resolved);
                        }
                    }
                }
                css.push_str(&resp.body);
                css.push('\n');
                fetched += 1;
            } else {
                eprintln!("Skipping CSS: {} exceeds {} byte limit", url, MAX_CSS_SIZE);
            }
        } else {
            eprintln!("Failed to fetch CSS: {}", url);
        }
    }
    eprintln!(
        "Combined CSS from {} of {} linked stylesheets ({} bytes)",
        fetched,
        link_stylesheets,
        css.len()
    );
    css
}

/// Extract @import URLs from CSS (basic parsing)
fn extract_imports(css: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in css.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("@import") {
            // Extract URL from @import rule
            // @import url("...") or @import "..." or @import '...'
            if let Some(start) = trimmed.find('"').or_else(|| trimmed.find('\'')) {
                if let Some(end) = trimmed[start + 1..]
                    .find('"')
                    .or_else(|| trimmed[start + 1..].find('\''))
                {
                    let url = &trimmed[start + 1..start + 1 + end];
                    imports.push(url.to_string());
                }
            }
        }
    }
    imports
}

/// Fetch images from the page (blocking, with parallelism).
fn decode_svg(bytes: &[u8]) -> Result<ImageData, String> {
    let mut opt = usvg::Options::default();
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_system_fonts();
    opt.fontdb = std::sync::Arc::new(fontdb);
    let tree = usvg::Tree::from_data(bytes, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    // SVGs from sites (e.g. ABC News' logo.svg) can declare a huge viewBox but
    // are used as small background images. Render them at a reasonable max
    // dimension so they fit in memory and the image cache.
    const MAX_SVG_DIM: f32 = 4096.0;
    let scale = (MAX_SVG_DIM / size.width().max(size.height())).min(1.0);
    let w = (size.width() * scale).ceil().max(1.0) as u32;
    let h = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("pixmap")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia uses premultiplied BGRA; convert to RGBA straight
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in pixmap.pixels() {
        let a = px.alpha();
        // Demultiply if alpha > 0
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let inv = 255.0 / a as f32;
            out.push(((px.red() as f32 * inv).min(255.0)) as u8);
            out.push(((px.green() as f32 * inv).min(255.0)) as u8);
            out.push(((px.blue() as f32 * inv).min(255.0)) as u8);
            out.push(a);
        }
    }
    Ok(ImageData {
        pixels: out,
        width: w,
        height: h,
    })
}

fn decode_and_downscale_image(bytes: &[u8], is_svg: bool) -> Option<ImageData> {
    if is_svg {
        return decode_svg(bytes).ok();
    }
    let mut img = image::load_from_memory(bytes).ok()?;
    let (w, h) = img.dimensions();
    if w > MAX_IMAGE_DIMENSION || h > MAX_IMAGE_DIMENSION {
        let ratio = (w as f32).max(h as f32) / MAX_IMAGE_DIMENSION as f32;
        let new_w = ((w as f32) / ratio).max(1.0) as u32;
        let new_h = ((h as f32) / ratio).max(1.0) as u32;
        img = img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3);
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(ImageData {
        pixels: rgba.into_raw(),
        width: w,
        height: h,
    })
}

/// For `<img srcset="...">` elements, pick the best source for the rendered
/// viewport width and rewrite the `src` attribute to that URL. This lets the
/// image fetcher and layout engine use a valid responsive URL instead of a
/// fallback `src` that may be rejected (e.g. PBS's mezzanine hero with a
/// fractional `resize=1700x956.25` parameter).
fn select_srcset_images(doc: &mut incognidium_dom::Document, base_url: &str, viewport_width: f32) {
    for node_id in 0..doc.nodes.len() {
        let node = &mut doc.nodes[node_id];
        let (is_img, srcset_attr) = match &mut node.data {
            incognidium_dom::NodeData::Element(ref mut el) if el.tag_name == "img" => {
                (true, el.attributes.get("srcset").cloned())
            }
            _ => continue,
        };
        if !is_img {
            continue;
        }
        let Some(srcset) = srcset_attr else { continue };
        let Some(selected) = select_srcset_url(&srcset, viewport_width) else {
            continue;
        };
        // Resolve relative URLs against the page base.
        let resolved = resolve_url(base_url, &selected).unwrap_or(selected);
        if let incognidium_dom::NodeData::Element(ref mut el) = &mut doc.nodes[node_id].data {
            el.attributes.insert("src".to_string(), resolved);
        }
    }
}

/// Parse a `srcset` attribute and pick the source whose descriptor is closest
/// to `target_width`. Width descriptors (`320w`) are preferred; density
/// descriptors (`1x`, `2x`) fall back to the 1x source. If no source is at
/// least `target_width` wide we take the largest available.
///
/// The parser is tolerant of commas inside URLs (e.g. WordPress image URLs
/// such as `?resize=50,33`), which the naive `split(',')` approach mishandles.
fn select_srcset_url(srcset: &str, target_width: f32) -> Option<String> {
    #[derive(Debug, Clone)]
    struct Candidate {
        url: String,
        descriptor: f32, // width in px for w descriptors, density for x descriptors
        is_width: bool,
    }

    fn token_is_descriptor(tok: &str) -> bool {
        let body = tok.trim();
        if body.len() < 2 {
            return false;
        }
        let (num, suffix) = body.split_at(body.len() - 1);
        matches!(suffix, "w" | "x") && num.parse::<f32>().is_ok() && !num.parse::<usize>().is_err()
        // any numeric
    }

    fn has_width_descriptor(entry: &str) -> bool {
        entry.split_whitespace().any(token_is_descriptor)
    }

    fn looks_like_new_candidate(text: &str) -> bool {
        let first = text.split_whitespace().next().unwrap_or("");
        !first.is_empty()
            && (first.starts_with("http")
                || first.starts_with("//")
                || first.starts_with('/')
                || first.chars().next().unwrap().is_alphabetic())
    }

    let parts: Vec<&str> = srcset.split(',').collect();
    let mut entries: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, part) in parts.iter().enumerate() {
        if !current.is_empty() {
            current.push(',');
        }
        current.push_str(part);
        if has_width_descriptor(&current) {
            entries.push(current.trim().to_string());
            current.clear();
        } else if i + 1 < parts.len() && looks_like_new_candidate(parts[i + 1]) {
            // This candidate has no descriptor and the next part starts a new
            // URL; treat it as the default 1x source.
            entries.push(current.trim().to_string());
            current.clear();
        }
    }
    if !current.is_empty() {
        entries.push(current.trim().to_string());
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for entry in entries {
        if entry.is_empty() {
            continue;
        }
        let mut parts = entry.split_whitespace();
        let url = parts.next()?.to_string();
        let mut descriptor = None;
        for tok in parts {
            if token_is_descriptor(tok) {
                let body = tok.trim();
                let suffix = body.chars().last().unwrap();
                let num = body[..body.len() - 1].parse::<f32>().ok()?;
                descriptor = Some((num, suffix == 'w'));
                break;
            }
        }
        let (desc, is_width) = descriptor.unwrap_or((1.0, false));
        candidates.push(Candidate {
            url,
            descriptor: desc,
            is_width,
        });
    }

    if candidates.is_empty() {
        return None;
    }

    // Prefer width descriptors; they are what responsive images use.
    let width_candidates: Vec<Candidate> =
        candidates.iter().filter(|c| c.is_width).cloned().collect();
    if !width_candidates.is_empty() {
        let chosen = width_candidates
            .iter()
            .filter(|c| c.descriptor >= target_width)
            .min_by(|a, b| {
                a.descriptor
                    .partial_cmp(&b.descriptor)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .or_else(|| {
                width_candidates.iter().max_by(|a, b| {
                    a.descriptor
                        .partial_cmp(&b.descriptor)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            });
        return chosen.map(|c| c.url.clone());
    }

    // Fallback for density descriptors: pick 1x (or closest to 1x).
    candidates
        .iter()
        .filter(|c| !c.is_width)
        .min_by(|a, b| {
            let da = (a.descriptor - 1.0).abs();
            let db = (b.descriptor - 1.0).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|c| c.url.clone())
}

/// Heuristic to skip small HTML error pages that servers return instead of an
/// image. XML declarations are valid for SVGs and must not be rejected here.
fn looks_like_html_document(bytes: &[u8]) -> bool {
    if bytes.len() >= 4000 {
        return false;
    }
    let prefix = &bytes[..bytes.len().min(64)];
    prefix.to_ascii_lowercase().starts_with(b"<!doctype html")
        || prefix.to_ascii_lowercase().starts_with(b"<html")
}

fn fetch_page_images(doc: &incognidium_dom::Document, base_url: &str) -> Vec<(String, ImageData)> {
    // Pages like MLB.com include dozens of tiny team-logo SVGs before the
    // visible hero image, so a low cap leaves later, larger images unloaded and
    // collapses their containers. Allow more images while still bounding the
    // work for very image-heavy pages.
    const MAX_IMAGES: usize = 500;
    let mut urls: Vec<(String, String)> = Vec::new();
    let mut results: Vec<(String, ImageData)> = Vec::new();

    for node in &doc.nodes {
        if results.len() + urls.len() >= MAX_IMAGES {
            break;
        }
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "img" {
                if let Some(src) = el.get_attr("src") {
                    if src.starts_with("data:") {
                        // Decode data URI inline
                        if let Some(img) = decode_data_uri_image(src) {
                            results.push((src.to_string(), img));
                        }
                        continue;
                    }
                    if is_inline_svg_url(&src) {
                        continue;
                    }
                    if let Ok(resolved) = resolve_url(base_url, src) {
                        urls.push((src.to_string(), resolved));
                    }
                }
            }
        }
    }

    if urls.is_empty() {
        return results;
    }

    let mut results = Vec::new();

    // Sequential fetches with a short pause between requests. Wikimedia Commons
    // in particular rate-limits heavily on a per-IP basis, so we throttle more
    // aggressively for upload.wikimedia.org than for other hosts.
    let is_wikimedia = urls
        .iter()
        .any(|(_, resolved)| resolved.to_lowercase().contains("upload.wikimedia.org"));
    let chunk_size: usize = if is_wikimedia { 1 } else { 5 };
    let pause_ms: u64 = if is_wikimedia { 500 } else { 100 };
    for (ci, chunk) in urls.chunks(chunk_size).enumerate() {
        if ci > 0 {
            std::thread::sleep(std::time::Duration::from_millis(pause_ms));
        }
        let handles: Vec<_> = chunk
            .iter()
            .map(|(src, resolved)| {
                let src = src.clone();
                let resolved = resolved.clone();
                std::thread::spawn(move || {
                    match fetch_bytes(&resolved) {
                        Ok(bytes) => {
                            if looks_like_html_document(&bytes) {
                                return None;
                            }
                            let is_svg = resolved.to_lowercase().ends_with(".svg")
                                || bytes.windows(4).take(512).any(|w| w == b"<svg");
                            if let Some(img) = decode_and_downscale_image(&bytes, is_svg) {
                                return Some((src, img));
                            }
                        }
                        Err(_) => {}
                    }
                    None
                })
            })
            .collect();

        for handle in handles {
            if let Ok(Some(result)) = handle.join() {
                results.push(result);
            }
        }
    }

    results
}

/// Decode a data URI image (e.g., "data:image/png;base64,...")
fn decode_data_uri_image(uri: &str) -> Option<ImageData> {
    // Format: data:[<mediatype>][;base64],<data>
    if !uri.starts_with("data:") {
        return None;
    }

    let after_data = &uri[5..]; // Skip "data:"
    let comma_pos = after_data.find(',')?;
    let meta = &after_data[..comma_pos];
    let data_part = &after_data[comma_pos + 1..];

    // Check if base64 encoded
    let is_base64 = meta.contains("base64");
    let mime_type = meta.split(';').next().unwrap_or("");

    let bytes = if is_base64 {
        use base64::{engine::general_purpose::STANDARD, Engine};
        STANDARD.decode(data_part).ok()?
    } else {
        // URL-encoded - but if URL decoding fails, try using raw bytes
        match urlencoding::decode(data_part) {
            Ok(decoded) => decoded.into_owned().into_bytes(),
            Err(_) => {
                // URL decode failed, use raw bytes (might already be decoded)
                data_part.as_bytes().to_vec()
            }
        }
    };

    // Handle SVG
    if mime_type.contains("svg") || data_part.contains("<svg") {
        return decode_svg(&bytes).ok();
    }

    // Decode with image crate
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(ImageData {
        pixels: rgba.into_raw(),
        width: w,
        height: h,
    })
}

/// Extract data URI images from CSS background-image properties
fn extract_css_data_uri_images(css: &str) -> Vec<(String, ImageData)> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Look for background-image: url("data:...") patterns
    // This is a simplified regex-like search
    for line in css.lines() {
        // Find url(
        let mut search_start = 0;
        while let Some(url_start) = line[search_start..].find("url(") {
            let url_idx = search_start + url_start + 4; // Skip "url("
            let remaining = &line[url_idx..];

            // Find the closing paren
            let Some(close_idx) = find_closing_paren(remaining) else {
                break;
            };

            let url_content = &remaining[..close_idx];
            // Remove quotes if present
            let url_content = url_content.trim();
            let url_content = url_content.strip_prefix('"').unwrap_or(url_content);
            let url_content = url_content.strip_prefix('\'').unwrap_or(url_content);
            let url_content = url_content.strip_suffix('"').unwrap_or(url_content);
            let url_content = url_content.strip_suffix('\'').unwrap_or(url_content);

            if url_content.starts_with("data:") && !seen.contains(url_content) {
                if let Some(img) = decode_data_uri_image(url_content) {
                    seen.insert(url_content.to_string());
                    results.push((url_content.to_string(), img));
                }
            }

            search_start = url_idx + close_idx + 1;
        }
    }

    results
}

/// Fetch background-image URLs referenced by computed styles. These are used
/// by modern sites for article-card covers and hero images, not by <img> tags.
fn fetch_background_images(
    styles: &incognidium_style::StyleMap,
    base_url: &str,
    existing: &HashMap<String, ImageData>,
) -> Vec<(String, ImageData)> {
    const MAX_BG_IMAGES: usize = 50;
    let mut urls: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for style in styles.values() {
        for img in &style.background_image {
            if let incognidium_style::BackgroundImage::Url(ref src) = img {
                if src.starts_with("data:") || existing.contains_key(src) || seen.contains(src) {
                    continue;
                }
                if let Ok(resolved) = resolve_url(base_url, src) {
                    seen.insert(src.clone());
                    urls.push((src.clone(), resolved));
                    if urls.len() >= MAX_BG_IMAGES {
                        break;
                    }
                }
            }
        }
    }
    if urls.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    for (ci, chunk) in urls.chunks(8).enumerate() {
        if ci > 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let handles: Vec<_> = chunk
            .iter()
            .map(|(src, resolved)| {
                let src = src.clone();
                let resolved = resolved.clone();
                std::thread::spawn(move || {
                    match fetch_bytes(&resolved) {
                        Ok(bytes) => {
                            if looks_like_html_document(&bytes) {
                                return None;
                            }
                            let is_svg = resolved.to_lowercase().ends_with(".svg")
                                || bytes.windows(4).take(512).any(|w| w == b"<svg");
                            match decode_and_downscale_image(&bytes, is_svg) {
                                Some(img) => return Some((src, img)),
                                None => eprintln!(
                                    "Background image decode failed for {} ({} bytes, svg={})",
                                    resolved,
                                    bytes.len(),
                                    is_svg
                                ),
                            }
                        }
                        Err(e) => {
                            eprintln!("Background image fetch failed for {}: {}", resolved, e)
                        }
                    }
                    None
                })
            })
            .collect();

        for handle in handles {
            if let Ok(Some(result)) = handle.join() {
                results.push(result);
            }
        }
    }

    results
}

/// Find the index of the closing parenthesis, respecting nested parens
fn find_closing_paren(s: &str) -> Option<usize> {
    let mut depth = 1;
    for (i, c) in s.chars().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Serialize the document tree back to a minimal HTML string for debugging.
fn serialize_document_to_html(doc: &incognidium_dom::Document) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n");
    let mut visited = std::collections::HashSet::new();
    serialize_node(doc, doc.root(), &mut out, &mut visited);
    out
}

fn serialize_node(
    doc: &incognidium_dom::Document,
    node_id: incognidium_dom::NodeId,
    out: &mut String,
    visited: &mut std::collections::HashSet<incognidium_dom::NodeId>,
) {
    if !visited.insert(node_id) {
        return;
    }
    let node = &doc.nodes[node_id];
    match &node.data {
        incognidium_dom::NodeData::Document => {
            for &child in &node.children {
                serialize_node(doc, child, out, visited);
            }
        }
        incognidium_dom::NodeData::Element(el) => {
            out.push('<');
            out.push_str(&el.tag_name);
            for (k, v) in &el.attributes {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                out.push_str(&v.replace('\"', "&quot;"));
                out.push('\"');
            }
            if is_void_element(&el.tag_name) {
                out.push_str(" />");
            } else {
                out.push('>');
                for &child in &node.children {
                    serialize_node(doc, child, out, visited);
                }
                out.push_str("</");
                out.push_str(&el.tag_name);
                out.push('>');
            }
        }
        incognidium_dom::NodeData::Text(t) => {
            // Escape minimal entities for readability
            out.push_str(
                &t.content
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;"),
            );
        }
        incognidium_dom::NodeData::Comment(_) => {}
    }
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_srcset_url_picks_closest_larger_width() {
        let srcset = "a.jpg 320w, a.jpg?w=640 640w, a.jpg?w=1024 1024w";
        assert_eq!(
            select_srcset_url(srcset, 800.0),
            Some("a.jpg?w=1024".to_string())
        );
    }

    #[test]
    fn test_select_srcset_url_falls_back_to_largest_when_too_small() {
        let srcset = "a.jpg 320w, a.jpg?w=480 480w";
        assert_eq!(
            select_srcset_url(srcset, 800.0),
            Some("a.jpg?w=480".to_string())
        );
    }

    #[test]
    fn test_select_srcset_url_handles_commas_in_url() {
        // WordPress-style resize parameter with a literal comma.
        let srcset = "a.jpg?resize=50,33&quality=75 50w, a.jpg?w=744 744w, a.jpg?w=1024 1024w";
        assert_eq!(
            select_srcset_url(srcset, 800.0),
            Some("a.jpg?w=1024".to_string())
        );
    }

    #[test]
    fn test_select_srcset_url_density_fallback() {
        let srcset = "a.jpg 1x, a.jpg?retina 2x";
        assert_eq!(select_srcset_url(srcset, 800.0), Some("a.jpg".to_string()));
    }
}
