/// Render a URL to a PNG file for debugging
use std::collections::HashMap;

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::{fetch_bytes, fetch_url, resolve_url};
use incognidium_paint::{paint_with_images_and_canvas, ImageData};
use incognidium_style::resolve_styles;

use incognidium_shell::{
    collect_scripts, execute_scripts_on_doc, fetch_background_images, propagate_canvas_background,
    rasterize_inline_svgs, resolve_styles_with_container_sizes,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "https://example.com".into());
    let output = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "/tmp/incognidium_render.png".into());
    // Optional: --text <path> to dump extracted text
    let text_output = args
        .iter()
        .position(|a| a == "--text")
        .and_then(|i| args.get(i + 1).cloned());
    // Optional: --dump-boxes <path> to dump the flat box list for debugging
    let dump_boxes_path = args
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
    // Optional: --max-height <px> to cap the output PNG height (default 2000)
    let max_height: u32 = args
        .iter()
        .position(|a| a == "--max-height")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    // Optional: --no-js to skip script execution (matches Firefox no-JS comparison)
    let no_js = args.iter().any(|a| a == "--no-js");

    eprintln!("Fetching {url}...");
    let resp = fetch_url(&url).expect("fetch failed");
    eprintln!("Got {} bytes of HTML", resp.body.len());

    let doc = parse_html(&resp.body);
    eprintln!("DOM: {} nodes", doc.nodes.len());

    // Collect scripts (inline + external)
    let scripts = collect_scripts(&doc, &url);
    eprintln!("Scripts: {} found", scripts.len());

    // Execute scripts and get modified DOM
    let mut image_cache: HashMap<String, ImageData> = HashMap::new();
    let mut doc = if !no_js && !scripts.is_empty() {
        let modified_doc = execute_scripts_on_doc(doc, &scripts, &mut image_cache, &url);
        eprintln!(
            "JS executed, modified DOM: {} nodes",
            modified_doc.nodes.len()
        );
        modified_doc
    } else {
        if no_js && !scripts.is_empty() {
            eprintln!("Skipping {} script(s) due to --no-js", scripts.len());
        }
        doc
    };

    // Fetch images from the page
    let fetched_images = fetch_page_images(&doc, &url);
    eprintln!("Images: {} fetched", fetched_images.len());
    for (src, data) in &fetched_images {
        image_cache.insert(src.clone(), data.clone());
    }

    // Fetch external CSS from <link rel="stylesheet"> tags
    let mut css_text = fetch_external_css(&doc, &url);
    eprintln!("CSS: {} bytes from external stylesheets", css_text.len());

    // Add <style> block CSS from the (possibly modified) DOM
    let style_css = doc.collect_style_text();
    eprintln!("CSS: {} bytes from <style> blocks", style_css.len());
    css_text.push_str(&style_css);

    // Force light mode by dropping dark color-scheme media queries.
    css_text = incognidium_shell::strip_dark_mode_media_queries(&css_text);

    let stylesheet = parse_css(&css_text);
    eprintln!("Parsed {} CSS rules", stylesheet.rules.len());
    let viewport_width = 1024.0f32;
    // The layout pass uses a tall canvas so we can measure the full document
    // height, but `vh`/viewport-percentage lengths must resolve against the
    // real output viewport (max_height) to match a real browser window.
    let layout_height = 20000.0f32;
    let style_viewport_height = max_height as f32;
    let mut styles = resolve_styles(&doc, &stylesheet, viewport_width, style_viewport_height);

    let mut visible = 0usize;
    let mut hidden = 0usize;
    for (_nid, st) in &styles {
        if st.display == incognidium_style::Display::None {
            hidden += 1;
        } else {
            visible += 1;
        }
    }
    eprintln!("Styles: {visible} visible, {hidden} hidden");

    // Rasterize inline SVGs after styles are resolved so `currentColor` maps
    // to the computed element color and the layout engine sees them as
    // replaced `<img>` elements with explicit dimensions.
    rasterize_inline_svgs(
        &mut doc,
        &mut image_cache,
        Some(&mut styles),
        viewport_width,
        style_viewport_height,
        Some(&url),
    );

    // Build image sizes map for layout
    let mut image_sizes = ImageSizes::new();
    for (src, img) in &image_cache {
        image_sizes.insert(src.clone(), (img.width, img.height));
    }

    // First layout pass produces real container sizes so that
    // `@container` queries can be evaluated in the second style pass.
    let layout_root =
        layout_with_images(&doc, &styles, viewport_width, layout_height, &image_sizes);
    styles = resolve_styles_with_container_sizes(
        &doc,
        &stylesheet,
        viewport_width,
        style_viewport_height,
        &layout_root,
        &styles,
    );
    let layout_root =
        layout_with_images(&doc, &styles, viewport_width, layout_height, &image_sizes);

    // Resolve and fetch CSS background images (sprites, icons, wordmarks) so
    // the paint pass can render them. This must happen after the final style
    // pass because those URLs only exist in computed styles.
    for (src, img) in fetch_background_images(&styles, &url) {
        image_cache.entry(src).or_insert(img);
    }

    let flat_boxes = flatten_layout(&layout_root, 0.0, 0.0, &styles);
    eprintln!("{} flat boxes", flat_boxes.len());

    // Count text boxes
    let text_boxes: Vec<_> = flat_boxes.iter().filter(|b| b.text.is_some()).collect();
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

    // Auto-size height to fit content (with 20px padding)
    let content_height = flat_boxes
        .iter()
        .map(|b| (b.y + b.height) as u32)
        .max()
        .unwrap_or(768)
        .max(200)
        + 20;
    let render_height = content_height.min(max_height);

    // Optional wait for JS rendering
    if wait_ms > 0 {
        eprintln!("Waiting {}ms for JS rendering...", wait_ms);
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));
    }

    let canvas_bg = propagate_canvas_background(&doc, &styles);
    let pixmap = paint_with_images_and_canvas(
        &flat_boxes,
        &styles,
        1024,
        render_height,
        &image_cache,
        canvas_bg,
    );
    pixmap.save_png(&output).expect("save png");
    eprintln!("Saved to {output} ({}x{})", 1024, render_height);

    // Extract and save text content
    let mut all_text: Vec<(f32, f32, String)> = Vec::new();
    for fbox in &flat_boxes {
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
    eprintln!("Extracted {} lines of text", lines.len());

    // Always print to stderr for piping
    if let Some(ref text_path) = text_output {
        std::fs::write(text_path, &extracted_text).expect("write text file");
        eprintln!("Text saved to {text_path}");
    }

    // Also print text to stdout (so it can be captured by the batch script)
    println!("{}", extracted_text);

    // Optional dump of the flat box list for debugging layout issues
    if let Some(ref path) = dump_boxes_path {
        dump_flat_boxes(path, &flat_boxes, &doc, &styles);
    }
}

