//! Shared logic for incognidium-shell and its binaries.

#[cfg(feature = "v8-engine")]
pub mod v8_dom;

#[cfg(feature = "boa-engine")]
pub mod boa_dom;

use std::collections::HashMap;
use std::path::Path;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::ImageEncoder;
use tiny_skia::Pixmap;

use incognidium_dom::{Document, NodeData};
use incognidium_net::{fetch_url, resolve_url};
use incognidium_paint::ImageData;
use incognidium_style::{CssColor, StyleMap};

/// A script to execute, with its source code and a label for error messages.
pub struct ScriptEntry {
    pub source: String,
    pub origin: String,
}

/// Collect scripts from the DOM in document order, handling both inline and
/// external `<script src="...">` tags.
///
/// - Skips `type="module"` scripts (ES modules not supported)
/// - Limits external script fetches to 20
/// - Maintains document order for execution
pub fn collect_scripts(doc: &incognidium_dom::Document, base_url: &str) -> Vec<ScriptEntry> {
    const MAX_EXTERNAL_SCRIPTS: usize = 20;
    // Domains that provide ads, tracking, or consent widgets. Skipping them
    // cuts network/JS overhead on heavy news/commerce sites without affecting
    // primary content.
    const BLOCKED_SCRIPT_HOSTS: [&str;
        24
    ] = [
        "google-analytics.com",
        "googletagmanager.com",
        "googletagservices.com",
        "googlesyndication.com",
        "googleadservices.com",
        "doubleclick.net",
        "doubleverify.com",
        "amazon-adsystem.com",
        "adsystem.amazon.com",
        "facebook.net",
        "connect.facebook.net",
        "platform.twitter.com",
        "twitter.com",
        "ads-twitter.com",
        "cookielaw.org",
        "onetrust.com",
        "newrelic.com",
        "js-agent.newrelic.com",
        "adsafeprotected.com",
        "moatads.com",
        "outbrain.com",
        "taboola.com",
        "scorecardresearch.com",
        "quantserve.com",
    ];

    fn host_matches_any(url: &str, hosts: &[&str]) -> bool {
        url.parse::<url::Url>()
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
            .map(|h| hosts.iter().any(|blocked| h.ends_with(blocked)))
            .unwrap_or(false)
    }

    let mut scripts = Vec::new();
    let mut external_count = 0usize;

    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "script" {
                // Skip non-executable script types
                if let Some(script_type) = el.get_attr("type") {
                    let st = script_type.to_lowercase();
                    if st == "module"
                        || st == "application/json"
                        || st == "application/ld+json"
                        || st == "text/template"
                        || st == "text/html"
                        || st == "importmap"
                        || st == "speculationrules"
                    {
                        continue;
                    }
                }

                if let Some(src) = el.get_attr("src") {
                    // External script
                    if external_count >= MAX_EXTERNAL_SCRIPTS {
                        continue;
                    }
                    let resolved = match resolve_url(base_url, src) {
                        Ok(u) => u,
                        Err(e) => {
                            eprintln!("Failed to resolve script URL {src}: {e}");
                            continue;
                        }
                    };
                    if host_matches_any(&resolved, &BLOCKED_SCRIPT_HOSTS) {
                        eprintln!("Skipping blocked script {resolved}");
                        external_count += 1;
                        continue;
                    }
                    match fetch_url(&resolved) {
                        Ok(resp) => {
                            if !resp.body.is_empty() {
                                scripts.push(ScriptEntry {
                                    source: resp.body,
                                    origin: resolved,
                                });
                            }
                            external_count += 1;
                        }
                        Err(e) => {
                            eprintln!("Failed to fetch script {resolved}: {e}");
                            external_count += 1;
                        }
                    }
                } else {
                    // Inline script
                    let mut text = String::new();
                    for &child_id in &node.children {
                        if let incognidium_dom::NodeData::Text(ref t) = doc.nodes[child_id].data {
                            text.push_str(&t.content);
                        }
                    }
                    if !text.is_empty() {
                        scripts.push(ScriptEntry {
                            source: text,
                            origin: format!("inline <script> in {}", base_url),
                        });
                    }
                }
            }
        }
    }
    scripts
}

