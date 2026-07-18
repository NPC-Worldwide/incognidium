/// Render a URL to a PNG file for debugging
use std::collections::HashMap;

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::{fetch_bytes, fetch_url, resolve_url};
use incognidium_paint::{paint_with_images, ImageData};
use incognidium_style::resolve_styles;

use incognidium_shell::{collect_scripts, execute_scripts_on_doc};

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
    // Optional: --wait <ms> to wait for JS rendering
    let wait_ms: u64 = args
        .iter()
        .position(|a| a == "--wait")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Optional: --no-js to skip JavaScript execution. Useful when JS engines
    // crash on a site and the server-rendered HTML is sufficient.
    let no_js = args.iter().any(|a| a == "--no-js")
        || std::env::var("INCOGNIDIUM_DISABLE_JS").is_ok();

    // Check if input is a file path (starts with / or . or ends with .html)
    let is_file = input.starts_with('/') || input.starts_with('.') || input.ends_with(".html");

    let (body, base_url) = if is_file {
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
        (resp.body, input)
    };
    eprintln!("Got {} bytes of HTML", body.len());

    let doc = parse_html(&body);
    eprintln!("DOM: {} nodes", doc.nodes.len());

    // Collect scripts (inline + external)
    let scripts = collect_scripts(&doc, &base_url);
    eprintln!("Scripts: {} found", scripts.len());
    if no_js {
        eprintln!("JS execution disabled by --no-js / INCOGNIDIUM_DISABLE_JS");
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
        // Give the JS thread a generous stack; modern bundles and V8 can
        // recurse deeply and overflow the default 2 MB Rust thread stack.
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
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
                modified_doc
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

    // Fetch images from the page
    let fetched_images = fetch_page_images(&doc, &base_url);
    eprintln!("Images: {} fetched", fetched_images.len());
    for (src, data) in &fetched_images {
        image_cache.insert(src.clone(), data.clone());
    }

    // Fetch external CSS from <link rel="stylesheet"> tags
    let mut css_text = fetch_external_css(&doc, &base_url);
    eprintln!("CSS: {} bytes from external stylesheets", css_text.len());

    // Add <style> block CSS from the (possibly modified) DOM
    eprintln!("About to collect style text");
    let style_css = doc.collect_style_text();
    eprintln!("CSS: {} bytes from <style> blocks", style_css.len());
    css_text.push_str(&style_css);

    // Extract data URI images from CSS background-image properties
    // This needs to happen before parsing CSS so they're in the image cache
    eprintln!("About to extract CSS data URI images from {} bytes", css_text.len());
    let css_data_uri_images = extract_css_data_uri_images(&css_text);
    eprintln!(
        "CSS Images: {} data URIs extracted",
        css_data_uri_images.len()
    );
    for (src, data) in css_data_uri_images {
        image_cache.insert(src, data);
    }

    // Scale fonts for PNG readability (24px base)
    css_text.push_str("\n:root { font-size: 24px !important; }\n");
    css_text.push_str("body { font-size: 24px !important; }\n");

    if let Some(ref css_path) = css_output {
        std::fs::write(css_path, &css_text).expect("write css dump");
        eprintln!("Combined CSS dumped to {css_path}");
    }

    let mut stylesheet = parse_css(&css_text);

    let mut styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

    // Some sites (e.g. Politico) serve HTML with `body { visibility: hidden !important; }`
    // as an anti-bot measure. Counter it with a higher-specificity rule so text
    // extraction and painting can see the real content.
    if let Some(body_id) = doc.body() {
        if let Some(style) = styles.get(&body_id) {
            if !matches!(style.visibility, incognidium_style::Visibility::Visible) {
                eprintln!(
                    "Detected body visibility={:?}; injecting visible override",
                    style.visibility
                );
                css_text.push_str("\nhtml body { visibility: visible !important; }\n");
                stylesheet = parse_css(&css_text);
                styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
            }
        }
    }

    // Build image sizes map for layout
    let mut image_sizes = ImageSizes::new();
    for (src, img) in &image_cache {
        image_sizes.insert(src.clone(), (img.width, img.height));
    }

    let layout_root = layout_with_images(&doc, &styles, 1024.0, 20000.0, &image_sizes);

    let flat_boxes = flatten_layout(&layout_root, 0.0, 0.0, &styles);
    eprintln!("{} flat boxes", flat_boxes.len());

    // Debug: print all flat boxes when very few are produced (layout collapse diagnosis)
    if flat_boxes.len() <= 5 || std::env::var("DUMP_BOXES").is_ok() {
        eprintln!("All flat boxes:");
        for fb in &flat_boxes {
            let preview = fb.text.as_deref().unwrap_or("(no text)");
            let (tag, cls) = match &doc.nodes[fb.node_id].data {
                incognidium_dom::NodeData::Element(ref e) => {
                    (e.tag_name.clone(), e.get_attr("class").unwrap_or("").to_string())
                }
                _ => (String::from("#text"), String::new()),
            };
            eprintln!(
                "  [{:.0},{:.0} {}x{}] type={:?} tag={} class={} text={}",
                fb.x,
                fb.y,
                fb.width,
                fb.height,
                fb.box_type,
                tag,
                &cls[..cls.len().min(60)],
                preview.chars().take(60).collect::<String>()
            );
        }
    }

    // Count text boxes (exclude images - alt text should not render)
    let text_boxes: Vec<_> = flat_boxes
        .iter()
        .filter(|b| b.text.is_some() && b.box_type != incognidium_layout::BoxType::Image)
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

    // Size height to fit content — full page capture, no cap
    let render_height = flat_boxes
        .iter()
        .map(|b| (b.y + b.height) as u32)
        .max()
        .unwrap_or(768)
        .max(200)
        + 20;

    // Optional wait for JS rendering
    if wait_ms > 0 {
        eprintln!("Waiting {}ms for JS rendering...", wait_ms);
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));
    }

    let pixmap = paint_with_images(&flat_boxes, &styles, 1024, render_height, &image_cache);
    pixmap.save_png(&output).expect("save png");
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
        if let Some(ref t) = fbox.text {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                all_text.push((fbox.y, fbox.x, trimmed.to_string()));
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

    let extracted_text = lines.join("\n");
    eprintln!(
        "Extracted {} lines of text ({} text fragments)",
        lines.len(),
        all_text.len()
    );

    // Always print to stderr for piping
    if let Some(ref text_path) = text_output {
        std::fs::write(text_path, &extracted_text).expect("write text file");
        eprintln!("Text saved to {text_path}");
    }

    // Also print text to stdout (so it can be captured by the batch script)
    println!("{}", extracted_text);
}

