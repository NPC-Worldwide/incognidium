/// Render a URL to a PNG file for debugging
use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::fetch_url;
use incognidium_paint::paint;
use incognidium_style::resolve_styles;

fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| "https://en.wikipedia.org/wiki/Main_Page".into());
    let output = std::env::args().nth(2).unwrap_or_else(|| "/tmp/incognidium_render.png".into());

    eprintln!("Fetching {url}...");
    let resp = fetch_url(&url).expect("fetch failed");
    eprintln!("Got {} bytes of HTML", resp.body.len());

    let doc = parse_html(&resp.body);
    eprintln!("DOM: {} nodes", doc.nodes.len());

    // Parse <style> block CSS; skip external CSS (complex site CSS breaks simple renderer)
    let css_text = doc.collect_style_text();
    eprintln!("CSS: {} bytes from <style> blocks", css_text.len());

    let stylesheet = parse_css(&css_text);
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

    let image_sizes = ImageSizes::new();
    let layout_root = layout_with_images(&doc, &styles, 1024.0, 20000.0, &image_sizes);
    let flat_boxes = flatten_layout(&layout_root, 0.0, 0.0);
    eprintln!("{} flat boxes", flat_boxes.len());

    // Count text boxes
    let text_boxes: Vec<_> = flat_boxes.iter().filter(|b| b.text.is_some()).collect();
    eprintln!("{} text boxes", text_boxes.len());
    for tb in text_boxes.iter().take(20) {
        if let Some(ref t) = tb.text {
            let preview: String = t.chars().take(80).collect();
            eprintln!("  [{:.0},{:.0} {}x{}] \"{}\"", tb.x, tb.y, tb.width, tb.height, preview);
        }
    }

    let pixmap = paint(&flat_boxes, &styles, 1024, 3000);
    pixmap.save_png(&output).expect("save png");
    eprintln!("Saved to {output}");
}