fn dump_flat_boxes(
    path: &str,
    flat_boxes: &[incognidium_layout::FlatBox],
    doc: &incognidium_dom::Document,
    styles: &incognidium_style::StyleMap,
) {
    use std::io::Write;
    let mut f = std::fs::File::create(path).expect("create dump file");
    for b in flat_boxes {
        let node = doc.nodes.get(b.node_id);
        let (tag, class) = match node.map(|n| &n.data) {
            Some(incognidium_dom::NodeData::Element(el)) => (
                el.tag_name.clone(),
                el.get_attr("class").unwrap_or_default().to_string(),
            ),
            Some(incognidium_dom::NodeData::Text(_)) => ("#text".to_string(), String::new()),
            _ => ("#other".to_string(), String::new()),
        };
        let (bg, fg, style_info) = styles
            .get(&b.node_id)
            .map(|s| {
                let pos = format!("{:?}", s.position).to_lowercase();
                (
                    format!(
                        "#{:02x}{:02x}{:02x}",
                        s.background_color.r, s.background_color.g, s.background_color.b
                    ),
                    format!("#{:02x}{:02x}{:02x}", s.color.r, s.color.g, s.color.b),
                    format!(
                        "pos={} w={:?} h={:?} left={:?} ml={:?}",
                        pos, s.width, s.height, s.left, s.margin_left
                    ),
                )
            })
            .unwrap_or_else(|| ("-".to_string(), "-".to_string(), "-".to_string()));
        let preview = b
            .text
            .as_ref()
            .map(|t| t.chars().take(60).collect::<String>().replace('\n', " "))
            .unwrap_or_default();
        writeln!(
            f,
            "node={} tag={} class={} [{:.1},{:.1} {:.1}x{:.1}] bg={} fg={} {} img={} text={}",
            b.node_id,
            tag,
            class,
            b.x,
            b.y,
            b.width,
            b.height,
            bg,
            fg,
            style_info,
            b.image_src.as_deref().unwrap_or(""),
            preview
        )
        .expect("write dump");
    }
    eprintln!("Dumped {} flat boxes to {path}", flat_boxes.len());
}