/// Fetch CSS from <link rel="stylesheet"> tags and follow @import rules.
fn fetch_external_css(doc: &incognidium_dom::Document, base_url: &str) -> String {
    const MAX_STYLESHEETS: usize = 20;
    const MAX_CSS_SIZE: usize = 4 * 1024 * 1024; // 4MB per stylesheet
    let mut css = String::new();
    let mut fetched = 0usize;
    let mut to_fetch: Vec<String> = Vec::new();

    // First collect all <link> stylesheets
    for node in &doc.nodes {
        if fetched >= MAX_STYLESHEETS {
            break;
        }
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "link" {
                let is_stylesheet = el
                    .get_attr("rel")
                    .map(|r| r.eq_ignore_ascii_case("stylesheet"))
                    .unwrap_or(false);
                if is_stylesheet {
                    // Skip print-only stylesheets unless the link has an onload
                    // handler that will flip the media to "all" (common perf pattern:
                    // <link rel="stylesheet" href="..." media="print" onload="this.media='all'">).
                    let mut skip_print = true;
                    if let Some(media) = el.get_attr("media") {
                        if media.eq_ignore_ascii_case("print") {
                            if let Some(onload) = el.get_attr("onload") {
                                let lower = onload.to_lowercase();
                                if lower.contains("this.media") && lower.contains("'all'") {
                                    skip_print = false;
                                }
                            }
                            if skip_print {
                                continue;
                            }
                        }
                    }
                    if let Some(href) = el.get_attr("href") {
                        if let Ok(resolved) = resolve_url(base_url, href) {
                            to_fetch.push(resolved);
                        }
                    }
                }
            }
        }
    }

    // Fetch stylesheets and follow @import rules
    let mut fetched_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(url) = to_fetch.pop() {
        if fetched >= MAX_STYLESHEETS {
            break;
        }
        if fetched_urls.contains(&url) {
            continue;
        }
        fetched_urls.insert(url.clone());

        if let Ok(resp) = fetch_url(&url) {
            if resp.body.len() <= MAX_CSS_SIZE {
                // Extract @import rules and fetch them
                let imports = extract_imports(&resp.body);
                for import_url in imports {
                    if let Ok(resolved) = resolve_url(&url, &import_url) {
                        if !fetched_urls.contains(&resolved) {
                            to_fetch.push(resolved);
                        }
                    }
                }
                css.push_str(&resp.body);
                css.push('\n');
                fetched += 1;
            }
        }
    }
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
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(bytes, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    let w = size.width().ceil() as u32;
    let h = size.height().ceil() as u32;
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("bad svg dims".into());
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("pixmap")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
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

fn fetch_page_images(doc: &incognidium_dom::Document, base_url: &str) -> Vec<(String, ImageData)> {
    const MAX_IMAGES: usize = 100;
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

    // Fetch in parallel (chunks of 4, with small delay between chunks to avoid rate limits)
    for (ci, chunk) in urls.chunks(4).enumerate() {
        if ci > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let handles: Vec<_> = chunk
            .iter()
            .map(|(src, resolved)| {
                let src = src.clone();
                let resolved = resolved.clone();
                std::thread::spawn(move || {
                    if let Ok(bytes) = fetch_bytes(&resolved) {
                        if bytes.len() < 4000
                            && (bytes.starts_with(b"<!DOCTYPE")
                                || bytes.starts_with(b"<html")
                                || bytes.starts_with(b"<?xml"))
                        {
                            return None;
                        }
                        let is_svg = resolved.to_lowercase().ends_with(".svg")
                            || bytes.windows(4).take(512).any(|w| w == b"<svg");
                        if is_svg {
                            if let Ok(img) = decode_svg(&bytes) {
                                return Some((src, img));
                            }
                        }
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            return Some((
                                src,
                                ImageData {
                                    pixels: rgba.into_raw(),
                                    width: w,
                                    height: h,
                                },
                            ));
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
        "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input"
            | "link" | "meta" | "param" | "source" | "track" | "wbr"
    )
}