/// Execute scripts using whichever JS engine is enabled at build time.
/// With `v8-engine` (default): fast, runs real framework bundles.
/// With `boa-engine`: pure Rust, no Google code, slower.
/// Env `INCOGNIDIUM_JS=off` skips JS entirely.
pub fn execute_scripts_on_doc(
    doc: incognidium_dom::Document,
    scripts: &[ScriptEntry],
    _image_cache: &mut HashMap<String, ImageData>,
) -> incognidium_dom::Document {
    if std::env::var("INCOGNIDIUM_JS").ok().as_deref() == Some("off") {
        return doc;
    }
    #[cfg(feature = "v8-engine")]
    {
        v8_dom::execute_scripts_v8(doc, scripts)
    }
    #[cfg(all(feature = "boa-engine", not(feature = "v8-engine")))]
    {
        return boa_dom::execute_scripts_boa(doc, scripts);
    }
    #[cfg(not(any(feature = "v8-engine", feature = "boa-engine")))]
    {
        let _ = scripts;
        doc
    }
}

/// Strip dark mode styles from CSS text.
///
/// Sites like Wikipedia ship both light and dark variable sets. The dark set
/// can arrive inside `prefers-color-scheme: dark` media queries or in plain
/// rules keyed off a night theme class (e.g. `html.skin-theme-clientpref-night`).
/// Because our renderer does not report a real color-scheme preference, those
/// blocks can end up matching and turning the page black. Removing them leaves
/// the light/default styles intact.
pub fn strip_dark_mode_media_queries(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut i = 0usize;
    let bytes = css.as_bytes();
    let len = bytes.len();

    fn is_night_selector(sel: &str) -> bool {
        let lower = sel.to_ascii_lowercase();
        lower.contains("skin-theme-clientpref-night")
            || lower.contains("-night")
            && lower.contains("theme")
    }

    while i < len {
        // Find next @media
        if let Some(at_pos) = css[i..].find("@media") {
            let at_pos = i + at_pos;
            out.push_str(&css[i..at_pos]);

            // Find the opening brace
            let after_at = at_pos + 6;
            let open = css[after_at..].find('{').map(|p| after_at + p);
            if let Some(open_pos) = open {
                let prelude = css[after_at..open_pos].to_ascii_lowercase();
                // Check if this media query is for dark color scheme
                let is_dark_media = prelude.contains("prefers-color-scheme")
                    && prelude.contains("dark");
                if is_dark_media {
                    // Skip this block by brace counting
                    let mut depth = 1usize;
                    let mut j = open_pos + 1;
                    while j < len && depth > 0 {
                        match bytes[j] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    i = j;
                } else {
                    // Keep the block
                    out.push_str(&css[at_pos..=open_pos]);
                    i = open_pos + 1;
                }
            } else {
                // Malformed: no opening brace; keep rest
                out.push_str(&css[at_pos..]);
                break;
            }
        } else {
            out.push_str(&css[i..]);
            break;
        }
    }

    // Second pass: strip top-level rules whose selector is a night theme class.
    // This handles `html.skin-theme-clientpref-night { ... }` and similar.
    let mut final_out = String::with_capacity(out.len());
    let mut k = 0usize;
    while k < out.len() {
        // Find next top-level opening brace. We look for the first '{' after k
        // that is not inside a string/comment is sufficient for this stripper.
        if let Some(open_pos) = out[k..].find('{') {
            let open_pos = k + open_pos;
            // Walk back to find the start of the selector
            let sel_start = {
                let mut s = open_pos;
                while s > k {
                    if out.as_bytes()[s - 1] == b'}' || out.as_bytes()[s - 1] == b'{' {
                        break;
                    }
                    s -= 1;
                }
                s
            };
            let selector = &out[sel_start..open_pos];
            if is_night_selector(selector) {
                // Skip this rule
                let mut depth = 1usize;
                let mut j = open_pos + 1;
                while j < out.len() && depth > 0 {
                    match out.as_bytes()[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                k = j;
            } else {
                final_out.push_str(&out[k..=open_pos]);
                k = open_pos + 1;
            }
        } else {
            final_out.push_str(&out[k..]);
            break;
        }
    }

    final_out
}

/// Trim excess items inside Brightspot list modules.
///
/// Brightspot pages (e.g. AP News) render the full list of articles in the
/// server HTML, then rely on JS/custom elements to hide everything past
/// `data-max-number-of-posts` and show a "Load more" button. Our headless
/// renderer does not run that behavior, so all items get laid out and inflate
/// the page height. This helper limits any list module that declares
/// `data-max-number-of-posts` to the number of posts it declares, matching the
/// visible state in a real browser.
///
/// Only children of `.PageList-items` with class `.PageList-items-item` are
/// trimmed; non-item siblings such as the load-more button are preserved.
pub fn trim_bsp_list_loadmore(doc: &mut Document) {
    fn find_items_container(
        doc: &Document,
        root: incognidium_dom::NodeId,
    ) -> Option<incognidium_dom::NodeId> {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let node = &doc.nodes[id];
            if let NodeData::Element(el) = &node.data {
                if el.classes().contains(&"PageList-items") {
                    return Some(id);
                }
            }
            stack.extend(node.children.iter().copied());
        }
        None
    }

    fn trim_node(doc: &mut Document, id: incognidium_dom::NodeId) {
        let maybe_max: Option<usize> = {
            let node = &doc.nodes[id];
            if let NodeData::Element(el) = &node.data {
                el.get_attr("data-max-number-of-posts")
                    .and_then(|s| s.parse().ok())
            } else {
                None
            }
        };

        if let Some(max_items) = maybe_max {
            if let Some(items_id) = find_items_container(doc, id) {
                let items_node = &doc.nodes[items_id];
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = items_node
                    .children
                    .iter()
                    .filter(|&&cid| {
                        if let NodeData::Element(el) = &doc.nodes[cid].data {
                            if el.classes().contains(&"PageList-items-item") {
                                kept += 1;
                                kept > max_items
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    let set: std::collections::HashSet<incognidium_dom::NodeId> =
                        to_remove.iter().copied().collect();
                    let items_node = &mut doc.nodes[items_id];
                    items_node.children.retain(|cid| !set.contains(cid));
                }
            }
        }

        let child_ids: Vec<incognidium_dom::NodeId> = doc.nodes[id].children.clone();
        for cid in child_ids {
            trim_node(doc, cid);
        }
    }

    trim_node(doc, 0);
}

/// Remove empty placeholder containers that real browsers hide or fill with ads.
///
/// Many news/commerce pages include ad slots, tracking widgets, and CMS
/// placeholder boxes (e.g. `markupbox`, `ad-slot`, `dfp-ad`, `adsbygoogle`,
/// `taboola`, `outbrain`) in the server HTML. Without the site's ad/tracking JS
/// these boxes have no visible content, but they still occupy CSS-generated
/// height (padding, min-height, margins). This helper drops any such subtree
/// that contains no real content: no text, no images, no media, no form controls,
/// and no meaningful accessibility text. Visible placeholders (e.g. a footer
/// logo inside a `markupbox`) are preserved.
///
/// It also removes subtrees marked `aria-hidden="true"` when they have no
/// visible content, which is common for off-screen/hidden ad slots.
pub fn remove_empty_placeholders(doc: &mut Document) {
    fn has_visible_content(doc: &Document, id: incognidium_dom::NodeId) -> bool {
        let node = &doc.nodes[id];
        match &node.data {
            incognidium_dom::NodeData::Text(t) => !t.trim().is_empty(),
            incognidium_dom::NodeData::Element(el) => {
                if matches!(
                    el.tag_name.as_str(),
                    "img" | "picture" | "video" | "audio" | "svg" | "canvas" | "iframe"
                        | "object" | "embed" | "input" | "textarea" | "select" | "button"
                ) {
                    return true;
                }
                for attr in ["alt", "aria-label", "title", "placeholder"] {
                    if let Some(v) = el.get_attr(attr) {
                        if !v.trim().is_empty() {
                            return true;
                        }
                    }
                }
                node.children
                    .iter()
                    .any(|&cid| has_visible_content(doc, cid))
            }
            _ => false,
        }
    }

    fn is_placeholder(el: &incognidium_dom::ElementData) -> bool {
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        const PLACEHOLDER_CLASSES: [&str; 11] = [
            "markupbox",
            "ad",
            "ads",
            "ad-slot",
            "ad__placeholder",
            "ad-placeholder",
            "ad-container",
            "dfp-ad",
            "adsbygoogle",
            "taboola",
            "outbrain",
        ];
        if classes.iter().any(|c| PLACEHOLDER_CLASSES.contains(c)) {
            return true;
        }
        if let Some(v) = el.get_attr("aria-hidden") {
            if v == "true" {
                return true;
            }
        }
        false
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            if is_placeholder(el) && !has_visible_content(doc, id) {
                to_remove.push(id);
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    let remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();

    for id in to_remove {
        if let Some(parent_id) = doc.nodes[id].parent {
            let parent = &mut doc.nodes[parent_id];
            parent.children.retain(|cid| !remove_set.contains(cid));
        }
    }
}

/// Maximum pixel dimension for rasterized inline SVGs. Icons should stay small;
/// large decorative SVGs are downscaled to keep memory and paint costs sane.
const MAX_INLINE_SVG_DIM: f32 = 512.0;
const MAX_INLINE_SVGS: usize = 30;

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn serialize_svg_subtree(doc: &Document, node_id: incognidium_dom::NodeId, out: &mut String) {
    let node = &doc.nodes[node_id];
    match &node.data {
        NodeData::Element(el) => {
            out.push('<');
            out.push_str(&el.tag_name);
            for (k, v) in &el.attributes {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                out.push_str(&escape_xml_attr(v));
                out.push('"');
            }
            if node.children.is_empty() {
                // SVG elements like <line>, <path>, <circle> are typically empty in source;
                // write them as self-closing for compact XML.
                out.push_str("/>");
            } else {
                out.push('>');
                for &child_id in &node.children {
                    serialize_svg_subtree(doc, child_id, out);
                }
                out.push_str("</");
                out.push_str(&el.tag_name);
                out.push('>');
            }
        }
        NodeData::Text(t) => {
            out.push_str(&escape_xml_text(&t.content));
        }
        NodeData::Comment(c) => {
            out.push_str("<!--");
            out.push_str(c);
            out.push_str("-->");
        }
        _ => {}
    }
}

fn css_color_to_svg(color: CssColor) -> String {
    if color.a == 255 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        let a = color.a as f32 / 255.0;
        format!("rgba({}, {}, {}, {})", color.r, color.g, color.b, a)
    }
}

fn render_svg_xml(svg: &str, current_color: CssColor) -> Option<ImageData> {
    // Inline SVGs frequently use `currentColor` for strokes/fills so they match
    // the surrounding text color. usvg alone cannot resolve CSS `currentColor`,
    // so substitute the computed (or default) color before rasterizing.
    let color_str = css_color_to_svg(current_color);
    let svg = svg.replace("currentColor", &color_str);
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&svg, &opt).ok()?;
    let size = tree.size();
    let intrinsic_w = size.width();
    let intrinsic_h = size.height();
    if intrinsic_w <= 0.0 || intrinsic_h <= 0.0 {
        return None;
    }
    let scale = if intrinsic_w > MAX_INLINE_SVG_DIM || intrinsic_h > MAX_INLINE_SVG_DIM {
        MAX_INLINE_SVG_DIM / intrinsic_w.max(intrinsic_h)
    } else {
        1.0
    };
    let w = (intrinsic_w * scale).ceil().max(1.0) as u32;
    let h = (intrinsic_h * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia uses premultiplied BGRA; convert to straight RGBA.
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in pixmap.pixels() {
        let a = px.alpha();
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
    Some(ImageData {
        pixels: out,
        width: w,
        height: h,
    })
}

/// Rasterize inline `<svg>` elements and turn them into `<img>` placeholders
/// that reference the raster in `image_cache`. This lets the existing layout
/// and paint pipelines render icon menus, logos, and other simple inline SVGs
/// without needing a full SVG layout implementation.
pub fn rasterize_inline_svgs(
    doc: &mut Document,
    image_cache: &mut HashMap<String, ImageData>,
    styles: Option<&StyleMap>,
) {
    let svg_ids: Vec<incognidium_dom::NodeId> = doc
        .nodes
        .iter()
        .filter_map(|n| {
            if let NodeData::Element(el) = &n.data {
                if el.tag_name == "svg" {
                    return Some(n.id);
                }
            }
            None
        })
        .collect();

    let mut count = 0usize;
    for id in svg_ids {
        if count >= MAX_INLINE_SVGS {
            break;
        }
        let mut svg_xml = String::new();
        serialize_svg_subtree(doc, id, &mut svg_xml);
        if svg_xml.is_empty() {
            continue;
        }
        let current_color = styles
            .and_then(|s| s.get(&id))
            .map(|s| s.color)
            .unwrap_or(CssColor::BLACK);
        let Some(img) = render_svg_xml(&svg_xml, current_color) else {
            continue;
        };
        let key = format!("inline-svg:{id}");

        // Detach SVG children first so we can safely mutate the node data next.
        let children = std::mem::take(&mut doc.nodes[id].children);
        for child in children {
            doc.nodes[child].parent = None;
        }

        if let NodeData::Element(ref mut el) = doc.nodes[id].data {
            // Preserve author-specified dimensions if present; otherwise use
            // the raster's natural size.
            let width_px = el
                .get_attr("width")
                .and_then(|w| w.trim_end_matches("px").parse::<f32>().ok())
                .unwrap_or(img.width as f32)
                .round()
                .max(1.0) as u32;
            let height_px = el
                .get_attr("height")
                .and_then(|h| h.trim_end_matches("px").parse::<f32>().ok())
                .unwrap_or(img.height as f32)
                .round()
                .max(1.0) as u32;

            el.tag_name = "img".to_string();
            el.attributes.insert("src".to_string(), key.clone());
            el.attributes.insert("width".to_string(), width_px.to_string());
            el.attributes.insert("height".to_string(), height_px.to_string());
            // Alt text so the placeholder is accessible and visible even if
            // the raster is not in cache.
            if !el.attributes.contains_key("alt") {
                let alt = el.get_attr("aria-label").unwrap_or("").to_string();
                el.attributes.insert("alt".to_string(), alt);
            }
            image_cache.insert(key, img);
            count += 1;
        }
    }
}

/// Predicate to skip synthetic inline-SVG URLs during normal image fetching.
pub fn is_inline_svg_url(url: &str) -> bool {
    url.starts_with("inline-svg:")
}

/// Encode a tiny-skia pixmap as a PNG using best compression. Smaller than
/// `pixmap.save_png()`/`encode_png()` while remaining lossless, which matters
/// for very long news/article pages in the QA pipeline.
pub fn encode_png_compressed(pixmap: &Pixmap, writer: impl std::io::Write) -> Result<(), Box<dyn std::error::Error>> {
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);
    encoder.write_image(
        pixmap.data(),
        pixmap.width(),
        pixmap.height(),
        image::ColorType::Rgba8.into(),
    )?;
    Ok(())
}

/// Convenience wrapper that writes a compressed PNG to disk.
pub fn save_png_compressed(pixmap: &Pixmap, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    encode_png_compressed(pixmap, file)
}