/// Fetch CSS from <link rel="stylesheet"> tags.
fn fetch_external_css(doc: &incognidium_dom::Document, base_url: &str) -> String {
    const MAX_STYLESHEETS: usize = 10;
    // Real stylesheets are often 300-600KB (bundled resets, design tokens,
    // breakpoints). Capping them at 256KB silently dropped the base
    // stylesheet on several sites and left mobile-only rules visible on a
    // desktop viewport.
    const MAX_CSS_SIZE: usize = 1024 * 1024; // 1MB per stylesheet
    let mut css = String::new();
    let mut fetched = 0usize;

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
                    // Skip print-only stylesheets
                    if let Some(media) = el.get_attr("media") {
                        if media.eq_ignore_ascii_case("print") {
                            continue;
                        }
                    }
                    if let Some(href) = el.get_attr("href") {
                        let resolved = match resolve_url(base_url, href) {
                            Ok(u) => u,
                            Err(_) => continue,
                        };
                        match fetch_url(&resolved) {
                            Ok(resp) => {
                                if resp.body.len() <= MAX_CSS_SIZE {
                                    css.push_str(&resp.body);
                                    css.push('\n');
                                    fetched += 1;
                                } else {
                                    eprintln!(
                                        "Skipping stylesheet {}: {} bytes exceeds {} byte limit",
                                        resolved,
                                        resp.body.len(),
                                        MAX_CSS_SIZE
                                    );
                                }
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
        }
    }
    css
}

/// Fetch images from the page (blocking, with parallelism).
fn fetch_page_images(doc: &incognidium_dom::Document, base_url: &str) -> Vec<(String, ImageData)> {
    const MAX_IMAGES: usize = 30;
    let mut urls: Vec<(String, String)> = Vec::new();

    for node in &doc.nodes {
        if urls.len() >= MAX_IMAGES {
            break;
        }
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "img" {
                if let Some(src) = el.get_attr("src") {
                    if src.starts_with("data:") {
                        continue;
                    }
                    if src.contains(".svg") {
                        continue;
                    } // SVGs need special handling
                    if let Ok(resolved) = resolve_url(base_url, src) {
                        urls.push((src.to_string(), resolved));
                    }
                }
            }
        }
    }

    if urls.is_empty() {
        return vec![];
    }

    let mut results = Vec::new();

    // Fetch in parallel (chunks of 8)
    for chunk in urls.chunks(8) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|(src, resolved)| {
                let src = src.clone();
                let resolved = resolved.clone();
                std::thread::spawn(move || {
                    match fetch_bytes(&resolved) {
                        Ok(bytes) => {
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
