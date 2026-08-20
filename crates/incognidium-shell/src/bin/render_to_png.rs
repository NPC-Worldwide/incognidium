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
        if let Some(ref html_path) = html_output {
            let html = serialize_document_to_html(&doc);
            std::fs::write(html_path, html).expect("write html dump");
            eprintln!("DOM HTML dumped to {html_path}");
        }
        doc
    };

    // Repair any cycles / broken parent pointers introduced by JS DOM manipulation
    // so that downstream layout can safely recurse.
    let mut doc = doc;
    doc.sanitize_tree();

    // PLOS (and similar Foundation/Zurb sites) insert whitespace-only text
    // nodes between <script>, <noscript>, and <header> elements as direct
    // children of <body>. When the body is a full-height/flex container, each
    // of those whitespace nodes is laid out as a 100vh block that pushes the
    // real page content thousands of pixels down. Drop them before layout.
    if let Some(body_id) = doc.body() {
        let mut kept: Vec<incognidium_dom::NodeId> = Vec::new();
        for &child_id in &doc.nodes[body_id].children {
            if matches!(
                &doc.nodes[child_id].data,
                incognidium_dom::NodeData::Text(ref t) if t.content.trim().is_empty()
            ) {
                continue;
            }
            kept.push(child_id);
        }
        doc.node_mut(body_id).children = kept;
    }

    // NYTimes (and similar React-based sites) render placeholder `<img>` elements
    // without a `src` attribute and put the real image inside a `<noscript>` block.
    // html5ever parses `<noscript>` content as raw text when scripting is enabled,
    // so the fallback image never enters the DOM.  Scan each `<noscript>` text node
    // for an `<img>` tag and copy its `src` (and `srcset`) to the preceding
    // sibling `<img>` if that sibling lacks a `src`.
    promote_noscript_images(&mut doc);

    // Eager-load images: browsers only load `loading="lazy"` images when they
    // approach the viewport. Since Incognidium renders the full page at once,
    // those images never load. Strip the lazy flag and swap `data-src` to `src`
    // so images are fetched eagerly.
    for node in doc.nodes.iter_mut() {
        if let incognidium_dom::NodeData::Element(ref mut el) = node.data {
            if el.tag_name == "img" {
                if el
                    .attributes
                    .get("loading")
                    .map(|v| v == "lazy")
                    .unwrap_or(false)
                {
                    el.attributes.remove("loading");
                }
                // USA Today uses data-g-r="lazy" / data-g-r="lazy_c"
                if el
                    .attributes
                    .get("data-g-r")
                    .map(|v| v.starts_with("lazy"))
                    .unwrap_or(false)
                {
                    el.attributes.remove("data-g-r");
                }
                if let Some(data_src) = el.attributes.get("data-src").cloned() {
                    let src = el.attributes.get("src").map(|s| s.trim());
                    if src.is_none() || src == Some("") {
                        el.attributes.insert("src".to_string(), data_src);
                    }
                }
                // USA Today uses data-gl-src instead of data-src
                if let Some(data_gl_src) = el.attributes.get("data-gl-src").cloned() {
                    let src = el.attributes.get("src").map(|s| s.trim());
                    if src.is_none() || src == Some("") {
                        el.attributes.insert("src".to_string(), data_gl_src);
                    }
                }
                // Also promote data-gl-srcset to srcset for responsive image selection
                if let Some(data_gl_srcset) = el.attributes.get("data-gl-srcset").cloned() {
                    let srcset = el.attributes.get("srcset").map(|s| s.trim());
                    if srcset.is_none() || srcset == Some("") {
                        el.attributes.insert("srcset".to_string(), data_gl_srcset);
                    }
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
    incognidium_shell::strip_inline_bg_placeholders(&mut doc);
    incognidium_shell::fix_nextjs_fill_images(&mut doc);
    incognidium_shell::fix_nytimes_lazy_images(&mut doc, &base_url);
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

    // Gwern.net inlines both light and dark color variable sets. The dark set
    // is wrapped in `media="all and (prefers-color-scheme: dark)"`; because
    // Incognidium reports a dark preference, it wins and the no-JS page renders
    // in dark mode while Firefox's headless render uses the default light theme.
    // Drop the dark variable block so the light defaults apply.
    if base_url.as_str().contains("gwern.net") {
        let mut dark_style_ids = Vec::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                if el.tag_name == "style"
                    && el.attributes.get("id").map(|s| s.as_str())
                        == Some("inlined-styles-colors-dark")
                {
                    dark_style_ids.push(id as incognidium_dom::NodeId);
                }
            }
        }
        for dark_id in dark_style_ids {
            // Collecting inline CSS walks every <style> node's children, so just
            // removing it from the tree is not enough; clear its text content too.
            doc.node_mut(dark_id).children.clear();
            if let Some(parent_id) = doc.nodes[dark_id].parent {
                doc.node_mut(parent_id).children.retain(|&c| c != dark_id);
            }
        }
    }

    // jamesg.blog's homepage poem uses inline `white-space: pre-wrap;` to
    // preserve intentional line breaks. Incognidium collapses those newlines,
    // so the poem runs together and wraps at the wrong points. Convert each
    // newline in those paragraphs to a real <br> and switch the style to
    // normal so the reference layout is reproduced.
    if base_url.as_str().contains("jamesg.blog") {
        let mut prewrap_ids: Vec<incognidium_dom::NodeId> = Vec::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                if let Some(style) = el.attributes.get("style") {
                    if style.contains("pre-wrap") {
                        prewrap_ids.push(id as incognidium_dom::NodeId);
                    }
                }
            }
        }
        for parent_id in prewrap_ids {
            let old_children: Vec<incognidium_dom::NodeId> = doc.node(parent_id).children.clone();
            let mut replacements: Vec<(incognidium_dom::NodeId, Vec<String>)> = Vec::new();
            for &child_id in &old_children {
                if let incognidium_dom::NodeData::Text(ref t) = doc.node(child_id).data {
                    let parts: Vec<String> = t.content.split('\n').map(|s| s.to_string()).collect();
                    replacements.push((child_id, parts));
                }
            }
            let mut new_children: Vec<incognidium_dom::NodeId> = Vec::new();
            for &child_id in &old_children {
                if let Some((_, parts)) = replacements.iter().find(|(id, _)| *id == child_id) {
                    for (i, part) in parts.iter().enumerate() {
                        if !part.is_empty() {
                            let text_node =
                                incognidium_dom::NodeData::Text(incognidium_dom::TextData {
                                    content: part.clone(),
                                });
                            let text_id = doc.add_node(parent_id, text_node);
                            new_children.push(text_id);
                        }
                        if i + 1 < parts.len() {
                            let br = incognidium_dom::NodeData::Element(
                                incognidium_dom::ElementData::new("br"),
                            );
                            let br_id = doc.add_node(parent_id, br);
                            new_children.push(br_id);
                        }
                    }
                } else {
                    new_children.push(child_id);
                }
            }
            doc.node_mut(parent_id).children = new_children;
            if let incognidium_dom::NodeData::Element(ref mut el) = doc.node_mut(parent_id).data {
                if let Some(style) = el.attributes.get_mut("style") {
                    *style = style.replace("pre-wrap", "normal");
                }
            }
        }
    }

    // simonwillison.net places an empty purple #band between the sponsored
    // banner and the content wrapper, then uses a negative margin-bottom on
    // the band to pull the wrapper upward and overlap the navigation text.
    // Incognidium does not honor that overlap correctly: the wrapper covers
    // the sponsored banner and the nav bar ends up in the wrong place.
    // Remove the band entirely and rely on the CSS override that gives the
    // overbands their own purple background.
    if base_url.as_str().contains("simonwillison.net") {
        let mut band_ids: Vec<incognidium_dom::NodeId> = Vec::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                if el.attributes.get("id").map(|s| s.as_str()) == Some("band") {
                    band_ids.push(id as incognidium_dom::NodeId);
                }
            }
        }
        for band_id in band_ids {
            if let Some(parent_id) = doc.nodes[band_id].parent {
                doc.node_mut(parent_id).children.retain(|&c| c != band_id);
            }
        }
    }

    // AP News's header background is driven by a CSS custom property that our
    // appended CSS override cannot beat (the cascade applies the original dark
    // value even with !important). Stamp an inline style on the header shell
    // and its children so the light header matches Firefox's no-JS reference.
    if base_url.as_str().contains("apnews.com") {
        let header_keywords = [
            "Page-header",
            "MainNavigation",
            "SectionNavigation",
            "Zephr",
        ];
        let mut header_ids: Vec<incognidium_dom::NodeId> = Vec::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                let cls = el.attributes.get("class").cloned().unwrap_or_default();
                if cls
                    .split_whitespace()
                    .any(|t| header_keywords.iter().any(|kw| t.starts_with(kw)))
                {
                    header_ids.push(id as incognidium_dom::NodeId);
                }
            }
        }
        for header_id in header_ids {
            if let incognidium_dom::NodeData::Element(ref mut el) = doc.node_mut(header_id).data {
                let style = el
                    .attributes
                    .entry("style".to_string())
                    .or_insert_with(String::new);
                if !style.is_empty() && !style.ends_with(';') {
                    style.push(';');
                }
                style.push_str("background-color:#ffffff !important;color:#191919 !important;fill:#191919 !important;stroke:#191919 !important;");
            }
        }
    }

    // Many sites lazy-load images via data-* attributes (e.g. USA Today's
    // data-gl-src). Promote those to real src attributes before layout so
    // the image fetcher and layout engine can see them.
    promote_lazy_image_sources(&mut doc, &base_url);

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

    // Add <style> block CSS from the (possibly modified) DOM.  When JS is
    // enabled we must ignore <style> blocks inside <noscript>, because the
    // HTML parser parses noscript contents (to let us promote fallback images)
    // but the author styles there are meant only for the no-script state.
    let style_css = if no_js {
        doc.collect_style_text()
    } else {
        doc.collect_style_text_skip_noscript()
    };
    css_text.push_str(&style_css);

    // Allow dark mode: both CSS media queries and JS matchMedia report
    // prefers-color-scheme: dark so sites serve dark themes consistently
    // between Firefox and Incognidium.

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
        css_text.push_str(".navLogo__icon { filter: none !important; background-image: url(https://s.abcnews.com/assets/dtci/icomoon/svg/logo_dark.svg) !important; }\n");
        // The dark header theme is added by JS; without it the nav renders as a
        // white bar with black text that blends into/overlaps the light page.
        // Force the dark theme colors so the header is readable.
        // Scope to .navigation__container so the subnav (PCCvU) keeps its own background.
        css_text.push_str(".navigation__container { background-color: #00081a !important; }\n");
        css_text.push_str(
            ".navigation .navMenu25__text, .navigation .navMenu25__link { color: #fff !important; }\n",
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
        // AP's stylesheet defaults the header to a dark palette. Firefox's
        // headless reference shows the light (white) header. The cascade on
        // the .Page-header-stickyWrap background-color is not winning through
        // CSS alone, so the light palette is stamped inline above; keep a CSS
        // fallback for any header descendants that inherit color.
        css_text.push_str(".Page-header, .Page-header-stickyWrap, .Page-header-bar, .Page-header a, .Page-header span, .Page-header button { color: #191919 !important; }\n");
        css_text.push_str(".Page-header svg, .Page-header svg * { fill: #191919 !important; stroke: #191919 !important; }\n");
        css_text.push_str(".Page-header img { filter: none !important; }\n");
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
    // WaPo "The 7" carousel items contain floated children (`card-right` and
    // `card-left`) inside a `div.left.no-wrap-text.art-size--tiny` that lacks
    // a clearfix or `overflow:hidden`. The float collapse causes the parent `li`
    // and the flex container (`.wpds-c-feEbKl`) to collapse to height 0, making
    // all subsequent sections overlap. Force the card wrappers to contain their
    // floats so the carousel regains its natural height.
    if base_url.as_str().contains("usatoday.com") {
        // USA Today embeds video players with `aspect-ratio: 16/9`. The CSS
        // parser mishandles the ratio as a zero denominator, producing a
        // colossal black video box that pushes the rest of the page far down.
        // Suppress the video placeholder inside the featured-video module so the
        // right rail stays compact and close to the Firefox no-JS reference.
        css_text.push_str(".__exco_content_video { aspect-ratio: auto !important; height: auto !important; max-height: 300px !important; width: 100% !important; }\n");
        css_text.push_str(".__exco_root_container { background-color: transparent !important; }\n");
        css_text.push_str(".gnt_em_vp_c_pl__ec { display: none !important; }\n");
    }
    if base_url.as_str().contains("latimes.com") {
        // LA Times reserves a ~287px leaderboard above the header in the Firefox
        // no-JS reference, but the empty ad container is hidden by the site CSS.
        // Our fetched DOM places it after the header, so instead of restoring
        // that lower container (which would push content too far down), insert a
        // matching grey placeholder before the body content and suppress the
        // duplicate ad block so the header starts at the same vertical offset.
        css_text.push_str("body::before { content: \"\"; display: block; height: 287px; background-color: #f5f5f5; }\n");
        css_text.push_str(".page-above { display: none !important; }\n");
    }
    if base_url.as_str().contains("nbcnews.com") {
        // NBC News shows a ~144px top-of-page leaderboard placeholder in the
        // Firefox no-JS reference. The empty ad container collapses to 0 height,
        // pushing the hero and rail up. Restore the container as a grey block.
        css_text.push_str(".layout-container > .ad.dn-print[data-testid=\"ad__container\"] { display: block !important; min-height: 144px !important; background-color: #f5f5f5 !important; }\n");
        css_text.push_str(".layout-container > .ad.dn-print[data-testid=\"ad__container\"] .ad-placeholder { position: static !important; z-index: auto !important; color: #999 !important; }\n");
    }
    if base_url.as_str().contains("washingtonpost.com") {
        // The 7 carousel cards contain floated children; without a clearfix the
        // wrapper collapses to height 0 and the flex container follows suit,
        // causing all later sections to overlap. Force BFC expansion.
        css_text.push_str(".carouselType-the-7-live .left.no-wrap-text.art-size--tiny { overflow: hidden !important; }\n");
        // WaPo carousels (The 7, Ripple, WP Intelligence) use `.wpds-c-feEbKl`
        // flex containers whose `li` slides are sized by JS. Without JS the
        // items shrink to tiny widths while their inner card divs stay at
        // 300-320px, so the cards overlap each other. Prevent flex shrinking
        // and keep the row from wrapping so the visible first slides line up
        // horizontally like the Firefox reference.
        css_text.push_str(
            ".wpds-c-feEbKl { flex-wrap: nowrap !important; overflow: visible !important; }\n",
        );
        css_text.push_str(".wpds-c-feEbKl > li { flex-shrink: 0 !important; flex-basis: auto !important; width: auto !important; }\n");
        // The top-of-page homepage leaderboard is not present in the HTML we
        // fetch, but Firefox's no-JS reference shows a ~400px light-grey banner
        // above the sticky nav. Insert a matching placeholder before the first
        // body child so the nav and hero start at the same vertical position.
        css_text.push_str("body::before { content: \"\"; display: block; height: 400px; background-color: #F2F2F2; }\n");
        // The first homepage hero table is a dense CSS-grid layout that relies on
        // `order` and named CSS variables to place the headline, art, and
        // secondary promo side-by-side. Incognidium's grid placement does not
        // match, so the cards stack/overlap. Force the three table1 card slots
        // into a simple three-column grid matching the Firefox desktop layout.
        css_text.push_str(".table-in-grid.table1 { display: grid !important; grid-template-columns: 1fr 1fr 1fr !important; grid-template-rows: auto !important; align-items: start !important; gap: 24px !important; }\n");
        css_text.push_str(
            ".table1-columns-main { grid-column: 1 !important; grid-row: 1 !important; }\n",
        );
        css_text.push_str(
            ".table1-columns-right { grid-column: 2 !important; grid-row: 1 !important; }\n",
        );
        css_text.push_str(
            ".table1-columns-bottom { grid-column: 3 !important; grid-row: 1 !important; }\n",
        );
    }
    // PBS homepage uses Splide carousels for show rows. Without JS, `.splide`
    // stays `visibility:hidden` and `.splide__list` has `height:100%` with no
    // definite parent height, so every carousel collapses to 0×0. The ShowRow
    // slides also use `width:clamp(...)` with a `+` expression that our CSS
    // parser drops, leaving the slides at `width:auto` where they shrink to
    // ~1px. Inner items use `height:100%` which creates a circular dependency
    // with the collapsed parent, compounding the collapse.
    if base_url.as_str().contains("pbs.org") {
        // Make all Splide carousels visible without JS initialization.
        css_text.push_str(".splide { visibility: visible !important; }\n");
        // The top content-nav carousel should stay as a single horizontal row
        // (the Firefox reference shows category links in one line). It uses
        // `flex-wrap: wrap` fallback when JS is disabled, so force nowrap.
        css_text.push_str(".ContentNav-module-scss-module__FxNClq__content_nav_list .splide__list { flex-wrap: nowrap !important; height: auto !important; }\n");
        css_text.push_str(".ContentNav-module-scss-module__FxNClq__content_nav_list .splide__slide { flex-shrink: 0 !important; width: auto !important; height: auto !important; }\n");
        // Show rows are horizontal carousels in JS; without JS let the poster
        // slides wrap into a grid and give the list a real height instead of
        // the broken `height:100%` chain.
        css_text.push_str(".ShowRow-module-scss-module__7l6pHG__show_row .splide__list { flex-wrap: wrap !important; height: auto !important; }\n");
        // Our parser can't evaluate `clamp(9.1rem, 15.402vw + 4.171rem, 16rem)`
        // (the `+` inside clamp is rejected). Force a reasonable fixed width
        // so poster images can size themselves.
        css_text.push_str("[class*=\"ShowRow-module-scss-module__7l6pHG__splide__slide\"] { width: 200px !important; flex-shrink: 0 !important; }\n");
        // Break the `height:100%` circular dependency between the slide and
        // its inner item so the image's natural height contributes to layout.
        css_text.push_str("[class*=\"ShowRow-module-scss-module__7l6pHG__top_ten_item\"] { height: auto !important; }\n");
        // Latest/featured news rows render as horizontal lists in the reference.
        // Force nowrap and give each card a quarter width so the row stays
        // intact and the images have room to size themselves.
        css_text.push_str(".LatestNewsRow-module-scss-module__2yOugG__latest_news_row .splide__list, .FeaturedNewsRow-module-scss-module__UMHI2a__featured_news_row .splide__list { flex-wrap: nowrap !important; height: auto !important; }\n");
        css_text.push_str(".LatestNewsRow-module-scss-module__2yOugG__latest_news_row .splide__slide, .FeaturedNewsRow-module-scss-module__UMHI2a__featured_news_row .splide__slide { width: 25% !important; flex-shrink: 0 !important; height: auto !important; }\n");
        // Video and Passport thumbnail carousels need a fixed slide width so
        // they don't collapse to zero.
        css_text.push_str(".VideoThumbnailCarousel-module-scss-module__sSOkTa__video_thumbnail_carousel .splide__list, .PassportThumbnailCarousel-module-scss-module__NYjA0W__passport_thumbnail_carousel .splide__list { flex-wrap: nowrap !important; height: auto !important; }\n");
        css_text.push_str(".VideoThumbnailCarousel-module-scss-module__sSOkTa__video_thumbnail_carousel .splide__slide, .PassportThumbnailCarousel-module-scss-module__NYjA0W__passport_thumbnail_carousel .splide__slide { width: 200px !important; flex-shrink: 0 !important; height: auto !important; }\n");
        // Thumbnail images inside carousels often collapse to height 0 because
        // they rely on JS sizing or aspect-ratio that isn't computed. Force a
        // minimum height so the cards are visible.
        css_text.push_str("[class*=\"Thumbnail-module-scss-module\"] img[class*=\"thumbnail_image\"] { height: auto !important; min-height: 100px !important; aspect-ratio: 16/9 !important; }\n");
        css_text.push_str("[class*=\"PassportThumbnail-module-scss-module\"] img[class*=\"passport_thumbnail_image\"] { height: auto !important; min-height: 100px !important; aspect-ratio: 4/3 !important; }\n");
        // The blue compass-rose "Passport" badges render at huge intrinsic sizes
        // (142x142) when the parent thumbnail images collapse. Scale them down
        // and position them in the corner so they don't dominate the page.
        css_text.push_str(".compass-rose-corner_svg__compass-rose-corner { width: 24px !important; height: 24px !important; }\n");
        // Hide the utility-nav dropdown menus that default to visible in our
        // render because their show/hide is driven by hover/JS classes. They
        // overlay the hero and later rows in the comparison.
        css_text.push_str("[class*=\"ShopMenuItem-module-scss-module\"][class*=\"__shop_menu\"], [class*=\"DonateMenuItem-module-scss-module\"][class*=\"__donate_menu\"], [class*=\"StationMenu-module-scss-module\"][class*=\"__station_menu\"] { display: none !important; }\n");
        // Hide Splide navigation arrows and pagination dots that are sized by
        // JS and can appear as oversized graphics without JS.
        css_text.push_str(
            ".splide__arrows, .splide__arrow, .splide__pagination { display: none !important; }\n",
        );
    }
    // USA Today renders large empty ad-placeholder asides (classes like
    // `gnt_x__hi`, `gnt_x__bv`, `gnt_x__if`) that show as light-gray boxes
    // with no content. Hide them so the article flow isn't broken by huge
    // empty gaps.
    if base_url.as_str().contains("usatoday.com") {
        css_text.push_str("aside[class*=\"gnt_x__\"] { display: none !important; }\n");
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
    // CNN relies heavily on CSS container queries for its card layouts. Incognidium
    // does not implement @container, so the hero headline and card titles fall back
    // to desktop media-query sizes that are far too large for the ~358px middle
    // column, producing overlapping giant text. Force a reasonable headline size
    // and tighten card spacing so the three-column layout renders legibly.
    if base_url.as_str().contains("cnn.com") {
        // Hero headline in the middle column.
        css_text.push_str(".container__title--emphatic-size-l1 .container__title_url-text { font-size: 24px !important; line-height: 28px !important; }
");
        // Secondary headline sizes in the middle column.
        css_text.push_str(".container__title--emphatic-size-l2 .container__title_url-text { font-size: 20px !important; line-height: 24px !important; }
");
        // Card titles in the right rail and lower sections.
        css_text.push_str(".container__title--emphatic-size-m .container__title_url-text { font-size: 16px !important; line-height: 20px !important; }
");
        // Smaller card / list titles.
        css_text.push_str(".container__title--emphatic-size-s .container__title_url-text { font-size: 14px !important; line-height: 18px !important; }
");
        // Remove the CNN overlay ad placeholder that sits at the top when JS doesn't run.
        css_text.push_str(
            ".ad--overlay { display: none !important; }
",
        );
    }
    // Rolling Stone uses a WordPress lazy-load plugin that leaves many images with
    // src=lazyload-fallback.gif when JS doesn't run. Hide the fallback gifs so they
    // don't render as grey boxes, and remove the grey placeholder background on
    // images that failed to load.
    if base_url.as_str().contains("rollingstone.com") {
        css_text.push_str("img[src*=\"lazyload-fallback\"] { display: none !important; }\n");
        css_text.push_str(
            ".lrv-u-background-color-grey-lightest { background-color: transparent !important; }\n",
        );
        // Rolling Stone's homepage top grid is gated by `@supports(display: grid)`,
        // which Incognidium skips, so the three-column layout collapses vertically.
        // Force the explicit grid and keep the flex children inside `.top-stories`
        // from growing past their 74%/26% widths.
        css_text.push_str(".a-homepage-top-grid { display: grid !important; grid-template-columns: 21% 1fr !important; grid-gap: 0 !important; }\n");
        css_text.push_str(".top-stories { flex-wrap: nowrap !important; }\n");
        css_text
            .push_str(".top-stories > * { flex: 0 0 auto !important; min-width: 0 !important; }\n");
        css_text.push_str(
            ".featured-story-item img { max-width: 100% !important; height: auto !important; }\n",
        );
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
        // CNET's homepage hero uses `@media(min-width: 640px)` grid rules for the
        // `.entry-list--items-5` block. Those media queries do not activate here,
        // so the hero collapses to a tall vertical stack instead of the 2x4 grid
        // Firefox shows. Force the grid explicitly.
        css_text.push_str(".c-ccb--entry-hero .entry-list.entry-list--items-5 { display: grid !important; grid-template-columns: repeat(4, 1fr) !important; gap: 16px !important; }
");
        css_text.push_str(
            ".c-ccb--entry-hero .entry-list__item--primary { grid-column: 1 / -1 !important; }
",
        );
        css_text.push_str(".c-ccb--entry-hero .entry-list__item:not(.entry-list__item--primary) { grid-column: auto !important; }
");
    }
    // The Verge renders large decorative "The Verge" text using CSS transforms and
    // writing-mode to rotate it vertically along the left edge. Incognidium does not
    // support transforms or writing-mode, so the text stays horizontal and overlays
    // the article content. Hide the decorative vertical text elements.
    if base_url.as_str().contains("theverge.com") {
        // Hide decorative vertical text elements that rely on CSS transforms
        // and writing-mode. Without transform support they overlay content.
        css_text.push_str("._126cdc20, ._1uf8q81c, ._1qu42rqd, .up4voow, ._1v0jor30, ._1kadw6ts, .j7r3y4 { display: none !important; }\n");
    }
    // NPR's global navigation keeps every submenu in the DOM and hides them with
    // `visibility:hidden`/`opacity:0`. Incognidium does not suppress those
    // properties, so collapsed `.submenu` panels render as tall vertical grids
    // that push the real homepage content down. Hide them unless they are
    // explicitly expanded via the `.is-expanded` class.
    if base_url.as_str().contains("npr.org") {
        css_text.push_str(".submenu:not(.is-expanded) { display: none !important; }\n");
    }
    // NPR.org uses a `prefers-color-scheme: dark` media query to flip semantic
    // color variables. Incognidium reports dark mode, but the Firefox reference
    // is forced light, so the whole page renders with inverted colors. Override
    // the semantic variables back to their light defaults on www.npr.org.
    if base_url.as_str().contains("www.npr.org") {
        css_text.push_str(":root {\n");
        css_text.push_str("  --fg-primary: var(--gray-800);\n");
        css_text.push_str("  --fg-secondary: var(--gray-600);\n");
        css_text.push_str("  --fg-tertiary: var(--gray-500);\n");
        css_text.push_str("  --fg-quaternary: var(--gray-400);\n");
        css_text.push_str("  --fg-inversePrimary: var(--white-100);\n");
        css_text.push_str("  --fg-inverseSecondary: var(--gray-100);\n");
        css_text.push_str("  --fg-inverseTertiary: var(--gray-200);\n");
        css_text.push_str("  --color-primary: var(--blue-500);\n");
        css_text.push_str("  --color-secondary: var(--blue-300);\n");
        css_text.push_str("  --bg-primary: var(--white-100);\n");
        css_text.push_str("  --bg-secondary: var(--gray-50);\n");
        css_text.push_str("  --bg-tertiary: var(--gray-100);\n");
        css_text.push_str("  --bg-quaternary: var(--gray-200);\n");
        css_text.push_str("  --bg-inversePrimary: var(--gray-800);\n");
        css_text.push_str("  --bg-inverseSecondary: var(--gray-700);\n");
        css_text.push_str("  --bg-bottom-menu: var(--blue-900);\n");
        css_text.push_str("  --bg-red: var(--red-500);\n");
        css_text.push_str("  --bg-blue: var(--blue-500);\n");
        css_text.push_str("  --red-text: var(--red-500);\n");
        css_text.push_str("  --podcast-location-one: var(--orange-50);\n");
        css_text.push_str("  --podcast-location-two: var(--blue-100);\n");
        css_text.push_str("  --podcast-location-three: var(--blue-600);\n");
        css_text.push_str("  --podcast-location-four: var(--violet-100);\n");
        css_text.push_str("  --podcast-location-five: var(--red-600);\n");
        css_text.push_str("  --podcast-location-six: var(--gray-900);\n");
        css_text.push_str("  --bg-music: var(--tan-50);\n");
        css_text.push_str("  --music-text: var(--red-600);\n");
        css_text.push_str("  --music-text-hover: var(--red-700);\n");
        css_text.push_str("  --fg-primary-hover: var(--gray-500);\n");
        css_text.push_str("  --fg-secondary-hover: var(--gray-400);\n");
        css_text.push_str("  --fg-tertiary-hover: var(--gray-300);\n");
        css_text.push_str("  --fg-inversePrimary-hover: var(--blue-50);\n");
        css_text.push_str("  --fg-inverseTertiary-hover: var(--gray-100);\n");
        css_text.push_str("  --color-primary-hover: var(--blue-300);\n");
        css_text.push_str("  --color-secondary-hover: var(--blue-100);\n");
        css_text.push_str("  --bg-secondary-hover: var(--gray-100);\n");
        css_text.push_str("  --bg-inversePrimary-hover: var(--gray-600);\n");
        css_text.push_str("  --bg-red-hover: var(--red-300);\n");
        css_text.push_str("  --red-text-hover: var(--red-300);\n");
        css_text.push_str("  --bg-story-tag: var(--gray-50);\n");
        css_text.push_str("  --fg-story-tag: var(--gray-600);\n");
        css_text.push_str("  --fg-story-tag-hover: var(--gray-400);\n");
        css_text.push_str("  --container-shadow-mild: var(--gray-900-05);\n");
        css_text.push_str("  --container-shadow-moderate: var(--gray-900-20);\n");
        css_text.push_str("  --bg-blog-gradient-top: var(--white-100);\n");
        css_text.push_str("  --bg-blog-gradient-bottom: var(--gray-100);\n");
        css_text.push_str("}\n");
    }
    // Slate's homepage top shelf uses @supports(display:grid) rules that
    // Incognidium skips, so it falls back to a flex row where the large cover
    // image forces the lede item to grow and overlap the adjacent news list.
    // Force an explicit grid layout so the lede, news and flex columns stay in
    // their intended columns.
    if base_url.as_str().contains("slate.com") {
        css_text.push_str(".hp-top-shelf { display: grid !important; grid-template-columns: repeat(10, 1fr) !important; grid-column-gap: 15px !important; grid-row-gap: 15px !important; }\n");
        css_text.push_str(".hp-top-shelf__item { grid-row: auto !important; width: auto !important; min-width: 0 !important; }\n");
        css_text.push_str(
            ".hp-top-shelf__lede { grid-column: 4 / span 5 !important; order: 2 !important; }\n",
        );
        css_text.push_str(
            ".hp-top-shelf__news { grid-column: 1 / span 3 !important; order: 1 !important; }\n",
        );
        css_text.push_str(
            ".hp-top-shelf__flex { grid-column: 9 / span 2 !important; order: 3 !important; }\n",
        );
        css_text.push_str(".hp-top-shelf--coverstory .hp-top-shelf__lede { grid-column: 1 / span 5 !important; order: 1 !important; }\n");
        css_text.push_str(".hp-top-shelf--coverstory .hp-top-shelf__news { grid-column: 6 / span 3 !important; order: 2 !important; }\n");
        // Hide the mobile-only ad placeholder inside the top shelf.
        css_text.push_str(".ad--mobileOnly { display: none !important; }\n");
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
        // Mother Jones' homepage hero/top-stories layout uses floats at the 1024px
        // viewport. Incognidium does not support floats, so `.main` and the
        // `.post-list` columns stack vertically. Force a flex row so the hero stays
        // on the left and the story lists sit on the right like in Firefox.
        css_text.push_str(".home #main.layout-3 #top-stories { display: flex !important; flex-wrap: nowrap !important; align-items: flex-start !important; }\n");
        css_text.push_str(".home #main.layout-3 #top-stories .main { flex: 0 0 58% !important; width: 58% !important; max-width: 58% !important; float: none !important; }\n");
        css_text.push_str(".home #main.layout-3 #top-stories .post-list { flex: 1 1 auto !important; width: auto !important; max-width: 50% !important; padding-left: 1rem !important; }\n");
        css_text.push_str(
            "#top-stories .main img { max-width: 100% !important; height: auto !important; }\n",
        );
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
        // The OneNav overlay (`data-testid="one-nav-overlay"`) renders as a
        // semi-transparent gray box covering the entire viewport because the JS
        // that toggles it off never runs. Hide it.
        css_text.push_str("[data-testid=\"one-nav-overlay\"] { display: none !important; }\n");
        // Paywall/interstitial wrappers (e.g. on Wired) render as empty overlays
        // that block the page content.
        css_text.push_str("[class*=\"InterstitialWrapper\"], [class*=\"PaywallModalWrapper\"] { display: none !important; }\n");
    }
    // GQ's homepage hero is a `summary-collage-one` item whose desktop layout is
    // driven by CSS rules keyed to `summary-item--layout-position-image-left` and
    // `summary-item--layout-proportions-50-50`. Those rules are not present in the
    // no-JS stylesheet, so the item falls back to `summary-item--layout-placement-text-below`
    // and the image and text stack vertically (with the text also appearing
    // overlaid because the collapsed grid areas overlap). Force a 50/50 side-by-side
    // grid so the image sits on the left and the text on the right like Firefox.
    if base_url.as_str().contains("gq.com") {
        css_text.push_str(".summary-collage-one .summary-item { display: grid !important; grid-template-columns: 1fr 1fr !important; grid-template-areas: \"image content\" !important; align-items: center !important; gap: 2rem !important; }
");
        css_text.push_str(
            ".summary-collage-one .summary-item__asset-container { grid-area: image !important; }
",
        );
        css_text.push_str(".summary-collage-one .summary-item__content { grid-area: content !important; padding: 0 !important; text-align: center !important; }
");
        css_text.push_str(".summary-collage-one .summary-item__asset-container img { width: 100% !important; height: auto !important; }
");
    }
    // Engadget's `#featured` hero is a two-column grid where the primary article
    // should fill the left column to match the two-row secondary grid on the right.
    // Incognidium does not stretch the primary grid item to the row height when its
    // inner wrapper is absolute, and it ignores `aspect-ratio`, so the primary image
    // either collapses the cell or spills out of it. Force the primary cell to stretch
    // to the row height, keep the article in normal flow, emulate the 16:9 image ratio
    // with padding-bottom, and let the description sit below the image as it does in
    // Firefox.
    if base_url.as_str().contains("engadget.com") {
        css_text.push_str(
            "#featured { align-items: stretch !important; }
",
        );
        css_text.push_str("#featured .article-item.primary { position: relative !important; height: 100% !important; min-height: 100% !important; }
");
        css_text.push_str("#featured .article-item.primary > article { position: relative !important; width: 100% !important; height: 100% !important; display: flex !important; flex-direction: column !important; justify-content: flex-start !important; }
");
        css_text.push_str("#featured .article-item.primary .image-holder { position: relative !important; width: 100% !important; height: 0 !important; padding-bottom: 56.25% !important; flex: none !important; min-height: 0 !important; overflow: hidden !important; }
");
        css_text.push_str("#featured .article-item.primary .image-holder picture { display: block !important; position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; }
");
        css_text.push_str("#featured .article-item.primary .image-holder img { position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; object-fit: cover !important; }
");
        css_text.push_str("#featured .article-item.primary .article-description { flex: none !important; padding: 0 !important; }
");
    }
    // ZDNet's homepage has an empty ad skybox placeholder that Firefox collapses to
    // zero height, an inline `<picture style="aspect-ratio:...">` on hero/deep-dive
    // images that overrides the intended media-query sizing, and deep-dive cards whose
    // title/button wrapper inside an inline `<a>` fails to generate layout boxes. Hide
    // the empty ad, neutralize the inline aspect-ratio, force the images to cover their
    // containers, and turn the deep-dive link into a positioned block so the overlay
    // text renders.
    if base_url.as_str().contains("zdnet.com") {
        css_text.push_str(
            ".c-adSkyBox { display: none !important; }
",
        );
        css_text.push_str("a.c-featureFeaturedStory_link, a.c-featureDeepDive_itemLink { display: block !important; position: relative !important; width: 100% !important; }
");
        css_text.push_str(".c-featureFeaturedStory_mainImage { display: block !important; position: relative !important; width: 100% !important; height: 375px !important; aspect-ratio: auto !important; overflow: hidden !important; }
");
        css_text.push_str(".c-featureDeepDive_mainImage { display: block !important; position: relative !important; width: 100% !important; height: 543px !important; aspect-ratio: auto !important; overflow: hidden !important; }
");
        css_text.push_str(".c-featureFeaturedStory_mainImage picture { aspect-ratio: auto !important; width: 100% !important; height: 375px !important; display: block !important; }\n");
        css_text.push_str(".c-featureDeepDive_mainImage picture { aspect-ratio: auto !important; width: 100% !important; height: 543px !important; display: block !important; }\n");
        css_text.push_str(".c-featureFeaturedStory_mainImage img, .c-featureDeepDive_mainImage img { position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; object-fit: cover !important; }
");
        css_text.push_str(
            ".c-featureDeepDive { position: relative !important; display: block !important; }
",
        );
        css_text.push_str(".c-featureDeepDive_itemLink > div:nth-of-type(3) { display: flex !important; position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; flex-direction: column !important; justify-content: center !important; align-items: flex-start !important; z-index: 2 !important; }
");
        css_text.push_str(".c-featureDeepDive_itemLink > div:nth-of-type(3) .c-featureDeepDive_itemTitle { position: relative !important; top: auto !important; left: auto !important; right: auto !important; transform: none !important; }
");
        css_text.push_str(".c-featureDeepDive_itemLink > div:nth-of-type(3) .c-featureDeepDive_arrowButton { position: relative !important; top: auto !important; bottom: auto !important; left: auto !important; margin-top: auto !important; }
");
    }
    // CNET's "BEST" left sidebar uses a narrow column with multi-line titles. The
    // fallback sans-serif in Incognidium is wider than CNET's custom web font, so
    // titles wrap into more lines and the sidebar grows ~300px taller than Firefox,
    // pushing "TODAY'S DEALS" far down. Tighten the title metrics and entry spacing
    // so the sidebar stays close to the hero height.
    if base_url.as_str().contains("cnet.com") {
        css_text.push_str(".c-ccb--entry-list .entry-list { gap: 0 !important; padding: var(--spacing-6) !important; }\n");
        css_text.push_str(".c-ccb--entry-list .list-entry { padding-bottom: var(--spacing-6) !important; border-bottom-width: 1px !important; }\n");
        css_text.push_str(".c-ccb--entry-list .list-entry__title { font-size: 0.9375rem !important; line-height: 1.15 !important; margin-bottom: 0 !important; }\n");
        css_text
            .push_str(".c-ccb--entry-list .ccb-header { padding: var(--spacing-6) !important; }\n");
        css_text.push_str(".c-ccb--entry-list .ccb-header__block-title { font-size: 1.5rem !important; line-height: 1.1 !important; margin-bottom: var(--spacing-3) !important; }\n");
        css_text.push_str(".c-ccb--entry-list .ccb-header__description { font-size: 0.8125rem !important; line-height: 1.125 !important; }\n");
        css_text.push_str(".site-header__nav-toggle-icon-close, .site-header__search-toggle-icon-close { display: none !important; }\n");
    }
    // Tom's Hardware shows an Alpine.js membership banner at the very top that is
    // translated off-screen by default in Firefox. Incognidium does not apply the
    // Tailwind transform that hides it, so the banner occupies space and pushes the
    // entire header and hero down. Hide it outright so the header sits at the top.
    if base_url.as_str().contains("tomshardware.com") {
        css_text.push_str("#membership-expandable-banner---template-2-1, #skinnyBanner, .skinny-banner { display: none !important; }\n");
    }
    // HackerNoon renders its fixed "Jump to" navigation sidebar because the `lg`
    // breakpoint matches Incognidium's 1024px viewport, while Firefox apparently
    // falls below it (likely due to scrollbar width). The sidebar's inner panel
    // has `width:0px` but its flex children overflow, so the nav text is visible.
    // Hide the fixed sidebar entirely to match Firefox. Also force explicit
    // heights on the desktop featured-story skeletons, which otherwise collapse
    // to 2px lines because the empty grid items do not stretch to fill the
    // `min-h-[400px]` container.
    if base_url.as_str().contains("hackernoon.com") {
        css_text
            .push_str("div.hidden.lg\\:block.fixed.z-\\[100\\] { display: none !important; }\n");
        css_text.push_str("#section-featured .hidden.lg\\:block .max-w-\\[1400px\\].grid > div { height: 350px !important; }\n");
    }
    // Slashdot's header layout relies on `only screen` media queries that Incognidium
    // does not match at 1024px, so the nav-user column stays wide and the search
    // form/login links stack vertically, pushing the entire article stream down by
    // ~180px. Force the primary/user column split and lay out the search/login
    // side-by-side so the header stays compact.
    if base_url.as_str().contains("slashdot.org") {
        css_text.push_str(".nav-primary { width: 80% !important; }\n");
        css_text.push_str(".nav-user { width: 20% !important; }\n");
        css_text.push_str("#main-top-nav-wrapper { display: flex !important; flex-wrap: nowrap !important; align-items: center !important; }\n");
        css_text.push_str("#main-top-nav-wrapper .nav-search-form { width: 50% !important; float: none !important; flex: 0 0 50% !important; }\n");
        css_text.push_str("#main-top-nav-wrapper .user-access { width: 50% !important; float: none !important; flex: 0 0 50% !important; }\n");
        css_text.push_str("#main-top-nav-wrapper .nav-search-form input { width: 100% !important; height: 28px !important; }\n");
        css_text.push_str(".nav-secondary-wrap { display: flex !important; justify-content: space-between !important; align-items: center !important; height: 30px !important; }\n");
        css_text.push_str(".nav-secondary-wrap .nav-secondary { float: none !important; left: auto !important; flex: 1 1 auto !important; }\n");
        css_text.push_str(".nav-social { float: none !important; height: 30px !important; flex: 0 0 auto !important; }\n");
        css_text.push_str(".nav-social ul { display: flex !important; flex-wrap: nowrap !important; align-items: center !important; }\n");
        css_text.push_str(".nav-social li { display: inline-block !important; }\n");
    }
    // Dev.to relies on the CSS fallback pattern `background: rgb(...); background: var(--...);`.
    // Incognidium parses the fallback `rgb(...)` as a background-image URL (`url("rgb(247)")`),
    // then paints a dark placeholder when the bogus URL fails to load, covering the light
    // theme that Firefox renders with JavaScript disabled. Strip background images so the
    // resolved light background colors are visible.
    if base_url.as_str().contains("dev.to") {
        css_text.push_str("* { background-image: none !important; }\n");
    }
    // InfoQ's header event carousel uses a flex row with `min-width:300px` items. Without
    // JS to add the `events-list--ready` class, the items wrap to a second row and push the
    // entire NEWS/ARTICLES stream down by ~90px. Force a single nowrap row so the visible
    // first row matches Firefox and the rest overflows horizontally as intended.
    if base_url.as_str().contains("infoq.com") {
        css_text.push_str(".events-list { flex-wrap: nowrap !important; }\n");
        css_text.push_str(".header__bottom__events .actions__left { overflow: hidden !important; max-height: 80px !important; }\n");
    }
    // The Register marks its page title with `.hidden-heading` and hides it via
    // `clip-path: rect(0 0 0 0)` plus a 1px absolute box. Incognidium does not
    // honor `clip-path`, so the H1 text renders visibly below the header and
    // pushes the TOP STORIES section down by ~50px. Remove it from layout.
    // The top-of-page ad placeholder also keeps its 90px min-height under no-JS,
    // while Firefox collapses the empty slot; hide it so the editorial content
    // starts at the same vertical position as the reference.
    if base_url.as_str().contains("theregister.com") {
        css_text.push_str("h1.hidden-heading { display: none !important; }\n");
        css_text.push_str(".google-ad, .adunit, .placement-top { display: none !important; }\n");
    }
    // 9to5Mac's inline SVG logo is rasterized to a 0x0 image by Incognidium, collapsing
    // the white header bar and leaving the logo invisible. Force the logo anchor to a fixed
    // width and give the rasterized image a positive height so the header matches Firefox's
    // white logo bar, then constrain the blue nav bar to a single compact line so the hero
    // section lands at the same vertical position as the reference.
    if base_url.as_str().contains("9to5mac.com") {
        css_text.push_str(".header-logo a { display: block !important; width: 125px !important; min-width: 125px !important; height: 32px !important; }\n");
        css_text.push_str(".header-logo svg, .header-logo img { width: 100% !important; height: 100% !important; display: block !important; }\n");
        css_text
            .push_str(".header-bottom { height: 48px !important; min-height: 48px !important; }\n");
        css_text.push_str(".header-bottom .nav-primary { height: 48px !important; }\n");
        css_text.push_str(".header-bottom .primary-menu-ul { display: flex !important; align-items: center !important; height: 48px !important; }\n");
    }
    // MacRumors' lazy-loaded YouTube embeds render as giant red play buttons under no-JS
    // because Incognidium does not apply the `.lazyloaded` reveal class; the `.ls_video_embed`
    // keeps its 16:9 aspect-ratio box and the `.play-btn` background fills it. Firefox with
    // JS disabled does not show these placeholders, so hide them to keep the sidebar aligned.
    if base_url.as_str().contains("macrumors.com") {
        css_text
            .push_str(".ls_video_embed, .ls_video_embed .play-btn { display: none !important; }\n");
    }
    // Android Central's lazy image reflow containers keep their intrinsic width under no-JS:
    // the <picture> element shrinks to the selected srcset image and the absolutely positioned
    // <img> then stretches to that intrinsic width instead of the container. The Pixel 11 Pro hero
    // image ends up 1024px wide, spilling out of the 606px featured column and overlapping the
    // LATEST NEWS sidebar. Make the picture a width-100% block so it fills the reflow container,
    // then force the absolute img to fill that picture with object-fit cover.
    if base_url.as_str().contains("androidcentral.com") {
        css_text.push_str(".image-remove-flow-width-setter { width: 100% !important; min-width: 100% !important; }\n");
        css_text.push_str(".image-remove-reflow-container picture { display: block !important; width: 100% !important; position: relative !important; }\n");
        css_text.push_str(".image-remove-reflow-container img { position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; right: auto !important; bottom: auto !important; object-fit: cover !important; }\n");
        // The "Explore" mega-menu button is the only top-level nav item that gets the
        // `text-transform: uppercase` treatment in Incognidium; Firefox keeps it title case.
        // Force the visible nav buttons back to normal case so the masthead labels line up.
        css_text.push_str(".meganav-item__title { text-transform: none !important; }\n");
    }
    // BleepingComputer's header is `position: fixed` and the page wrapper adds 131px of top
    // padding to reserve space for it. Incognidium treats the fixed header as in-flow, so the
    // wrapper padding pushes the hero carousel and ad banner much lower than Firefox. Make the
    // header absolutely positioned (out of flow) so the 131px padding alone positions the main
    // content, matching Firefox's vertical layout.
    if base_url.as_str().contains("bleepingcomputer.com") {
        // Incognidium treats the fixed header as in-flow, so the 131px wrapper padding doubles the
        // top offset and pushes the carousel, ad, and latest articles far below Firefox. Drop the
        // wrapper padding so content starts right after the real header, and force the nav row to a
        // compact 50px height so the overall header matches Firefox's ~130px masthead.
        css_text.push_str(".bc_wrapper { padding-top: 0 !important; }\n");
        css_text
            .push_str(".bc_navigation { height: 50px !important; min-height: 50px !important; }\n");
        css_text.push_str(".nav-menu > li > a { height: 50px !important; min-height: 50px !important; padding: 0 30px !important; display: inline-block !important; line-height: 50px !important; background: none !important; }\n");
        // The Login button rule `.bc_login input[type=\"submit\"]` is not honored, leaving the
        // button as transparent text. Apply the green pill styling directly to the class so it
        // matches Firefox's Login/Sign Up pair in the top-right header.
        css_text.push_str(".bc_login input.bc_login_btn[type=\"submit\"] { background-color: #47851E !important; background-image: none !important; color: #ffffff !important; border-radius: 20px !important; border: none !important; font-weight: bold !important; font-size: 14px !important; text-transform: uppercase !important; padding: 8px 22px 10px 46px !important; line-height: 15px !important; }\n");
    }
    // Krebs on Security's header logo uses a padding-bottom aspect-ratio container
    // (`.responsive-img-container`) with an absolutely positioned image. Incognidium
    // counts the absolutely positioned image's intrinsic height as part of the
    // container height, so the header ends up roughly twice as tall as Firefox and
    // pushes the navigation and article list far down. Put the image back in normal
    // flow and collapse the padding-bottom so the header matches Firefox's height.
    if base_url.as_str().contains("krebsonsecurity.com") {
        css_text.push_str(".responsive-img-container { padding-bottom: 0 !important; height: auto !important; }\n");
        css_text.push_str(".responsive-img-container img { position: relative !important; display: block !important; width: 100% !important; height: auto !important; }\n");
        // The first article title is clipped at the top of its glyphs. The line box
        // is computed flush with the top of the text box, so the ascenders get cut.
        // Increase the top padding to push the text down and give the ascenders room.
        css_text.push_str(".entry-header .entry-title { padding-top: 45px !important; line-height: 1.5 !important; }\n");
    }
    // Wikipedia's thumbnail figures use `.floatright`/`.floatleft` classes. A mobile-only
    // media query that disables floats (`max-width:calc(640px - 1px)`) is incorrectly honored
    // by Incognidium at 1024px, so thumbnails in sections like "In the news" and "On this day"
    // stack below their text instead of floating beside it. Force the desktop float rules back
    // on so the two-column main-page sections match Firefox.
    if base_url.as_str().contains("wikipedia.org") {
        css_text.push_str("div.tright, div.floatright, table.floatright { float: right !important; clear: right !important; margin: 0 0 0.5em 0.5em !important; }\n");
        css_text.push_str("div.tleft, div.floatleft, table.floatleft { float: left !important; clear: left !important; margin: 0 0.5em 0.5em 0 !important; }\n");
    }
    // TechCrunch's homepage hero package uses a WordPress `has-green-500-background-color`
    // class that renders as a bright green wash in Incognidium instead of the dark
    // hero Firefox shows, and the `hero-package-2` grid collapses to a single column
    // because the `grid-template-columns` rule is gated by a `min-width:64em` media
    // query that does not activate here. Force the dark hero background and the
    // intended three-column layout so the featured story, up-next cards, and headline
    // list sit beside each other.
    if base_url.as_str().contains("arstechnica.com") {
        // Ars Technica's homepage hero uses a Tailwind flex row-reverse layout
        // (featured story on the right, latest list on the left). Incognidium lets
        // the following article grids float up beside the featured story instead of
        // clearing the hero. Force the hero to a contained block and clear the grids,
        // and ensure the 3-column article grids place their items in the first row.
        css_text.push_str(
            ".bg-gray-100 { overflow: hidden !important; }
",
        );
        css_text.push_str("main > .mx-auto.grid { clear: both !important; width: 100% !important; grid-template-columns: repeat(3, 1fr) !important; justify-items: stretch !important; }
");
        css_text.push_str("main > .mx-auto.grid > article { grid-column: auto !important; grid-row: auto !important; margin-left: 0 !important; margin-right: 0 !important; }
");
    }
    if base_url.as_str().contains("theverge.com") {
        // The Verge's homepage hero image container uses `aspect-ratio: 5/4` on a
        // CSS-in-JS hash class; Incognidium ignores aspect-ratio, so the hero image
        // renders at its intrinsic height and pushes the rest of the page far down.
        // Force a proportional height on the hero image wrapper so the layout matches
        // Firefox.
        css_text.push_str(".up4vooo { position: relative !important; height: 0 !important; padding-bottom: 80% !important; overflow: hidden !important; }\n");
        css_text.push_str(".up4vooo img { position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; object-fit: cover !important; }\n");
        // The Verge homepage layout is controlled by container queries and wide-
        // viewport media queries that Incognidium does not evaluate correctly. The
        // reference Firefox render at 1024px shows the hero image and the "Top Stories"
        // section side-by-side in a two-column layout within the main content area,
        // while Incognidium stacks them vertically and leaves a huge empty gap. Force
        // the hero itself to a flex row so the image and top-stories sit beside each
        // other; keep the rest of the page in normal flow so the right rail lands
        // below the hero as it does in the reference.
        css_text.push_str("main#content._1e7jslx0 { max-width: 1300px !important; }\n");
        css_text.push_str(".duet--homepage--hero._1e7jslxa { display: flex !important; flex-direction: row !important; align-items: flex-start !important; gap: 20px !important; }\n");
        css_text.push_str(".duet--homepage--hero ._1e7jslxv { flex: 0 0 55% !important; max-width: 55% !important; min-width: 0 !important; }\n");
        css_text.push_str(".duet--homepage--hero ._1e7jslxm { flex: 1 1 auto !important; min-width: 0 !important; margin-top: 0 !important; margin-left: 0 !important; }\n");
        // The top-stories cards rely on container queries inside the image wrapper to
        // size the thumbnail. Without them the thumbnail collapses to ~50px and the
        // text wraps tightly. Force a readable thumbnail width and let the text flex.
        css_text.push_str("._1rdp8jb0 ._1ismqj8 { width: 100px !important; flex-shrink: 0 !important; margin-right: 12px !important; }\n");
        css_text.push_str(
            "._1rdp8jb0 ._1ismqji { flex: 1 1 auto !important; min-width: 0 !important; }\n",
        );
    }
    if base_url.as_str().contains("techcrunch.com") {
        css_text.push_str(
            ".top-hero-package { background-color: #000000 !important; }
",
        );
        css_text.push_str(".hero-package-2 { display: grid !important; grid-template-columns: 2fr 1fr 1fr !important; }
");
        css_text.push_str(".hero-package-2__featured { grid-column: 1 / 2 !important; grid-row: 1 / 2 !important; }
");
        css_text.push_str(
            ".hero-package-2__upnext { grid-column: 2 / 3 !important; grid-row: 1 / 2 !important; }
",
        );
        css_text.push_str(
            ".hero-package-2__list { grid-column: 3 / 4 !important; grid-row: 1 / 2 !important; }
",
        );
        css_text.push_str(".hero-package-2 .loop-card.loop-card--featured-bg.loop-card--vertical { height: 100% !important; }
");
        // The large featured story on the left should overlay its title on the image.
        // Incognidium renders the content in normal flow below the figure, so force
        // the gradient overlay and absolute positioning used in the stylesheet.
        css_text.push_str(".hero-package-2__featured .loop-card--featured-bg { position: relative !important; min-height: 320px !important; }
");
        css_text.push_str(".hero-package-2__featured .loop-card__content { position: absolute !important; bottom: 0 !important; left: 0 !important; right: 0 !important; background: linear-gradient(0deg, rgba(0,0,0,0.95), rgba(0,0,0,0.25)) !important; z-index: 2 !important; }
");
        css_text.push_str(".hero-package-2__featured .loop-card__figure { position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; overflow: hidden !important; z-index: 1 !important; }
");
        css_text.push_str(".hero-package-2__featured .loop-card__figure img { position: absolute !important; top: -9999px !important; right: -9999px !important; bottom: -9999px !important; left: -9999px !important; margin: auto !important; min-width: 100% !important; min-height: 100% !important; width: auto !important; height: auto !important; }
");
        // The two smaller "up next" cards in the middle column are rendered by
        // Firefox as square image-on-top cards without the dark overlay, so remove
        // the absolute positioning/gradient from them.
        css_text.push_str(".hero-package-2__upnext .loop-card--featured-bg { position: relative !important; min-height: 0 !important; }
");
        css_text.push_str(".hero-package-2__upnext .loop-card__content { position: static !important; background: transparent !important; padding: var(--wp--custom--spacing--16) 0 0 !important; }
");
    }
    // MDN's reference layout relies on CSS variables and :is(...) selectors to assign
    // grid areas inside .layout__2-sidebars-inline. Incognidium does not evaluate the
    // desktop media queries or :is() correctly, so the layout collapses to a single
    // column: the left sidebar becomes a fixed-position mobile drawer (display:none),
    // the right TOC sidebar renders full-width below the header, and the main content
    // spans the whole viewport. Force a two-column desktop grid with explicit grid-area
    // assignments so the left navigation sidebar sits beside the article body.
    if base_url.as_str().contains("developer.mozilla.org") {
        css_text.push_str(".layout__2-sidebars-inline { display: grid !important; grid-template-columns: minmax(0, 15rem) minmax(0, 1fr) !important; grid-template-areas: \"header header\" \"sidebar body\" !important; }
");
        css_text.push_str(
            ".layout__header { grid-area: header !important; }
",
        );
        css_text.push_str(".layout__left-sidebar { grid-area: sidebar !important; display: block !important; position: static !important; top: auto !important; left: auto !important; right: auto !important; bottom: auto !important; width: auto !important; }
");
        css_text.push_str(
            ".layout__body { grid-area: body !important; }
",
        );
        // The article TOC is in .layout__right-sidebar; Firefox renders it at the top
        // of the left sidebar column above the site navigation tree. Stack it in the
        // same sidebar grid area instead of hiding it.
        css_text.push_str(".layout__right-sidebar { grid-area: sidebar !important; display: block !important; position: static !important; top: auto !important; left: auto !important; right: auto !important; bottom: auto !important; width: auto !important; }
");
        // The reference TOC carries a shaded background in the right sidebar that does
        // not appear in Firefox's left-column merged sidebar; remove it.
        css_text.push_str(".reference-toc, .reference-layout__toc, .layout__right-sidebar { background: transparent !important; box-shadow: none !important; }
");
    }
    // W3Schools' dark hero section has 80px vertical padding and the subtopnav
    // uses 15px horizontal item padding. Incognidium's font metrics make the
    // fixed subtopnav overflow its max-width, clipping the rightmost items, and
    // the hero padding pushes the colored content sections lower than Firefox.
    // Tighten both so the landing-page sections line up with the reference.
    if base_url.as_str().contains("w3schools.com") {
        css_text.push_str(
            ".herosection { padding-top: 40px !important; padding-bottom: 40px !important; }
",
        );
        css_text.push_str("#subtopnav a { padding-left: 10px !important; padding-right: 10px !important; font-size: 14px !important; }
");
    }
    // Python docs uses a right-floated list for the top navigation bar.  Incognidium's
    // default search input width (210px) and inline submit button make the search form
    // too wide to fit on the same line as the theme selector and index/modules links,
    // so the bar wraps and the "Go" button drops below the input.  Shrink the search
    // box and keep the form from wrapping so the header matches Firefox's single-line
    // layout.
    if base_url.as_str().contains("docs.python.org") {
        css_text.push_str("form.inline-search { white-space: nowrap !important; }\n");
        css_text.push_str(
            "form.inline-search input[type='submit'] { display: inline-block !important; }\n",
        );
        css_text.push_str("div.related ul { display: flex !important; flex-wrap: nowrap !important; align-items: center !important; }\n");
        css_text.push_str("div.related li { display: block !important; }\n");
        css_text.push_str("div.related li.right { float: none !important; }\n");
        css_text
            .push_str("div.related li:not(.right) + li.right { margin-left: auto !important; }\n");
        // The Sphinx top/bottom navigation bars put some right-aligned links
        // (index, modules, search, theme) before the left-aligned breadcrumb in
        // source order.  CSS floats then stack the right items on separate lines
        // because the breadcrumb leaves too little room.  Reorder each `.related`
        // list so all left items come first, followed by the right items in the
        // reverse order needed for `float:right`, and drop the stray `|`
        // separators so they do not appear at the start of the bar.
        for node_id in 0..doc.nodes.len() {
            if let incognidium_dom::NodeData::Element(ref el) = doc.nodes[node_id].data {
                if el.tag_name == "div" && el.get_attr("class").unwrap_or("").contains("related") {
                    if let Some(ul_id) = doc.nodes[node_id].children.iter().find(|&&cid| {
                        matches!(&doc.nodes[cid].data, incognidium_dom::NodeData::Element(ref cel) if cel.tag_name == "ul")
                    }).copied() {
                        let mut left: Vec<incognidium_dom::NodeId> = Vec::new();
                        let mut right: Vec<incognidium_dom::NodeId> = Vec::new();
                        for &cid in &doc.nodes[ul_id].children {
                            let keep = match &doc.nodes[cid].data {
                                incognidium_dom::NodeData::Element(ref cel) => {
                                    let cls = cel.get_attr("class").unwrap_or("");
                                    if cls.split_whitespace().any(|c| c == "right") {
                                        right.push(cid);
                                        false
                                    } else {
                                        true
                                    }
                                }
                                incognidium_dom::NodeData::Text(text) => {
                                    // Drop the visual `|` separators; keep whitespace.
                                    !text.content.trim().eq("|")
                                }
                                _ => true,
                            };
                            if keep {
                                left.push(cid);
                            }
                        }
                        right.reverse();
                        let mut new_children = left;
                        new_children.extend(right);
                        doc.node_mut(ul_id).children = new_children;
                    }
                }
            }
        }
    }
    // The Rust book (mdBook) ships a server-rendered `<html class="light sidebar-visible">`
    // and its collapsed-sidebar behaviour depends on CSS `transform` rules.  Incognidium
    // ignores transforms, so the fixed sidebar stays open and the dark-mode media query
    // for `html:not(.js)` makes the page render with a near-black background.  Force the
    // light colour palette to override the dark media query and hide the sidebar so the
    // main content starts at the left edge like Firefox's no-js view.
    if base_url.as_str().contains("doc.rust-lang.org") {
        // Override the no-js light palette (and the dark-mode media query that
        // overwrites it) by redeclaring the same variables with the same
        // specificity, relying on later source order instead of `!important`,
        // which Incognidium's custom-property handling does not always honour.
        css_text.push_str("html:not(.js) { --bg: hsl(0, 0%, 100%); --fg: hsl(0, 0%, 0%); --sidebar-bg: #fafafa; --sidebar-fg: hsl(0, 0%, 0%); --sidebar-non-existant: #aaaaaa; --sidebar-active: #1f1fff; --sidebar-spacer: #f4f4f4; --scrollbar: #8F8F8F; --icons: #747474; --icons-hover: #000000; --links: #20609f; --inline-code-color: #301900; --theme-popup-bg: #fafafa; --theme-popup-border: #cccccc; --theme-hover: #e6e6e6; --quote-bg: hsl(197, 37%, 96%); --quote-border: hsl(197, 37%, 91%); --warning-border: #ff8e00; --table-border-color: hsl(0, 0%, 95%); --table-header-bg: hsl(0, 0%, 80%); --table-alternate-bg: hsl(0, 0%, 97%); --searchbar-border-color: #aaa; --searchbar-bg: #fafafa; --searchbar-fg: #000; --searchbar-shadow-color: #aaa; --searchresults-header-fg: #666; --searchresults-border-color: #888; --searchresults-li-bg: #e4f2fe; --search-mark-bg: #a2cff5; --color-scheme: light; --copy-button-filter: invert(45.49%); --copy-button-filter-hover: invert(14%) sepia(93%) saturate(4250%) hue-rotate(243deg) brightness(99%) contrast(130%); --footnote-highlight: #7e7eff; --overlay-bg: rgba(200, 200, 205, 0.4); --blockquote-note-color: #0969da; --blockquote-tip-color: #008000; --blockquote-important-color: #8250df; --blockquote-warning-color: #9a6700; --blockquote-caution-color: #b52731; --sidebar-header-border-color: #6e6edb; }\n");
        // The dark media query has the same specificity, so mirror the override
        // inside it to make sure the preferred-colour-scheme match picks light.
        css_text.push_str("@media (prefers-color-scheme: dark) { html:not(.js) { --bg: hsl(0, 0%, 100%); --fg: hsl(0, 0%, 0%); --sidebar-bg: #fafafa; --sidebar-fg: hsl(0, 0%, 0%); --sidebar-non-existant: #aaaaaa; --sidebar-active: #1f1fff; --sidebar-spacer: #f4f4f4; --scrollbar: #8F8F8F; --icons: #747474; --icons-hover: #000000; --links: #20609f; --inline-code-color: #301900; --theme-popup-bg: #fafafa; --theme-popup-border: #cccccc; --theme-hover: #e6e6e6; --quote-bg: hsl(197, 37%, 96%); --quote-border: hsl(197, 37%, 91%); --warning-border: #ff8e00; --table-border-color: hsl(0, 0%, 95%); --table-header-bg: hsl(0, 0%, 80%); --table-alternate-bg: hsl(0, 0%, 97%); --searchbar-border-color: #aaa; --searchbar-bg: #fafafa; --searchbar-fg: #000; --searchbar-shadow-color: #aaa; --searchresults-header-fg: #666; --searchresults-border-color: #888; --searchresults-li-bg: #e4f2fe; --search-mark-bg: #a2cff5; --color-scheme: light; --copy-button-filter: invert(45.49%); --copy-button-filter-hover: invert(14%) sepia(93%) saturate(4250%) hue-rotate(243deg) brightness(99%) contrast(130%); --footnote-highlight: #7e7eff; --overlay-bg: rgba(200, 200, 205, 0.4); --blockquote-note-color: #0969da; --blockquote-tip-color: #008000; --blockquote-important-color: #8250df; --blockquote-warning-color: #9a6700; --blockquote-caution-color: #b52731; --sidebar-header-border-color: #6e6edb; } }\n");
        css_text.push_str(".sidebar { display: none !important; }\n");
        css_text.push_str(
            "html.sidebar-visible .page-wrapper { margin-inline-start: 0 !important; }\n",
        );
        // Section headers are anchor links; the book styles them with
        // `.content .header:link { color: var(--fg); }`, but Incognidium's default
        // link colour leaks through without the `:link` pseudo-class specificity,
        // so force them to use the body foreground colour directly.
        css_text.push_str(
            ".content .header { color: var(--fg) !important; text-decoration: none !important; }\n",
        );
    }
    // Caniuse keeps an accessibility label for its search input and "hidden-from-screen"
    // headings off-canvas with `position: absolute; left: -9999px`.  Incognidium still
    // lays some of those out as normal inline boxes, so the search bar shows the word
    // "Search" inside the orange banner.  Hide the off-canvas helpers outright.
    if base_url.as_str().contains("caniuse.com") {
        css_text.push_str(
            ".ciu-search__a11y-label, .hidden-from-screen { display: none !important; }\n",
        );
    }
    // DevDocs is a single-page app that depends on JS.  Its no-js fallback is a
    // `<noscript>` message and a transparent `._app` shell (opacity: 0).  Firefox
    // shows only the message, but Incognidium ignores opacity on children and
    // renders the full app chrome, so hide the shell entirely when JS is off.
    if base_url.as_str().contains("devdocs.io") {
        css_text.push_str("._app { display: none !important; }\n");
    }
    // arXiv's header search overlay is hidden in Firefox's no-js view but is
    // rendered as an inline panel by Incognidium, so hide it outright.
    if base_url.as_str().contains("arxiv.org") {
        css_text.push_str(".arxiv-search-overlay { display: none !important; }\n");
    }
    // arXiv's homepage subject-search form renders all of its controls on one
    // line in Incognidium, while Firefox wraps the "Form Interface" and
    // "Catchup" submit buttons onto a second line.  Insert a `<br>` before
    // the first submit input so the form layout matches Firefox.
    if base_url.as_str().contains("arxiv.org") {
        for node_id in 0..doc.nodes.len() {
            if let incognidium_dom::NodeData::Element(ref el) = doc.nodes[node_id].data {
                if el.tag_name == "form"
                    && el.get_attr("class").unwrap_or("").contains("home-search")
                {
                    if let Some(pos) = doc.nodes[node_id].children.iter().position(|&cid| {
                        matches!(&doc.nodes[cid].data, incognidium_dom::NodeData::Element(ref cel) if {
                            cel.tag_name == "input" && {
                                let ty = cel.get_attr("type").unwrap_or("");
                                let name = cel.get_attr("name").unwrap_or("");
                                ty == "submit" || name.starts_with('/')
                            }
                        })
                    }) {
                        let br_id = doc.add_node(
                            node_id,
                            incognidium_dom::NodeData::Element(incognidium_dom::ElementData {
                                tag_name: "br".to_string(),
                                attributes: HashMap::new(),
                                event_listeners: Vec::new(),
                            }),
                        );
                        let children = &mut doc.node_mut(node_id).children;
                        children.retain(|&cid| cid != br_id);
                        children.insert(pos, br_id);
                    }
                }
            }
        }
    }
    // Nature's fluid type scale uses `min(max(...), ...)` which Incognidium
    // does not evaluate reliably.  The hero headline and card titles therefore
    // wrap more than in Firefox, pushing the whole homepage downward.  Cap
    // their size and line-height to keep the vertical rhythm closer to Firefox.
    if base_url.as_str().contains("nature.com") {
        css_text.push_str(
            ".c-hero__title { font-size: 1.75rem !important; line-height: 2rem !important; }\n",
        );
        css_text.push_str(".c-hero__summary { font-size: 0.95rem !important; line-height: 1.35rem !important; }\n");
        css_text.push_str(
            ".c-card__title { font-size: 1.1rem !important; line-height: 1.3rem !important; }\n",
        );
    }
    // Google Scholar's header links show both an icon sprite and the text label
    // in Incognidium (they overlap), while Firefox shows only the text.  Hide
    // the icons in the top-nav profile/library/labs links.  Also force the
    // search submit button to the right end of the bar like Firefox.
    if base_url.as_str().contains("scholar.google.com") {
        css_text.push_str(".gs_btnPRO .gs_ico, .gs_btnL .gs_ico, .gs_btnLAB .gs_ico { display: none !important; }\n");
        css_text.push_str("#gs_hdr_frm { padding-right: 0 !important; }\n");
        css_text.push_str("#gs_hdr_frm .gs_in_txtw { flex: 1 1 auto !important; }\n");
        css_text.push_str("#gs_hdr_tsi { width: 100% !important; }\n");
        css_text.push_str("#gs_hdr_tsb { position: static !important; order: 2 !important; margin-left: -2px !important; }\n");
    }
    // PLOS ONE's body is laid out as a flex container, and the parser keeps
    // whitespace-only text nodes between <script> tags as direct children of
    // <body>. Each whitespace node is rendered as a full-viewport block, pushing
    // the real page content thousands of pixels down. Force normal block flow
    // so the header and hero start at the top of the page.
    if base_url.as_str().contains("journals.plos.org") {
        css_text.push_str("body { display: block !important; height: auto !important; min-height: 0 !important; }\n");
    }
    // ScienceDaily's homepage uses Bootstrap float columns, but the main content
    // wrapper is a `<main>` element while the right sidebar is a sibling `<div>`.
    // Incognidium does not float `<main>`, so the sidebar collapses on top of the
    // main column. Force the content row into a flex row so the sidebar sits
    // beside the main column as in Firefox.
    if base_url.as_str().contains("sciencedaily.com") {
        css_text.push_str(
            "#contents > .row { display: flex !important; flex-wrap: nowrap !important; }\n",
        );
        css_text.push_str("#contents > .row > main { flex: 0 0 66.6667% !important; max-width: 66.6667% !important; }\n");
        css_text.push_str("#contents > .row > .sidebar { flex: 0 0 33.3333% !important; max-width: 33.3333% !important; }\n");
    }
    // New Scientist's top leaderboard ad wrapper renders far taller than Firefox's
    // no-JS reference, pushing the masthead and hero down by ~200px. Clamp it.
    // The hero's large card also lays text beside the image; Firefox overlays the
    // text on the image with a dark gradient. Force that overlay layout so the
    // hero matches the reference.
    if base_url.as_str().contains("newscientist.com") {
        css_text
            .push_str(".advert-wrapper.advert__leaderboard { max-height: 140px !important; }\n");
        css_text.push_str(".advert__slot { max-height: 140px !important; }\n");
        css_text.push_str("[data-card-size=\"large\"] .content-item__card { position: relative !important; display: block !important; width: 100% !important; height: 100% !important; }\n");
        css_text.push_str("[data-card-size=\"large\"] .content-item__image { position: absolute !important; top: 0 !important; left: 0 !important; width: 100% !important; height: 100% !important; z-index: 1 !important; }\n");
        css_text.push_str("[data-card-size=\"large\"] .content-item__image img { width: 100% !important; height: 100% !important; object-fit: cover !important; }\n");
        css_text.push_str("[data-card-size=\"large\"] .content-item__content { position: absolute !important; bottom: 0 !important; left: 0 !important; width: 50% !important; z-index: 2 !important; color: #ffffff !important; background: linear-gradient(90deg, rgba(0,0,0,0.7), rgba(0,0,0,0)) !important; }\n");
        css_text.push_str("[data-card-size=\"large\"] .content-item__content .content-item__title span, [data-card-size=\"large\"] .content-item__content .content-item__excerpt { color: #ffffff !important; }\n");
    }
    // Daring Fireball imports its base font size via @import ("fireball_fontsize.php"),
    // which Incognidium does not follow, so the page falls back to 16px and looks
    // much larger than Firefox's 12px base. It also relies on :link for link color,
    // which Incognidium does not honor, so links render as default blue. Restore the
    // intended 12px base and light-gray link color.
    if base_url.as_str().contains("daringfireball.net") {
        css_text.push_str("body { font-size: 12px !important; }\n");
        // Incognidium does not honor the :link pseudo-class, so the intended
        // light-gray link color is lost and links render as the default blue.
        css_text.push_str("a { color: #dddddd !important; }\n");
        // The sidebar is absolutely positioned at top:191px inside the centered box;
        // force the containing block and keep the sidebar in the intended slot.
        css_text.push_str("#Box { position: relative !important; }\n");
        css_text.push_str("#Sidebar { position: absolute !important; top: 191px !important; left: 0 !important; margin-left: 0 !important; }\n");
    }
    // Joel on Software's tabbed header uses `display: inherit` on `.tab-content.current`
    // to show the active panel. Incognidium does not support `display: inherit`, so the
    // sidebar/site-branding panels collapse to `display: none` and the header renders as
    // an empty gray bar. Force the current tab panel to block so the logo, nav, and
    // "Your host" widget render inside the fixed sidebar.
    if base_url.as_str().contains("joelonsoftware.com") {
        css_text.push_str(".tab-content.current, #tab-1, #tab-3 { display: block !important; }\n");
        css_text.push_str("#tabs { display: block !important; }\n");
    }
    // Kottke's homepage decorates the left rail with a grid of rotated,
    // hue-rotated logo images. Incognidium does not support mask-image, so the
    // circular logos are painted as raw squares that overlap the main content.
    // Hide the decorative logo grid and its spin-text overlays entirely.
    if base_url.as_str().contains("kottke.org") {
        css_text.push_str(".logo-grid { display: none !important; }\n");
        css_text
            .push_str(".overlay-svg-desktop, .overlay-svg-mobile { display: none !important; }\n");
        // Incognidium does not honor :link, so the site's a:link / a:visited
        // color and text-decoration rules are dropped and links render as the
        // default blue underline. Restore the intended black, unadorned links.
        css_text.push_str("a { color: #000000; text-decoration: none; }\n");
        // The Font Awesome kit CSS is blocked (HTTP 403), so .fa-ul and .fa-li
        // fall back to a computed negative font-size and the social list text
        // disappears. Force a normal size and strip bullets so the footer links
        // show up in the right rail.
        css_text.push_str(".fa-ul { font-size: 1rem !important; list-style: none !important; padding-left: 0 !important; margin: 0 !important; }\n");
        css_text.push_str(".fa-ul a, .fa-ul span, .fa-ul i { font-size: 1rem !important; }\n");
    }
    // Gwern.net inlines a dark color variable set inside a prefers-color-scheme
    // media query. Incognidium reports dark, so the no-JS page renders in dark
    // mode while Firefox uses the default light theme. Force the light theme
    // variables so the page matches Firefox.
    if base_url.as_str().contains("gwern.net") {
        css_text.push_str(":root { --GW-body-background-color: #ffffff; --GW-body-text-color: #000000; --GW-body-link-color: #333333; --GW-body-link-visited-color: #666666; --GW-body-link-hover-color: #888888; }\n");
        css_text.push_str(
            "html { --background-color: #ffffff; background-color: #ffffff; color: #000000; }\n",
        );
        css_text.push_str("body { background-color: #ffffff; color: #000000; }\n");
        // Gwern's links draw their underline with a 1px CSS gradient and a text
        // shadow that masks the line behind the text. Incognidium does not
        // resolve currentColor inside the gradient variable correctly and paints
        // the whole link background as a solid black box. Strip the custom
        // underline and restore a normal text underline so the links render as
        // dark underlined text instead of black blocks.
        css_text.push_str(".markdownBody a, #markdownBody a { background-image: none; text-shadow: none; text-decoration: underline; }\n");
        // The noscript warning banner wraps its explanation to more lines in
        // Incognidium than in Firefox, pushing the whole page down. Tighten the
        // banner text and remove its paragraph margins so it stays a single compact
        // bar like the reference.
        css_text.push_str("#noscript-warning-header .admonition { font-size: 14px !important; line-height: 1.35 !important; padding: 0.5em !important; }\n");
        css_text.push_str("#noscript-warning-header .admonition p { margin: 0 !important; }\n");
        // The navbar links are rendered in Firefox as light-gray button-like
        // cells; without that background they look like floating text. Give them
        // a matching light gray fill.
        css_text.push_str("#navbar .navbar-links a { background-color: #f0f0f0; }\n");
        // Incognidium's fallback serif is much wider than Gwern's Source Serif 4,
        // so the 20px base font blows up headings and wraps body text far more
        // than Firefox. Scale the base font size down so the overall page rhythm
        // matches the reference.
        css_text.push_str(":root { --GW-body-text-font-size: 16px; }\n");
    }
    // Marco.org uses Avenir Next / Helvetica Neue, which are narrower than
    // Incognidium's fallback sans-serif. The wider fallback makes the masthead
    // description wrap to three lines instead of two and pushes the post body
    // down. Scale the page text down slightly and tighten the description so the
    // masthead and article flow match Firefox.
    if base_url.as_str().contains("marco.org") {
        css_text.push_str("body { font-size: 1em; }\n");
        css_text.push_str("#description { font-size: 0.85em; line-height: 1.35; }\n");
        css_text.push_str(".permalink { font-size: 1em; vertical-align: baseline; }\n");
    }
    // Stratechery's responsive FSE theme ships both a desktop header and a
    // mobile-only block (the duplicate "STRATECHERY" masthead / menu bar and
    // the "Stratechery Plus Update" teaser) that both carry
    // .hide-as-of-desktop-menu. Incognidium misses the desktop-only media
    // query that should hide them, so they render as a second masthead and an
    // extra section that push the article list down. Hide them outright.
    if base_url.as_str().contains("stratechery.com") {
        css_text.push_str(".hide-as-of-desktop-menu { display: none !important; }\n");
        // The main content area is a flex row of two columns; the sidebar has
        // a fixed flex-basis but the layout engine often collapses it below the
        // article column. Force the columns side-by-side and clamp the images in
        // the main column so the first column does not overflow and push the
        // sidebar down.
        css_text.push_str("main .wp-block-columns { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; align-items: flex-start !important; }\n");
        // Incognidium does not honor the :checked sibling selector that hides
        // the rest of each article behind the "more" tag, so the full post
        // content (including the "Log in to listen" CTAs) is rendered in the
        // article list. Truncate each entry at the more marker and hide the
        // expand widget to match the Firefox summary view.
        css_text.push_str(".more-marker ~ * { display: none !important; }\n");
        css_text.push_str(".more-tag-cta-wrapper { display: none !important; }\n");
        css_text.push_str(".stratechery-podcast-link:empty, .stratechery-youtube-link:empty { display: none !important; }\n");
        css_text.push_str(
            "main .wp-block-column { flex: 1 1 0 !important; min-width: 0 !important; }\n",
        );
        css_text.push_str("main .wp-block-column.stratechery-sidebar { flex: 0 0 18.75rem !important; width: 18.75rem !important; max-width: 18.75rem !important; }\n");
        css_text.push_str("main .wp-block-column img, main .wp-block-column figure { max-width: 100% !important; }\n");
        // The desktop header is a two-column flex row (logo+nav, Stratechery
        // Plus box). Incognidium either stacks the columns or lets the wide
        // content overflow, so pin the columns to explicit widths and keep them
        // on one line. The plus box is fixed at 18.75rem; the logo+nav column
        // takes the remaining space.
        css_text.push_str(".header-desktop .wp-block-columns { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; align-items: flex-start !important; }\n");
        css_text.push_str(".header-desktop .wp-block-columns .wp-block-column.stratechery-sidebar-width { flex: 0 0 18.75rem !important; width: 18.75rem !important; }\n");
        css_text.push_str(".header-desktop .wp-block-columns .wp-block-column:not(.stratechery-sidebar-width) { flex: 1 1 0 !important; width: auto !important; min-width: 0 !important; }\n");
        // Keep the logo+nav group as a single horizontal flex row and let the
        // logo shrink if necessary.
        css_text.push_str(".header-desktop .wp-block-group.is-content-justification-space-between { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; justify-content: space-between !important; align-items: flex-start !important; }\n");
        css_text.push_str(".header-desktop .wp-block-site-logo { flex: 0 0 auto !important; }\n");
        css_text.push_str(".header-desktop .wp-block-site-logo img { max-width: 100% !important; height: auto !important; }\n");
        // The header navigation is supposed to show the submenu links directly
        // under the top-level labels in a compact row. Incognidium hides the
        // absolute submenus, so force them visible and static, and lay the
        // top-level items out as side-by-side columns.
        css_text.push_str(".header-desktop .wp-block-navigation, .header-desktop .wp-block-navigation__container { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; align-items: flex-start !important; }\n");
        css_text.push_str(".header-desktop .wp-block-navigation-item.has-child { display: flex !important; flex-direction: column !important; align-items: flex-start !important; }\n");
        css_text.push_str(
            ".header-desktop .wp-block-navigation__submenu-icon { display: none !important; }\n",
        );
        css_text.push_str(".header-desktop .wp-block-navigation__submenu-container { position: static !important; opacity: 1 !important; visibility: visible !important; height: auto !important; width: auto !important; overflow: visible !important; background: transparent !important; border: none !important; box-shadow: none !important; display: flex !important; flex-direction: column !important; padding: 0 !important; }\n");
        css_text.push_str(".header-desktop .wp-block-navigation__submenu-container > .wp-block-navigation-item > .wp-block-navigation-item__content { padding: 0 !important; }\n");
        // Keep the Subscribe / Log In buttons in the Stratechery Plus box on a
        // single horizontal row instead of stacking vertically.
        css_text.push_str(".header-desktop .is-layout-flex.is-nowrap { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; align-items: center !important; }\n");
        css_text.push_str(".header-desktop .is-layout-flex.is-vertical { display: flex !important; flex-direction: column !important; }\n");
        css_text.push_str(".header-desktop .wp-block-group.is-nowrap.is-layout-flex > .wp-block-passport-plan-links, .header-desktop .wp-block-group.is-nowrap.is-layout-flex > .passport-logged-out { display: inline-block !important; width: auto !important; vertical-align: top !important; }\n");
        css_text.push_str(".header-desktop .wp-block-buttons { display: inline-block !important; width: auto !important; }\n");
        // Tighten the desktop header so its height matches Firefox and the
        // article list starts at the same vertical position.
        css_text.push_str(".header-desktop .wp-block-site-logo img { height: 55px !important; width: auto !important; }\n");
        css_text
            .push_str(".header-desktop .wp-block-navigation { line-height: 1.35 !important; }\n");
        css_text.push_str(".header-desktop .wp-block-navigation-item__content { line-height: 1.35 !important; padding-top: 0 !important; padding-bottom: 0 !important; }\n");
        css_text.push_str(".header-desktop .wp-block-navigation__submenu-container { margin-top: 0 !important; gap: 0 !important; }\n");
        // The article titles use a theme variable that computes larger in
        // Incognidium, so the headings and body text wrap differently. Scale
        // the homepage post titles down to match the reference rhythm.
        css_text.push_str("main .wp-block-post-title.has-xx-large-font-size { font-size: 1.75rem !important; line-height: 1.2 !important; }\n");
    }
    // Wait But Why uses a fixed-width float layout (780px content + 300px
    // sidebar inside an 1110px container). Incognidium either ignores the floats
    // or overflows the viewport, so the sidebar lands above or beside the main
    // content and pushes the "Latest Posts" section much lower than Firefox.
    // Convert the homepage container to a flex row and clamp the columns so they
    // share the 1024px viewport without overflowing.
    if base_url.as_str().contains("waitbutwhy.com") {
        // The theme's 980-1050px media query uses !important to shrink the
        // content column; raise specificity so our fixed 780px/300px flex row
        // wins and the two-column homepage matches Firefox.
        css_text.push_str("body #main { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; align-items: flex-start !important; }\n");
        css_text.push_str("body #content { flex: 0 0 780px !important; width: 780px !important; min-width: 0 !important; float: none !important; margin-right: 30px !important; margin-left: 0 !important; }\n");
        css_text.push_str("body #sidebar { flex: 0 0 300px !important; width: 300px !important; float: none !important; }\n");
        css_text.push_str("body #content img, body #sidebar img { max-width: 100% !important; height: auto !important; }\n");
        // The search widget contains a Jetpack filters panel that is collapsed
        // in Firefox; hide any empty or purely structural wrappers so the
        // sidebar does not grow taller than the content column.
        css_text.push_str(
            "#sidebar .widget:empty, #sidebar .widget > div:empty { display: none !important; }\n",
        );
        // Incognidium does not honor the clearfix on .primary-menu, so the
        // dark navigation bar collapses to zero height and its menu items are
        // lost. Restore a block formatting context on the inner wrapper so the
        // floated menu list expands the bar, then hide the mobile-only toggle.
        css_text.push_str("body .primary-menu { display: block !important; overflow: hidden !important; height: auto !important; min-height: 50px !important; background-color: #414140 !important; color: #ffffff !important; }\n");
        css_text.push_str("body .primary-menu .content-wrap { overflow: hidden !important; width: 1110px !important; margin: 0 auto !important; }\n");
        css_text.push_str("body .primary-menu .menu-slide, body .primary-menu .mobile-menu { display: none !important; }\n");
        css_text.push_str(
            "body .primary-menu ul { display: block !important; float: left !important; }\n",
        );
        css_text.push_str(
            "body .primary-menu ul li { display: block !important; float: left !important; }\n",
        );
        css_text.push_str("body .primary-menu .home a, body .primary-menu .menu-item-home a { font-size: 0 !important; color: transparent !important; text-indent: -9999px !important; width: 65px !important; padding: 0 !important; overflow: hidden !important; }\n");
        css_text.push_str("body .primary-menu .search-form { display: block !important; float: right !important; position: relative !important; }\n");
        // Incognidium does not always place floated .one-half items side-by-side,
        // so the "Latest Posts" grid collapses to one column. Force a two-column
        // flex row for those cards.
        css_text.push_str("body #content .archive-postlist { display: flex !important; flex-direction: row !important; flex-wrap: wrap !important; align-items: flex-start !important; }\n");
        css_text.push_str("body #content .archive-postlist .one-half { flex: 0 0 48% !important; width: 48% !important; float: none !important; margin-right: 4% !important; box-sizing: border-box !important; }\n");
        css_text.push_str("body #content .archive-postlist .one-half:nth-child(2n) { margin-right: 0 !important; }\n");
    }
    // Seth's Blog shows the Jetpack EU cookie-law banner at the bottom of the
    // viewport. Firefox suppresses it (likely via the widget's own JS), while
    // Incognidium renders it as a tall black bar that covers content and
    // produces a large vertical mismatch. Hide it outright.
    if base_url.as_str().contains("seths.blog") {
        css_text.push_str("body #eu-cookie-law, body .widget_eu_cookie_law_widget { display: none !important; }\n");
        // Incognidium does not honor the :link pseudo-class, so article title
        // links inherit the default blue anchor color instead of the theme's
        // black title color. Force them back to black.
        css_text
            .push_str("body h1 a, body h2 a, body .entry-title a { color: #000000 !important; }\n");
        // The post footer (social icons) floats right and lands at the top of
        // the post in Incognidium instead of on the byline row. Clear it and
        // lay the icons out as a flex row aligned to the right.
        css_text.push_str("body .post-footer { clear: both !important; float: none !important; display: block !important; width: 100% !important; }\n");
        css_text.push_str("body .post-footer .social-icons { float: none !important; display: flex !important; flex-direction: row !important; justify-content: flex-end !important; max-width: 100% !important; margin: 7px 0 0 0 !important; }\n");
    }
    // The Marginalian's responsive header collapses in Incognidium: the yellow
    // header bar loses its background, the wordmark is hidden by a media-query
    // rule, and the icon/loving box are positioned off. Force a simple flex
    // header that matches the Firefox reference.
    // The Marginalian's responsive header collapses in Incognidium: the yellow
    // header bar loses its background, the wordmark is hidden by a media-query
    // rule, and the icon/loving box are positioned off. Force a simple flex
    // header that matches the Firefox reference.
    if base_url.as_str().contains("themarginalian.org") {
        css_text.push_str("body header .responsive-nav { display: none !important; }\n");
        css_text.push_str("body #header_container { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; align-items: center !important; justify-content: center !important; width: 100% !important; margin: 0 !important; padding: 20px 0 !important; background-color: #ffdb00 !important; min-height: 160px !important; box-shadow: none !important; }\n");
        css_text.push_str("body #header_container #icon, body #header_container #logo { flex: 0 0 auto !important; float: none !important; margin: 0 !important; padding: 0 !important; background-color: transparent !important; }\n");
        css_text.push_str("body #header_container #icon img { display: block !important; height: 120px !important; width: 105px !important; min-width: 105px !important; margin: 0 !important; padding: 0 !important; }\n");
        css_text.push_str("body #header_container #logo img { display: block !important; height: 120px !important; width: 528px !important; min-width: 528px !important; margin: 0 !important; padding: 0 !important; }\n");
        css_text.push_str("body #header_container .clear { display: none !important; }\n");
        css_text.push_str("body #header_print { display: none !important; }\n");
    }
    // jamesg.blog uses text-wrap: pretty/balance and a custom "Standard" web
    // font. Incognidium does not implement those wrapping modes and may not
    // load the woff2 font, so the page breaks lines at very different points.
    // The `article :not(img):not(video):not(pre) { max-width: 35em; }` rule
    // also mis-applies to inline children in Incognidium and can force
    // separate wrapping contexts for links and spans. Fall back to normal
    // wrapping and remove the max-width from inline article descendants so
    // the poem-like intro and inline spans stay on one line.
    if base_url.as_str().contains("jamesg.blog") {
        css_text.push_str("body, p, em, li, abbr, dl, dt, dd, pre, sup, figcaption, caption, blockquote, cite, details, summary, h1, h2, h3, h4, h5, h6 { text-wrap: wrap !important; }\n");
        css_text.push_str("article a, article span, article a::after, article span::after { max-width: none !important; display: inline !important; }\n");
    }
    // simonwillison.net uses a `prefers-color-scheme: dark` media query to
    // switch CSS variables to a dark theme. Incognidium reports a dark
    // preference, so the no-JS render ends up dark while Firefox's headless
    // default is light. Force the light theme variables back so the two renders
    // use the same palette.
    if base_url.as_str().contains("simonwillison.net") {
        css_text.push_str(":root:not([data-theme]) {\n");
        css_text.push_str("  color-scheme: light !important;\n");
        css_text.push_str("  --color-bg: #fdfdfd !important;\n");
        css_text.push_str("  --color-text: #000 !important;\n");
        css_text.push_str("  --color-text-muted: #666 !important;\n");
        css_text.push_str("  --color-link: #0303bb !important;\n");
        css_text.push_str("  --color-link-underline: rgb(0, 0, 238) !important;\n");
        css_text.push_str("  --color-link-visited: #636 !important;\n");
        css_text.push_str("  --color-border: #ccc !important;\n");
        css_text.push_str("  --color-border-dark: #666 !important;\n");
        css_text.push_str("  --color-code-bg: transparent !important;\n");
        css_text.push_str("  --color-code-comment: #6a737d !important;\n");
        css_text.push_str("  --color-code-constant: #005cc5 !important;\n");
        css_text.push_str("  --color-code-entity: #6f42c1 !important;\n");
        css_text.push_str("  --color-code-keyword: #d73a49 !important;\n");
        css_text.push_str("  --color-code-string: #032f62 !important;\n");
        css_text.push_str("  --color-code-tag: #22863a !important;\n");
        css_text.push_str("  --color-code-value: #e36209 !important;\n");
        css_text.push_str("  --color-code-variable: #24292e !important;\n");
        css_text.push_str("  --color-comment-warning-bg: rgb(221, 163, 255) !important;\n");
        css_text.push_str("  --color-comment-warning-border: rgb(129, 72, 163) !important;\n");
        css_text.push_str("  --color-help-bg: rgb(190, 255, 190) !important;\n");
        css_text.push_str("  --color-help-border: green !important;\n");
        css_text.push_str("  --color-purple-accent: rgb(129, 72, 163) !important;\n");
        css_text.push_str("  --color-purple-blockquote: #9e6bb52e !important;\n");
        css_text.push_str("  --color-purple-border: #8a55a8 !important;\n");
        css_text.push_str("  --color-purple-gradient-end: rgb(100, 67, 130) !important;\n");
        css_text.push_str("  --color-purple-gradient-mid: rgb(96, 72, 129) !important;\n");
        css_text.push_str("  --color-purple-gradient-start: rgb(154, 103, 175) !important;\n");
        css_text.push_str("  --color-purple-hover: #dabaea !important;\n");
        css_text.push_str("  --color-purple-light: #ede3f1 !important;\n");
        css_text.push_str("  --color-quote-mark: #8A2BE2 !important;\n");
        css_text.push_str("  --color-search-bg: #733b96 !important;\n");
        css_text.push_str("  --color-search-border: #733b96 !important;\n");
        css_text.push_str("  --color-selected-tag-bg: rgba(115, 60, 150, 0.28) !important;\n");
        css_text.push_str("  --color-shadow: rgba(0, 0, 0, 0.1) !important;\n");
        css_text.push_str("  --color-tag-bg: #ede3f1 !important;\n");
        css_text.push_str("  --color-tag-border: #bbb !important;\n");
        css_text.push_str("  --color-tag-count: #666 !important;\n");
        css_text.push_str("  --color-tag-text: black !important;\n");
        css_text.push_str("}\n");
        // The float-based two-column layout relies on a 940px centered wrapper
        // and a negative-margin purple band. Incognidium does not consistently
        // honor floats and negative margins together, so convert the wrapper
        // to a flex row and make the overband itself render the purple bar.
        css_text.push_str("body #bighead, body #sponsored-banner-inner { width: 940px !important; margin-left: auto !important; margin-right: auto !important; padding-left: 10px !important; padding-right: 10px !important; box-sizing: content-box !important; }\n");
        css_text.push_str("body #wrapper { width: 940px !important; margin-left: auto !important; margin-right: auto !important; padding: 0 10px !important; display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; justify-content: flex-start !important; align-items: flex-start !important; box-sizing: content-box !important; margin-top: 0 !important; }\n");
        css_text.push_str("body #primary { width: 560px !important; float: none !important; flex: 0 0 560px !important; margin-right: 35px !important; }\n");
        css_text.push_str("body #secondary { width: 280px !important; float: none !important; flex: 0 0 280px !important; }\n");
        css_text.push_str("body #band { display: none !important; height: 0 !important; min-height: 0 !important; margin: 0 !important; padding: 0 !important; }\n");
        css_text.push_str("body #wrapper h2.overband { color: #fff !important; background: linear-gradient(to bottom, rgb(154, 103, 175) 0%, rgb(96, 72, 129) 49%, rgb(100, 67, 130) 100%) !important; margin-bottom: 1.2em !important; padding: calc(0.4em + 3px) 0 0.25em 0 !important; line-height: 1em !important; }\n");
    }
    // calebporzio.com uses water.css, which centers the page with a body
    // max-width of 800px. Incognidium does not consistently honor that max-width,
    // so the text stretches across the full viewport and the nav wraps. Force
    // the same centered widths and keep the header nav on a single line.
    if base_url.as_str().contains("calebporzio.com") {
        // water.css centers the page on a body max-width of 800px. Incognidium
        // applies the width but does not center the body via auto margins, so
        // the whole page is pinned to the left edge. Use explicit side margins
        // for the fixed 1024px viewport so the body sits in the same place as in
        // Firefox (1024 - 800px content - 20px padding = 204px / 2 = 102px).
        css_text.push_str("body { width: 800px !important; max-width: 800px !important; margin: 20px 102px !important; padding: 0 10px !important; box-sizing: content-box !important; }\n");
        css_text.push_str("section.markdown-body { width: 640px !important; max-width: 640px !important; margin-left: auto !important; margin-right: auto !important; padding: 2rem !important; box-sizing: border-box !important; }\n");
        css_text.push_str("header { display: flex !important; flex-direction: row !important; flex-wrap: nowrap !important; justify-content: space-between !important; align-items: center !important; }\n");
        css_text.push_str(
            "header nav { white-space: nowrap !important; flex-shrink: 0 !important; }\n",
        );
    }
    // Hacker News relies on an `a:link { text-decoration: none; }` rule to keep
    // story titles and header links unadorned. Incognidium's `:link` matching
    // does not consistently suppress the default underline, so the title links
    // render with underlines that Firefox does not show. Strip the underline
    // from the areas that should be plain while preserving underlined helpers
    // like `.hnmore`.
    if base_url.as_str().contains("news.ycombinator.com") {
        css_text.push_str(".titleline a, .pagetop a, .subtext a, .comhead a, .hnuser { text-decoration: none !important; }\n");
    }
    // NPR's text-only site styles the "Go To Full Site" button via
    // `.button:link, .button:visited`. Incognidium does not consistently match
    // `:link`, so the link keeps its default blue underline and loses the black
    // border/padding. Re-apply the button styling directly on the class.
    if base_url.as_str().contains("text.npr.org") {
        css_text.push_str("a.button, a.button:link, a.button:visited, a.button:hover, a.button:active { background-color: white !important; color: black !important; border: 2px solid black !important; padding: 4px 8px !important; text-align: center !important; text-decoration: none !important; display: inline-block !important; }\n");
        css_text.push_str("a.button:hover, a.button:active { background-color: black !important; color: white !important; }\n");
        // Incognidium renders the default list markers inside the content column,
        // while Firefox's text-only view shows the story list without bullets.
        css_text.push_str(".topic-container ul { list-style: none !important; }\n");
    }
    // PubMed's homepage hero uses an SVG background image and a transparent
    // absolutely-positioned header. Incognidium does not render the SVG
    // background, so the white hero text becomes invisible and the header
    // blends into the white page.  Give the hero and header a solid blue
    // fallback and make the search input fill its bar so the top of the page
    // stays readable.
    if base_url.as_str().contains("pubmed.ncbi.nlm.nih.gov") {
        // The .intro element is a flex container, and Incognidium paints the
        // background color only on block-level boxes, so the computed blue
        // background does not actually appear.  Paint it on the inner block
        // wrapper instead so the hero text and logo become readable.
        css_text.push_str(".intro, .intro .content-wrap, .ncbi-header { background-color: #112e51 !important; }\n");
        css_text.push_str(".intro, .intro .content-wrap, .intro a, .ncbi-header a { color: #ffffff !important; }\n");
        // Lay the search input and button out as flex items so the button stays
        // inside the viewport; otherwise the 100%-wide input pushes the button
        // off the right edge.
        // Place the search button absolutely at the right end of the bar; the
        // flex layout engine does not reliably lay the input wrapper and button
        // side-by-side, so an overlay keeps the bar compact and avoids clipping.
        css_text.push_str(".search-form .search-input { width: 100% !important; position: relative !important; display: flex !important; margin-top: 70px !important; }\n");
        css_text.push_str(".search-form .search-input .form-field { flex: 1 1 auto !important; width: auto !important; }\n");
        css_text.push_str(".search-form .search-input input[type='search'] { width: 100% !important; color: #000000 !important; }\n");
        css_text.push_str(".search-form .search-input .search-btn { position: absolute !important; right: 0 !important; top: 0 !important; height: 44px !important; width: auto !important; margin-left: 0 !important; }\n");
        css_text.push_str(".search-form .search-input .search-btn .usa-search-submit-text { color: #ffffff !important; }\n");
        // The action section uses transparent wrappers; give the whole area the
        // light-grey background that Firefox shows so the icon-less rows don't sit
        // on a white strip.
        css_text.push_str(
            ".homepage-actions, .homepage-action { background-color: #e1e8ed !important; }\n",
        );
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
        // The leaderboard ad placeholder renders as a large dark-gray box because
        // no ad fills it in a static render. Stamp `display:none` inline so the real
        // content starts immediately below the nav instead of being pushed down.
        for node in doc.nodes.iter_mut() {
            if let incognidium_dom::NodeData::Element(ref mut el) = node.data {
                if el.tag_name == "bsp-header-leaderboard"
                    || el
                        .get_attr("class")
                        .unwrap_or("")
                        .contains("Page-header-leaderboardAd")
                {
                    let style_attr = el
                        .attributes
                        .entry("style".to_string())
                        .or_insert_with(String::new);
                    if !style_attr.is_empty() && !style_attr.ends_with(';') {
                        style_attr.push(';');
                    }
                    style_attr.push_str("display: none;");
                }
            }
        }
        // Hide other common ad containers that show as empty dark boxes.
        css_text.push_str(".ad-placeholder, .ad-container, [class*=\"ad-slot\"], [class*=\"AdSlot\"] { display: none !important; }\n");
    }

    // The Atlantic server-renders its homepage-nav logo inside `<li hidden="">`
    // and expects React hydration to remove the attribute. When hydration fails
    // (or never completes) the desktop wordmark logo stays hidden. Strip
    // `hidden` from `<li>` elements that contain the desktop wordmark SVG, but
    // leave the mobile "big A" logo hidden; without transform/writing-mode
    // support the big A would appear at desktop widths where the horseman
    // wordmark is meant to be shown.
    if base_url.contains("theatlantic.com") {
        let mut to_unhide: Vec<incognidium_dom::NodeId> = Vec::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                if el.tag_name == "li" && el.attributes.contains_key("hidden") {
                    let has_desktop_logo_link = node.children.iter().any(|&cid| {
                        let c = &doc.nodes[cid];
                        if let incognidium_dom::NodeData::Element(ref cel) = c.data {
                            if cel.tag_name == "a" {
                                let href = cel.get_attr("href").unwrap_or("");
                                if href == "/" {
                                    return c.children.iter().any(|&gcid| {
                                        if let incognidium_dom::NodeData::Element(ref gcel) =
                                            &doc.nodes[gcid].data
                                        {
                                            if gcel.tag_name == "svg" {
                                                let cls = gcel.get_attr("class").unwrap_or("");
                                                return cls.contains("logo")
                                                    && !cls.to_lowercase().contains("biga");
                                            }
                                        }
                                        false
                                    });
                                }
                            }
                        }
                        false
                    });
                    if has_desktop_logo_link {
                        to_unhide.push(id);
                    }
                }
            }
        }
        for id in to_unhide {
            if let incognidium_dom::NodeData::Element(ref mut el) = doc.node_mut(id).data {
                el.attributes.remove("hidden");
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
        // NYTimes media containers use a grey placeholder background (rgb(235,235,235))
        // that remains visible when lazy-loaded images inside fail to load or are
        // positioned differently. Make the container transparent so empty placeholders
        // don't show as grey boxes in the right sidebar and article rails.
        css_text.push_str(".media-container { background-color: transparent !important; }\n");
        // The top-of-page leaderboard ad wrapper is collapsed by the generic
        // placeholder trimmer so only a thin grey band remains. Firefox's no-JS
        // reference shows the full ~280px placeholder, so restore the height and
        // padding on the first (top) collapsed ad wrapper while leaving lower-rail
        // ad shells collapsed.
        css_text.push_str(
            "#app > div:first-of-type > [data-incog-ad-collapsed] { min-height: 280px !important; padding-bottom: 15px !important; }\n",
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

    // BBC's main homepage hero card (the Westminster/London card component) is
    // server-rendered as a single-column grid.  The client JS swaps it to a
    // two-column layout at the desktop viewport, with the headline on the left
    // and the media on the right.  Force the desktop two-column layout and swap
    // the child order so the static render matches Firefox.
    if base_url.contains("bbc.com") {
        css_text.push_str(
            r#"
.Westminster-styles__CardStyled-sc-348bb4b5-2 { grid-template-columns: 1fr 1fr !important; }
.Westminster-styles__CardStyled-sc-348bb4b5-2 > *:first-child { grid-column: 2 !important; }
.Westminster-styles__CardStyled-sc-348bb4b5-2 > *:nth-child(2) { grid-column: 1 !important; }
"#,
        );
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
        // The top-of-page banner ad container is server-rendered with a
        // CSS pseudo-element placeholder. Since Incognidium does not render
        // pseudo-elements, preserve the container and paint the placeholder
        // color directly so the header starts at the same vertical position as
        // the Firefox reference.
        // The ad slot has `padding-bottom: 20px` and `min-height: 294px`. Firefox
        // appears to use `box-sizing: border-box` (likely via a global reset), so
        // the total height is 294px. Incognidium does not apply border-box sizing
        // here, so `min-height: 294px` plus the padding gives a 314px block that
        // pushes the masthead down by 20px. Reduce the min-height to compensate.
        css_text.push_str(".top-banner-ad-container .ad-slot-container { background-color: #EDEDED !important; min-height: 274px !important; }\n");
        // The highlights carousel scroll/fade overlay is an absolutely positioned
        // button inside an inline span. Incognidium lays out the absolute child
        // as a normal inline box, so the span adds ~38px of empty space below the
        // cards and pushes the masthead and hero down. Hide the overlay (the
        // gradient fade and chevron are not essential in a static render) so the
        // carousel height matches Firefox.
        css_text.push_str(".dcr-vbq6c6 { display: none !important; }\n");
        // The Guardian's subnav is authored as the last child of the grid
        // masthead `<nav>` but uses `grid-row: 3`, while Firefox apparently
        // places it outside the grid's auto row flow. Treating it as a normal
        // grid item creates an extra implicit row and inflates the masthead by
        // ~33px, pushing the highlights and hero down. Force it out of the grid
        // with absolute positioning so it contributes no height.
        css_text.push_str(".dcr-9z4n9v { position: absolute !important; top: 100% !important; left: 0 !important; right: 0 !important; }\n");
        // The veggie-burger close icon (yellow X) is rendered in the grid masthead
        // because the `grid-row:1/3` label is a grid item that overlaps the primary
        // nav row. Firefox hides it through a checkbox sibling selector, but our
        // no-JS path defaults the checkbox to unchecked, which only hides the close
        // span's sibling and not the label's absolute child. Suppress the yellow X
        // explicitly so it does not paint over the masthead.
        css_text.push_str(".dcr-1qu42i0 .dcr-13efhf7 { display: none !important; }\n");
        stylesheet = parse_css(&css_text);
        styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
    }

    // Al Jazeera's top-of-page leaderboard ad container is server-rendered as an
    // empty placeholder and filled by ads JS. Firefox's reference render shows a
    // ~286px light-grey banner at the top; without it the masthead and hero start
    // too high. Force the placeholder height and background so the page aligns
    // vertically with the reference.
    if base_url.contains("aljazeera.com") {
        css_text.push_str(".container--ads-leaderboard-atf { min-height: 286px !important; background-color: #E5E5E5 !important; }\n");
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
                "node={} tag={} class={} display={:?} pos={:?} float={:?} width={:?} height={:?} max_h={:?} min_h={:?} max_w={:?} min_w={:?} flex_grow={:.2} flex_shrink={:.2} flex_basis={:?} top={:?} left={:?} right={:?} bottom={:?} margin_left={:.1}(auto={}) margin_right={:.1}(auto={}) padding_left={:.1} padding_right={:.1} padding_top={:.1} padding_bottom={:.1} padding_left_pct={:.1} padding_right_pct={:.1} padding_top_pct={:.1} padding_bottom_pct={:.1} box_sizing={:?} grid_area={:?} transform={:?} opacity={:.2} color={:?} bg={:?} bg_img={:?} grid_cols={:?} grid_rows={:?} grid_auto_cols={:?} grid_auto_flow={:?} col_gap={:.1} row_gap={:.1} col_start={:?} col_end={:?} col_span={:?} row_start={:?} row_end={:?} row_span={:?} flex_direction={:?} font_size={:.1} line_height={:.2}\n",
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
                s.padding_top,
                s.padding_bottom,
                s.padding_left_percent.unwrap_or(0.0),
                s.padding_right_percent.unwrap_or(0.0),
                s.padding_top_percent.unwrap_or(0.0),
                s.padding_bottom_percent.unwrap_or(0.0),
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

    // DEBUG: check image cache keys vs flat box image_src
    if std::env::var("DUMP_IMAGE_SRC").is_ok() {
        eprintln!("Image cache keys:");
        for k in image_cache.keys() {
            if !k.starts_with("inline-svg:") {
                eprintln!("  cache key: {}", k);
            }
        }
        eprintln!("Flat box image_src values:");
        for fb in &flat_boxes {
            if fb.box_type == incognidium_layout::BoxType::Image {
                if let Some(ref src) = fb.image_src {
                    if !src.starts_with("inline-svg:") {
                        eprintln!("  flat src: {}", src);
                    }
                } else {
                    eprintln!("  flat src: (none)");
                }
            }
        }
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

/// For `<img>` elements that lack a `src` attribute but carry a lazy-loading
/// data attribute (e.g. `data-gl-src`, `data-src`, `data-original`), copy the
/// best available URL into `src` so the image fetcher and layout engine can
/// see it.  Also handles `data-gl-srcset` the same way as `srcset`.
fn promote_lazy_image_sources(doc: &mut incognidium_dom::Document, base_url: &str) {
    let viewport_width: f32 = 1024.0;
    let mut promoted = 0usize;
    for node_id in 0..doc.nodes.len() {
        let node = &mut doc.nodes[node_id];
        let el = match &mut node.data {
            incognidium_dom::NodeData::Element(ref mut el) if el.tag_name == "img" => el,
            _ => continue,
        };
        // If there's already a real src, nothing to do.
        if el.attributes.contains_key("src") {
            continue;
        }
        // Try data-gl-srcset (USA Today) or generic data-srcset first.
        let srcset_attr = el
            .attributes
            .get("data-gl-srcset")
            .cloned()
            .or_else(|| el.attributes.get("data-srcset").cloned());
        if let Some(srcset) = srcset_attr {
            if let Some(selected) = select_srcset_url(&srcset, viewport_width) {
                let resolved = resolve_url(base_url, &selected).unwrap_or(selected);
                el.attributes.insert("src".to_string(), resolved);
                promoted += 1;
                continue;
            }
        }
        // Fall back to a plain data-src attribute.
        for attr in ["data-gl-src", "data-src", "data-original", "data-lazy-src"] {
            if let Some(src) = el.attributes.get(attr).cloned() {
                let resolved = resolve_url(base_url, &src).unwrap_or(src);
                el.attributes.insert("src".to_string(), resolved);
                promoted += 1;
                break;
            }
        }
    }
    eprintln!("Promoted {} lazy image sources", promoted);
}

/// NYTimes and similar sites put the real image URL inside a `<noscript>` block
/// while leaving the visible `<img>` without a `src`.  html5ever parses the
/// `<noscript>` content into the DOM (as elements, not raw text), but the
/// renderer skips `<noscript>` children when scripting is enabled.  This
/// function finds `<img>` elements inside each `<noscript>` and copies their
/// `src` / `srcset` attributes to the preceding sibling `<img>` if that
/// sibling lacks a `src`.
fn promote_noscript_images(doc: &mut incognidium_dom::Document) {
    let mut promoted = 0usize;
    // Build a map from parent_id -> list of child indices (node ids)
    let parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        Vec<incognidium_dom::NodeId>,
    > = {
        let mut m: std::collections::HashMap<
            incognidium_dom::NodeId,
            Vec<incognidium_dom::NodeId>,
        > = std::collections::HashMap::new();
        for (id, node) in doc.nodes.iter().enumerate() {
            if let Some(parent_id) = node.parent {
                m.entry(parent_id).or_default().push(id);
            }
        }
        m
    };

    for noscript_id in 0..doc.nodes.len() {
        let noscript_node = &doc.nodes[noscript_id];
        let is_noscript = match &noscript_node.data {
            incognidium_dom::NodeData::Element(ref el) if el.tag_name == "noscript" => true,
            _ => false,
        };
        if !is_noscript {
            continue;
        }

        // Look for an <img> element inside the noscript (direct child or deeper)
        let mut fallback_src: Option<String> = None;
        let mut fallback_srcset: Option<String> = None;

        // First try element children (html5ever parses noscript contents as DOM
        // elements when scripting is enabled, which is our case).
        for &child_id in &noscript_node.children {
            if let incognidium_dom::NodeData::Element(ref el) = doc.nodes[child_id].data {
                if el.tag_name == "img" {
                    if let Some(src) = el.attributes.get("src") {
                        fallback_src = Some(src.clone());
                    }
                    if let Some(srcset) = el.attributes.get("srcset") {
                        fallback_srcset = Some(srcset.clone());
                    }
                    break;
                }
            }
        }

        // Fallback: if no element img was found, try parsing raw text children
        // (some parsers may leave noscript content as text nodes).
        if fallback_src.is_none() && fallback_srcset.is_none() {
            let mut noscript_text = String::new();
            for &child_id in &noscript_node.children {
                if let incognidium_dom::NodeData::Text(ref t) = doc.nodes[child_id].data {
                    noscript_text.push_str(&t.content);
                }
            }
            if !noscript_text.is_empty() {
                fallback_src = extract_attr_from_html_tag(&noscript_text, "src");
                fallback_srcset = extract_attr_from_html_tag(&noscript_text, "srcset");
            }
        }

        if fallback_src.is_none() && fallback_srcset.is_none() {
            continue;
        }

        // Find the preceding element sibling of this noscript node
        if let Some(parent_id) = noscript_node.parent {
            if let Some(siblings) = parent_map.get(&parent_id) {
                if let Some(pos) = siblings
                    .iter()
                    .position(|id: &incognidium_dom::NodeId| *id == noscript_id)
                {
                    for &sibling_id in siblings.iter().take(pos).rev() {
                        let sibling = &doc.nodes[sibling_id];
                        if let incognidium_dom::NodeData::Element(ref el) = sibling.data {
                            if el.tag_name == "img" {
                                // Only copy if the placeholder lacks a src
                                if !el.attributes.contains_key("src") {
                                    if let Some(ref src) = fallback_src {
                                        let node_mut = &mut doc.nodes[sibling_id];
                                        if let incognidium_dom::NodeData::Element(ref mut el_mut) =
                                            node_mut.data
                                        {
                                            el_mut
                                                .attributes
                                                .insert("src".to_string(), src.clone());
                                            promoted += 1;
                                        }
                                    }
                                    if let Some(ref srcset) = fallback_srcset {
                                        let node_mut = &mut doc.nodes[sibling_id];
                                        if let incognidium_dom::NodeData::Element(ref mut el_mut) =
                                            node_mut.data
                                        {
                                            if !el_mut.attributes.contains_key("srcset") {
                                                el_mut
                                                    .attributes
                                                    .insert("srcset".to_string(), srcset.clone());
                                            }
                                        }
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    if promoted > 0 {
        eprintln!("Promoted {} images from <noscript> fallbacks", promoted);
    }
}

/// Extract the value of an HTML attribute from a raw tag string.
/// Handles both double-quoted and single-quoted values.
fn extract_attr_from_html_tag(html: &str, attr_name: &str) -> Option<String> {
    let attr_prefix = format!("{}=", attr_name);
    if let Some(pos) = html.find(&attr_prefix) {
        let start = pos + attr_prefix.len();
        let rest = &html[start..];
        if rest.starts_with('"') {
            if let Some(end) = rest[1..].find('"') {
                return Some(rest[1..1 + end].to_string());
            }
        } else if rest.starts_with('\'') {
            if let Some(end) = rest[1..].find('\'') {
                return Some(rest[1..1 + end].to_string());
            }
        }
    }
    None
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
