/// Render a URL to a PNG file for debugging
use std::collections::HashMap;

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::{fetch_url, fetch_bytes, resolve_url};
use incognidium_paint::{paint_with_images, ImageData};
use incognidium_style::resolve_styles;

use incognidium_shell::{collect_scripts, execute_scripts_on_doc};

fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| "https://en.wikipedia.org/wiki/Main_Page".into());
    let output = std::env::args().nth(2).unwrap_or_else(|| "/tmp/incognidium_render.png".into());

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
    let doc = if !scripts.is_empty() {
        let modified_doc = execute_scripts_on_doc(doc, &scripts, &mut image_cache);
        eprintln!("JS executed, modified DOM: {} nodes", modified_doc.nodes.len());
        modified_doc
    } else {
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

    let stylesheet = parse_css(&css_text);
    eprintln!("Parsed {} CSS rules", stylesheet.rules.len());
    let styles = resolve_styles(&doc, &stylesheet);

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

    // Build image sizes map for layout
    let mut image_sizes = ImageSizes::new();
    for (src, img) in &image_cache {
        image_sizes.insert(src.clone(), (img.width, img.height));
    }

    let layout_root = layout_with_images(&doc, &styles, 1024.0, 20000.0, &image_sizes);
    let flat_boxes = flatten_layout(&layout_root, 0.0, 0.0);
    eprintln!("{} flat boxes", flat_boxes.len());

    // Count text boxes
    let text_boxes: Vec<_> = flat_boxes.iter().filter(|b| b.text.is_some()).collect();
    eprintln!("{} text boxes", text_boxes.len());
    for tb in text_boxes.iter().take(10) {
        if let Some(ref t) = tb.text {
            let preview: String = t.chars().take(80).collect();
            eprintln!("  [{:.0},{:.0} {}x{}] \"{}\"", tb.x, tb.y, tb.width, tb.height, preview);
        }
    }

    // Auto-size height to fit content (with 20px padding)
    let content_height = flat_boxes.iter()
        .map(|b| (b.y + b.height) as u32)
        .max()
        .unwrap_or(768)
        .max(200) + 20;
    let render_height = content_height.min(2000); // cap at ~2 screenfuls

    let pixmap = paint_with_images(&flat_boxes, &styles, 1024, render_height, &image_cache);
    pixmap.save_png(&output).expect("save png");
    eprintln!("Saved to {output} ({}x{})", 1024, render_height);
}

/// Fetch CSS from <link rel="stylesheet"> tags.
fn fetch_external_css(doc: &incognidium_dom::Document, base_url: &str) -> String {
    const MAX_STYLESHEETS: usize = 10;
    const MAX_CSS_SIZE: usize = 256 * 1024; // 256KB per stylesheet
    let mut css = String::new();
    let mut fetched = 0usize;

    for node in &doc.nodes {
        if fetched >= MAX_STYLESHEETS { break; }
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "link" {
                let is_stylesheet = el.get_attr("rel")
                    .map(|r| r.eq_ignore_ascii_case("stylesheet"))
                    .unwrap_or(false);
                if is_stylesheet {
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
        if urls.len() >= MAX_IMAGES { break; }
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "img" {
                if let Some(src) = el.get_attr("src") {
                    if src.starts_with("data:") { continue; }
                    if src.contains(".svg") { continue; } // SVGs need special handling
                    if let Ok(resolved) = resolve_url(base_url, src) {
                        urls.push((src.to_string(), resolved));
                    }
                }
            }
        }
    }

    if urls.is_empty() { return vec![]; }

    let mut results = Vec::new();

    // Fetch in parallel (chunks of 8)
    for chunk in urls.chunks(8) {
        let handles: Vec<_> = chunk.iter().map(|(src, resolved)| {
            let src = src.clone();
            let resolved = resolved.clone();
            std::thread::spawn(move || {
                match fetch_bytes(&resolved) {
                    Ok(bytes) => {
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            return Some((src, ImageData {
                                pixels: rgba.into_raw(),
                                width: w,
                                height: h,
                            }));
                        }
                    }
                    Err(_) => {}
                }
                None
            })
        }).collect();

        for handle in handles {
            if let Ok(Some(result)) = handle.join() {
                results.push(result);
            }
        }
    }

    results
}
