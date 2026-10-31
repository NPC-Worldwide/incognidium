use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Icon, Window, WindowId};

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::{
    fetch_bytes, fetch_bytes_with_referer, fetch_url, fetch_url_fast, fetch_url_solving_challenges,
    resolve_url,
};
use incognidium_paint::{paint_with_images_and_canvas, ImageData};
use incognidium_style::resolve_styles;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use incognidium_devtools::{
    extract_links, extract_page_text, extract_title, DevToolsBridge, DevToolsCommand, NetworkEntry,
};

use incognidium_shell::{
    collect_scripts, decode_and_downscale_image, encode_png_compressed,
    propagate_canvas_background, resolve_styles_with_container_sizes, save_png_compressed,
};

const DEFAULT_WIDTH: u32 = 1400;
const DEFAULT_HEIGHT: u32 = 900;
const TOOLBAR_HEIGHT: u32 = 40;
const ADDR_BAR_LEFT: f32 = 90.0;
const ADDR_BAR_TOP: f32 = 6.0;
const ADDR_BAR_HEIGHT: f32 = 28.0;
const ADDR_BAR_RIGHT_MARGIN: f32 = 44.0;
const BTN_SIZE: f32 = 28.0;
const BTN_Y: f32 = 6.0;
const MENU_BTN_MARGIN: f32 = 8.0;

/// Load the embedded transparent logo and downscale it to a window-icon size.
fn load_window_icon() -> Option<Icon> {
    let bytes = include_bytes!("../../../assets/logo_transparent.png");
    let img = image::load_from_memory(bytes).ok()?;
    // Keep the icon small enough for X11's maximum request size (~256 KiB total).
    let img = img.resize(128, 128, image::imageops::FilterType::Lanczos3);
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), w, h)
        .map_err(|e| log::warn!("Failed to create window icon: {e:?}"))
        .ok()
}

struct App {
    // Current page
    html_content: String,
    current_url: String,

    // Navigation history
    history: Vec<String>,
    history_pos: usize, // index into history, points to current page

    // Address bar state
    address_text: String,
    address_focused: bool,
    cursor_pos: usize,

    // Window
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,

    // Scroll
    scroll_y: f32,

    // JS engine
    js_vm: murkiu_vm::Vm,

    // Mouse
    last_cursor: Option<(f64, f64)>,

    // Address bar selection state
    address_all_selected: bool,

    // Menu dropdown
    menu_open: bool,

    // Cached page images
    image_cache: std::collections::HashMap<String, ImageData>,

    // Cached flat boxes for click detection (links)
    flat_boxes: Vec<incognidium_layout::FlatBox>,

    // External CSS fetched from <link rel="stylesheet"> tags
    external_css: String,

    // Layout cache — avoids re-parsing on every scroll
    cached_layout: Option<CachedLayout>,
    layout_dirty: bool,
    last_layout_width: u32,

    // DevTools MCP bridge (None when not in --mcp mode)
    devtools: Option<Arc<DevToolsBridge>>,

    // Async image loading
    pending_images: Arc<Mutex<Vec<(String, ImageData)>>>,
    images_loading: Arc<AtomicBool>,

    // DOM document modified by JavaScript execution
    js_modified_doc: Option<incognidium_dom::Document>,

    // Screenshot requested via F12; saved during next render
    pending_screenshot: Option<String>,

    // Animation frame counter for toolbar spinners
    frame_count: u32,
}

/// Cached results from parse -> style -> layout pipeline.
struct CachedLayout {
    doc: incognidium_dom::Document,
    styles: incognidium_style::StyleMap,
    layout_root: Box<incognidium_layout::LayoutBox>,
}

impl App {
    fn new(initial_url: String, initial_html: String) -> Self {
        App {
            html_content: initial_html,
            current_url: initial_url.clone(),
            history: vec![initial_url.clone()],
            history_pos: 0,
            address_text: initial_url,
            address_focused: false,
            cursor_pos: 0,
            window: None,
            surface: None,
            scroll_y: 0.0,
            last_cursor: None,
            js_vm: murkiu_vm::Vm::new(),
            image_cache: std::collections::HashMap::new(),
            flat_boxes: Vec::new(),
            address_all_selected: false,
            menu_open: false,
            external_css: String::new(),
            cached_layout: None,
            layout_dirty: true,
            last_layout_width: 0,
            devtools: None,
            pending_images: Arc::new(Mutex::new(Vec::new())),
            images_loading: Arc::new(AtomicBool::new(false)),
            js_modified_doc: None,
            pending_screenshot: None,
            frame_count: 0,
        }
    }

    fn navigate(&mut self, url_input: &str) {
        let url_str = url_input.to_string();

        match fetch_url_solving_challenges(&url_str) {
            Ok(mut resp) => {
                // Follow a single <meta http-equiv="refresh"> redirect if the
                // initial response is a redirector page (e.g. a language runtime
                // download page).
                if let Some(target) = incognidium_shell::meta_refresh_target(&resp.body, &resp.url)
                {
                    eprintln!("Following meta refresh to {target}...");
                    if let Ok(redirect_resp) = fetch_url(&target) {
                        resp = redirect_resp;
                    } else {
                        eprintln!("Failed to follow meta refresh {target}");
                    }
                }
                if let Some(target) =
                    incognidium_shell::reddit_challenge_target(&resp.body, &resp.url)
                {
                    eprintln!("Solving Reddit challenge to {target}...");
                    if let Ok(challenge_resp) = fetch_url(&target) {
                        resp = challenge_resp;
                    } else {
                        eprintln!("Failed to solve Reddit challenge {target}");
                    }
                }
                self.log_network(
                    "GET",
                    &url_str,
                    Some(200),
                    &resp.body.len().to_string(),
                    resp.body.len(),
                    None,
                );
                self.html_content = resp.body.clone();
                self.current_url = resp.url.clone();
                self.address_text = resp.url.clone();
                self.cursor_pos = self.address_text.len();
                self.scroll_y = 0.0;

                // Clear all cached data from previous page
                self.js_modified_doc = None;
                self.cached_layout = None;
                self.external_css.clear();

                // Push to history (truncate forward history if we navigated from middle)
                if self.history_pos + 1 < self.history.len() {
                    self.history.truncate(self.history_pos + 1);
                }
                self.history.push(resp.url.clone());
                self.history_pos = self.history.len() - 1;

                // Fetch external CSS from <link> tags
                self.fetch_external_css(&resp.url, &resp.body);

                // Execute <script> tags
                self.execute_scripts();
                self.layout_dirty = true;

                // Render text content immediately (before images)
                self.image_cache.clear();
                self.render();

                // Fetch images in background (parallel)
                let css_for_images = self.external_css.clone();
                self.fetch_page_images_async(&resp.url, &resp.body, &css_for_images);
            }
            Err(e) => {
                self.log_network("GET", &url_str, None, "", 0, Some(&e));
                self.html_content = format!(
                    r#"<html><body><h1>Error</h1><p>Failed to load: {}</p><p>{}</p></body></html>"#,
                    url_str, e
                );
                self.current_url = url_str.clone();
                self.address_text = url_str;
                self.scroll_y = 0.0;
                self.image_cache.clear();
                self.js_modified_doc = None;
                self.layout_dirty = true;
            }
        }

        self.request_redraw();
    }

    fn go_back(&mut self) {
        if self.history_pos > 0 {
            self.history_pos -= 1;
            let url = self.history[self.history_pos].clone();
            self.load_from_history(&url);
        }
    }

    fn go_forward(&mut self) {
        if self.history_pos + 1 < self.history.len() {
            self.history_pos += 1;
            let url = self.history[self.history_pos].clone();
            self.load_from_history(&url);
        }
    }

    fn load_from_history(&mut self, url: &str) {
        match fetch_url_solving_challenges(url) {
            Ok(mut resp) => {
                if let Some(target) = incognidium_shell::meta_refresh_target(&resp.body, &resp.url)
                {
                    eprintln!("Following meta refresh to {target}...");
                    if let Ok(redirect_resp) = fetch_url(&target) {
                        resp = redirect_resp;
                    } else {
                        eprintln!("Failed to follow meta refresh {target}");
                    }
                }
                if let Some(target) =
                    incognidium_shell::reddit_challenge_target(&resp.body, &resp.url)
                {
                    eprintln!("Solving Reddit challenge to {target}...");
                    if let Ok(challenge_resp) = fetch_url(&target) {
                        resp = challenge_resp;
                    } else {
                        eprintln!("Failed to solve Reddit challenge {target}");
                    }
                }
                self.html_content = resp.body.clone();
                self.current_url = resp.url.clone();
                self.address_text = resp.url.clone();
                self.cursor_pos = self.address_text.len();
                self.scroll_y = 0.0;
                self.js_modified_doc = None;
                self.cached_layout = None;
                self.fetch_external_css(&resp.url, &resp.body);
                self.execute_scripts();
                self.layout_dirty = true;

                // Render text immediately, fetch images in background
                self.image_cache.clear();
                self.render();
                let css_for_images = self.external_css.clone();
                self.fetch_page_images_async(&resp.url, &resp.body, &css_for_images);
            }
            Err(e) => {
                self.html_content =
                    format!(r#"<html><body><h1>Error</h1><p>{}</p></body></html>"#, e);
                self.image_cache.clear();
                self.external_css.clear();
                self.js_modified_doc = None;
                self.layout_dirty = true;
            }
        }
        self.request_redraw();
    }

    fn fetch_page_images_async(&mut self, base_url: &str, html: &str, css_text: &str) {
        let doc = parse_html(html);
        let mut urls: Vec<(String, String)> = Vec::new(); // (src, resolved_url)
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        const MAX_IMAGES: usize = 50;

        for node in &doc.nodes {
            if urls.len() >= MAX_IMAGES {
                break;
            }
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                if el.tag_name == "img" {
                    if let Some(src) = el.get_attr("src") {
                        if src.starts_with("data:") || incognidium_shell::is_inline_svg_url(&src) {
                            continue;
                        }
                        if let Ok(resolved) = resolve_url(base_url, src) {
                            if seen.insert(resolved.clone()) {
                                urls.push((src.to_string(), resolved));
                            }
                        }
                    }
                }
                // Inline style background-image URLs
                if let Some(style) = el.get_attr("style") {
                    for (_, url) in extract_css_image_urls(style) {
                        if url.starts_with("data:") {
                            continue;
                        }
                        if let Ok(resolved) = resolve_url(base_url, &url) {
                            if seen.insert(resolved.clone()) {
                                urls.push((url, resolved));
                            }
                        }
                    }
                }
            }
        }

        // Background images referenced in external CSS
        for (_, url) in extract_css_image_urls(css_text) {
            if url.starts_with("data:") {
                continue;
            }
            if urls.len() >= MAX_IMAGES {
                break;
            }
            if let Ok(resolved) = resolve_url(base_url, &url) {
                if seen.insert(resolved.clone()) {
                    urls.push((url, resolved));
                }
            }
        }

        if urls.is_empty() {
            return;
        }

        let pending = self.pending_images.clone();
        let loading = self.images_loading.clone();
        let referer = base_url.to_string();
        loading.store(true, Ordering::SeqCst);

        // Clear pending buffer
        pending.lock().unwrap().clear();

        std::thread::spawn(move || {
            // Limit concurrent subresource fetches to avoid triggering CDN
            // rate-limiters. A small gap between chunks is polite without
            // materially slowing most pages.
            for (i, chunk) in urls.chunks(4).enumerate() {
                if i > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                let chunk: Vec<_> = chunk.to_vec();
                let handles: Vec<_> = chunk
                    .into_iter()
                    .map(|(src, resolved)| {
                        let referer = referer.clone();
                        std::thread::spawn(move || {
                            match fetch_bytes_with_referer(&resolved, Some(&referer)) {
                                Ok(bytes) => {
                                    if let Some(img) = decode_and_downscale_image(&bytes) {
                                        return Some((src, img));
                                    }
                                }
                                Err(e) => {
                                    log::warn!("Failed to fetch image {src}: {e}");
                                }
                            }
                            None
                        })
                    })
                    .collect();

                for handle in handles {
                    if let Ok(Some((src, data))) = handle.join() {
                        pending.lock().unwrap().push((src, data));
                    }
                }
            }
            loading.store(false, Ordering::SeqCst);
        });
    }

    /// Drain any images that arrived from the background fetch thread.
    /// Returns true if new images were added.
    fn drain_pending_images(&mut self) -> bool {
        let mut pending = self.pending_images.lock().unwrap();
        if pending.is_empty() {
            return false;
        }
        for (src, data) in pending.drain(..) {
            self.image_cache.insert(src, data);
        }
        self.layout_dirty = true;
        true
    }

    fn fetch_external_css(&mut self, base_url: &str, html: &str) {
        self.external_css.clear();
        let doc = parse_html(html);
        let mut fetched = 0usize;
        const MAX_STYLESHEETS: usize = 20;
        // Each entry carries the link's `media` attribute (if any) so fetched
        // rules can be wrapped in a matching @media gate. @import rules inside
        // a gated sheet inherit that gate.
        let mut to_fetch: Vec<(String, Option<String>)> = Vec::new();

        // Collect <link> stylesheets
        for node in &doc.nodes {
            if fetched >= MAX_STYLESHEETS {
                break;
            }
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
                        // A link's `media` attribute gates when its rules apply.
                        // print-only sheets are skipped unless an onload handler
                        // flips the media to "all" (common perf pattern:
                        // <link rel="stylesheet" ... media="print" onload="this.media='all'">).
                        let gate = el
                            .get_attr("media")
                            .map(|m| m.trim())
                            .filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("all"))
                            .map(|m| m.to_string());
                        if gate.as_deref() == Some("print") {
                            let flips_to_all = el
                                .get_attr("onload")
                                .map(|onload| {
                                    let lower = onload.to_lowercase();
                                    lower.contains("this.media")
                                        && (lower.contains("'all'") || lower.contains("\"all\""))
                                })
                                .unwrap_or(false);
                            if !flips_to_all {
                                continue;
                            }
                        }
                        if let Some(href) = el.get_attr("href") {
                            if let Ok(resolved) = resolve_url(base_url, href) {
                                to_fetch.push((resolved, gate.filter(|g| g != "print")));
                            }
                        }
                    }
                }
            }
        }

        // Fetch stylesheets and follow @import rules
        let mut fetched_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
        while let Some((url, gate)) = to_fetch.pop() {
            if fetched >= MAX_STYLESHEETS {
                break;
            }
            if fetched_urls.contains(&url) {
                continue;
            }
            fetched_urls.insert(url.clone());

            match fetch_url_fast(&url) {
                Ok(resp) => {
                    if resp.status < 200 || resp.status >= 300 {
                        log::warn!(
                            "Skipping stylesheet {}: HTTP {} ({} bytes)",
                            url,
                            resp.status,
                            resp.body.len()
                        );
                        continue;
                    }
                    // Follow @import rules (conditional imports are wrapped in
                    // their @media gate) instead of a line-based scan, which
                    // loses media conditions.
                    let mut with_imports = String::new();
                    let mut seen_imports = std::collections::HashSet::new();
                    let mut resolve_and_fetch =
                        |base: &str, href: &str| -> Option<(String, String)> {
                            let resolved = resolve_url(base, href).ok()?;
                            let resp = fetch_url_fast(&resolved).ok()?;
                            if resp.status < 200 || resp.status >= 300 {
                                return None;
                            }
                            Some((resolved, resp.body))
                        };
                    incognidium_shell::append_stylesheet_with_imports(
                        &mut with_imports,
                        &resp.body,
                        &url,
                        &mut resolve_and_fetch,
                        &mut seen_imports,
                        0,
                    );
                    // Wrap gated rules in their @media gate so the stylesheet
                    // parser evaluates it exactly like an @media rule.
                    match gate.as_deref() {
                        Some(m) => {
                            self.external_css.push_str(&format!("@media {} {{\n", m));
                            self.external_css.push_str(&with_imports);
                            self.external_css.push_str("\n}\n");
                        }
                        None => {
                            self.external_css.push_str(&with_imports);
                        }
                    }
                    fetched += 1;
                }
                Err(e) => {
                    log::warn!("Failed to fetch stylesheet: {e}");
                }
            }
        }
    }

    fn reload(&mut self) {
        let url = self.current_url.clone();
        self.load_from_history(&url);
    }

    fn execute_scripts(&mut self) {
        let doc = parse_html(&self.html_content);
        let scripts = collect_scripts(&doc, &self.current_url);
        if !scripts.is_empty() {
            let mut image_cache = std::collections::HashMap::new();
            let (viewport_width, viewport_height) = self
                .window
                .as_ref()
                .map(|w| {
                    let size = w.inner_size();
                    (size.width as f32, size.height as f32)
                })
                .unwrap_or((1024.0, 768.0));
            let modified_doc = incognidium_shell::execute_scripts_on_doc(
                doc,
                &scripts,
                &mut image_cache,
                &self.current_url,
                viewport_width,
                viewport_height,
            );
            self.image_cache.extend(image_cache);
            self.js_modified_doc = Some(modified_doc);
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn render(&mut self) {
        let window = match &self.window {
            Some(w) => w.clone(),
            None => return,
        };
        let surface = match &mut self.surface {
            Some(s) => s,
            None => return,
        };

        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        // Use physical size for the window buffer, logical size for layout/paint
        let phys_width = size.width.max(1);
        let phys_height = size.height.max(1);
        let width = (phys_width as f32 / scale).max(1.0) as u32;
        let height = (phys_height as f32 / scale).max(1.0) as u32;
        let _page_height = height.saturating_sub(TOOLBAR_HEIGHT);

        // Re-layout when content changed, window resized, or no cached layout yet
        if self.layout_dirty || self.last_layout_width != width || self.cached_layout.is_none() {
            // Use JS-modified DOM if available, otherwise re-parse from HTML
            let mut doc = if let Some(ref modified) = self.js_modified_doc {
                modified.clone()
            } else {
                parse_html(&self.html_content)
            };
            // Repair any cycles / broken parent pointers from JS before layout.
            doc.sanitize_tree();
            let mut css_text = self.external_css.clone();
            // The shell executes scripts, so styles inside <noscript> are not
            // meant for this render and must be skipped.
            css_text.push_str(&doc.collect_style_text_skip_noscript());

            // Force light mode: drop `prefers-color-scheme: dark` blocks so
            // pages using dark-only themes don't render as a black page.
            css_text = incognidium_shell::strip_dark_mode_media_queries(&css_text);

            let stylesheet = parse_css(&css_text);
            // Load @font-face web fonts so text measurement and painting use
            // the fonts the page declares instead of the built-in fallbacks.
            let page_url = self.current_url.clone();
            incognidium_css::webfonts::load_from_stylesheet(
                &stylesheet,
                &page_url,
                &|base, src| {
                    resolve_url(base, src)
                        .ok()
                        .and_then(|u| fetch_bytes(&u).ok())
                        .unwrap_or_default()
                },
            );
            let mut styles = resolve_styles(&doc, &stylesheet, width as f32, height as f32);

            // Rasterize inline SVGs after styles are resolved so `currentColor`
            // maps to the computed element color.
            incognidium_shell::rasterize_inline_svgs(
                &mut doc,
                &mut self.image_cache,
                Some(&mut styles),
                width as f32,
                height as f32,
                Some(&self.current_url),
            );

            let mut image_sizes = ImageSizes::new();
            for (src, img) in &self.image_cache {
                image_sizes.insert(src.clone(), (img.intrinsic_width, img.intrinsic_height));
            }

            // First layout pass produces real container sizes so that
            // `@container` queries can be evaluated in the second style pass.
            let layout_root =
                layout_with_images(&doc, &styles, width as f32, height as f32, &image_sizes);
            styles = resolve_styles_with_container_sizes(
                &doc,
                &stylesheet,
                width as f32,
                height as f32,
                &layout_root,
                &styles,
            );
            let layout_root =
                layout_with_images(&doc, &styles, width as f32, height as f32, &image_sizes);

            self.cached_layout = Some(CachedLayout {
                doc,
                styles,
                layout_root,
            });
            self.layout_dirty = false;
            self.last_layout_width = width;
        }

        let cached = match &self.cached_layout {
            Some(c) => c,
            None => return,
        };

        // Flatten with current scroll offset (cheap) — logical coordinates
        let flat_boxes = flatten_layout(&cached.layout_root, 0.0, -self.scroll_y, &cached.styles);
        self.flat_boxes = flat_boxes.clone();

        // Scale flat boxes and styles for physical-resolution painting
        let scaled_flat_boxes = scale_flat_boxes(&flat_boxes, scale);
        let scaled_styles = scale_styles(&cached.styles, scale);
        let phys_page_height = phys_height.saturating_sub((TOOLBAR_HEIGHT as f32 * scale) as u32);

        // Paint page content at physical resolution
        let canvas_bg = propagate_canvas_background(&cached.doc, &cached.styles);
        let pixmap = paint_with_images_and_canvas(
            &scaled_flat_boxes,
            &scaled_styles,
            phys_width,
            phys_page_height,
            &self.image_cache,
            canvas_bg,
        );

        // Create full window pixmap at physical resolution
        let mut full = Pixmap::new(phys_width, phys_height).expect("pixmap");
        full.fill(Color::WHITE);

        let th = TOOLBAR_HEIGHT as f32 * scale;

        // Draw toolbar background
        draw_toolbar_rect(&mut full, 0.0, 0.0, phys_width as f32, th, 0xf0, 0xf0, 0xf0);
        // Toolbar bottom border
        draw_toolbar_rect(
            &mut full,
            0.0,
            th - 1.0,
            phys_width as f32,
            1.0,
            0xcc,
            0xcc,
            0xcc,
        );

        // Navigation buttons with icons
        self.frame_count = self.frame_count.wrapping_add(1);
        let frame_angle = (self.frame_count % 60) as f32 * 6.0;
        let can_back = self.history_pos > 0;
        draw_icon_button(
            &mut full,
            6.0 * scale,
            BTN_Y * scale,
            ButtonIcon::Back,
            can_back,
            scale,
            frame_angle,
        );
        let can_fwd = self.history_pos + 1 < self.history.len();
        draw_icon_button(
            &mut full,
            36.0 * scale,
            BTN_Y * scale,
            ButtonIcon::Forward,
            can_fwd,
            scale,
            frame_angle,
        );
        let reloading = self.images_loading.load(Ordering::SeqCst);
        draw_icon_button(
            &mut full,
            66.0 * scale,
            BTN_Y * scale,
            if reloading {
                ButtonIcon::Reloading
            } else {
                ButtonIcon::Reload
            },
            true,
            scale,
            frame_angle,
        );

        // Address bar
        let addr_width = phys_width as f32 - ADDR_BAR_LEFT * scale - ADDR_BAR_RIGHT_MARGIN * scale;
        let is_https = self.current_url.starts_with("https://");
        draw_address_bar(
            &mut full,
            ADDR_BAR_LEFT * scale,
            ADDR_BAR_TOP * scale,
            addr_width,
            ADDR_BAR_HEIGHT * scale,
            &self.address_text,
            self.address_focused,
            self.cursor_pos,
            self.address_all_selected,
            is_https,
            scale,
        );

        // Menu button on the far right
        let menu_x = phys_width as f32 - BTN_SIZE - MENU_BTN_MARGIN;
        draw_icon_button(
            &mut full,
            menu_x,
            BTN_Y * scale,
            ButtonIcon::Menu,
            true,
            scale,
            frame_angle,
        );

        // Draw menu dropdown if open
        if self.menu_open {
            let win_width = phys_width as f32 / scale;
            let dw = 160.0 * scale;
            let dh = 132.0 * scale;
            let dx = (win_width - dw - MENU_BTN_MARGIN * scale).max(0.0);
            let dy = (TOOLBAR_HEIGHT as f32 - 2.0) * scale;
            draw_rounded_rect(&mut full, dx, dy, dw, dh, 6.0 * scale, 0xff, 0xff, 0xff);
            draw_rounded_rect_stroke(
                &mut full,
                dx,
                dy,
                dw,
                dh,
                6.0 * scale,
                1.0,
                0xcc,
                0xcc,
                0xcc,
            );
            let items = ["Back", "Forward", "Reload", "About"];
            let line_h = 22.0 * scale;
            let pad = 10.0 * scale;
            for (i, item) in items.iter().enumerate() {
                let iy = dy + 8.0 * scale + i as f32 * (line_h + 4.0 * scale);
                draw_toolbar_text(
                    &mut full,
                    dx + pad,
                    iy + line_h * 0.7,
                    item,
                    0x33,
                    0x33,
                    0x33,
                    scale,
                );
                if i + 1 < items.len() {
                    draw_toolbar_rect(
                        &mut full,
                        dx + pad,
                        iy + line_h + 2.0 * scale,
                        dw - pad * 2.0,
                        1.0,
                        0xee,
                        0xee,
                        0xee,
                    );
                }
            }
        }

        // Loading bar under toolbar when async resources are still fetching
        if reloading {
            let bar_y = th - 3.0 * scale;
            let bar_w = phys_width as f32 * 0.6;
            let bar_x = (phys_width as f32 - bar_w) * 0.5;
            draw_toolbar_rect(
                &mut full,
                0.0,
                bar_y,
                phys_width as f32,
                3.0 * scale,
                0xcc,
                0xcc,
                0xcc,
            );
            let progress = ((self.frame_count % 120) as f32 / 120.0) * bar_w * 1.4 - bar_w * 0.2;
            let p0 = progress.max(0.0);
            let p1 = (progress + bar_w * 0.4).min(phys_width as f32);
            if p1 > p0 {
                draw_toolbar_rect(
                    &mut full,
                    bar_x + p0,
                    bar_y,
                    p1 - p0,
                    3.0 * scale,
                    0x22,
                    0x77,
                    0xee,
                );
            }
        }

        // Copy physical-resolution page content directly below toolbar
        let page_data = pixmap.data();
        let full_data = full.data_mut();
        for y in 0..phys_page_height {
            for x in 0..phys_width {
                let src = ((y * phys_width + x) * 4) as usize;
                let dst = (((y + th as u32) * phys_width + x) * 4) as usize;
                if src + 3 < page_data.len() && dst + 3 < full_data.len() {
                    full_data[dst] = page_data[src];
                    full_data[dst + 1] = page_data[src + 1];
                    full_data[dst + 2] = page_data[src + 2];
                    full_data[dst + 3] = page_data[src + 3];
                }
            }
        }

        // Copy to window surface at physical resolution
        surface
            .resize(
                NonZeroU32::new(phys_width).unwrap(),
                NonZeroU32::new(phys_height).unwrap(),
            )
            .expect("resize");

        let mut buffer = surface.buffer_mut().expect("buffer");
        let data = full.data();
        let buf_len = buffer.len();
        for y in 0..phys_height {
            for x in 0..phys_width {
                let idx = ((y * phys_width + x) * 4) as usize;
                let buf_idx = (y * phys_width + x) as usize;
                if idx + 3 < data.len() && buf_idx < buf_len {
                    let r = data[idx] as u32;
                    let g = data[idx + 1] as u32;
                    let b = data[idx + 2] as u32;
                    buffer[buf_idx] = (r << 16) | (g << 8) | b;
                }
            }
        }

        buffer.present().expect("present");

        // Save screenshot if requested
        if let Some(path) = self.pending_screenshot.take() {
            match save_png_compressed(&full, std::path::Path::new(&path)) {
                Ok(_) => eprintln!("Screenshot saved to {path}"),
                Err(e) => eprintln!("Failed to save screenshot to {path}: {e}"),
            }
        }

        // Sync state to devtools bridge
        self.sync_devtools(&full);
    }

    fn handle_click(&mut self, x: f64, y: f64) {
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor() as f32)
            .unwrap_or(1.0);
        let x = x as f32 / scale;
        let y = y as f32 / scale;

        // Check if click is in toolbar
        if y < TOOLBAR_HEIGHT as f32 {
            // Back button
            if (6.0..6.0 + BTN_SIZE).contains(&x) && (BTN_Y..BTN_Y + BTN_SIZE).contains(&y) {
                self.go_back();
                return;
            }
            // Forward button
            if (36.0..36.0 + BTN_SIZE).contains(&x) && (BTN_Y..BTN_Y + BTN_SIZE).contains(&y) {
                self.go_forward();
                return;
            }
            // Reload button
            if (66.0..66.0 + BTN_SIZE).contains(&x) && (BTN_Y..BTN_Y + BTN_SIZE).contains(&y) {
                self.reload();
                return;
            }
            // Address bar
            if x >= ADDR_BAR_LEFT && (ADDR_BAR_TOP..ADDR_BAR_TOP + ADDR_BAR_HEIGHT).contains(&y) {
                self.address_focused = true;
                self.address_all_selected = true;
                self.cursor_pos = self.address_text.len();
                self.request_redraw();
                return;
            }
            // Menu button
            let win_width = self
                .window
                .as_ref()
                .map(|w| w.inner_size().width)
                .unwrap_or(DEFAULT_WIDTH);
            let menu_x = win_width as f32 / scale - BTN_SIZE - MENU_BTN_MARGIN;
            if (menu_x..menu_x + BTN_SIZE).contains(&x) && (BTN_Y..BTN_Y + BTN_SIZE).contains(&y) {
                return;
            }
            if self.menu_open {
                let dx = win_width as f32 / scale - 160.0 - MENU_BTN_MARGIN;
                let dy = TOOLBAR_HEIGHT as f32 - 2.0;
                let dw = 160.0;
                let dh = 132.0;
                if x >= dx && x <= dx + dw && y >= dy && y <= dy + dh {
                    let rel_y = y - dy - 8.0;
                    let item_h = 26.0;
                    let item = (rel_y / item_h).floor() as usize;
                    match item {
                        0 => {
                            self.menu_open = false;
                            self.go_back();
                        }
                        1 => {
                            self.menu_open = false;
                            self.go_forward();
                        }
                        2 => {
                            self.menu_open = false;
                            self.reload();
                        }
                        _ => {
                            self.menu_open = false;
                        }
                    }
                    return;
                }
                self.menu_open = false;
                return;
            }
        } else {
            // Click outside address bar unfocuses it
            self.address_focused = false;

            // Check for link clicks (y is relative to window, flat_boxes are relative to page)
            let page_y = y - TOOLBAR_HEIGHT as f32;
            for fbox in &self.flat_boxes {
                if let Some(ref href) = fbox.link_href {
                    if x >= fbox.x
                        && x <= fbox.x + fbox.width
                        && page_y >= fbox.y
                        && page_y <= fbox.y + fbox.height
                    {
                        let resolved = match resolve_url(&self.current_url, href) {
                            Ok(u) => u,
                            Err(_) => href.clone(),
                        };
                        self.navigate(&resolved);
                        return;
                    }
                }
            }

            self.request_redraw();
        }
    }

    fn handle_key(&mut self, key: Key, state: ElementState) {
        if state != ElementState::Pressed {
            return;
        }
        if !self.address_focused {
            // Page scrolling
            match key {
                Key::Named(NamedKey::ArrowDown) => {
                    self.scroll_y += 40.0;
                    self.request_redraw();
                }
                Key::Named(NamedKey::ArrowUp) => {
                    self.scroll_y = (self.scroll_y - 40.0).max(0.0);
                    self.request_redraw();
                }
                Key::Named(NamedKey::PageDown) => {
                    self.scroll_y += 400.0;
                    self.request_redraw();
                }
                Key::Named(NamedKey::PageUp) => {
                    self.scroll_y = (self.scroll_y - 400.0).max(0.0);
                    self.request_redraw();
                }
                Key::Named(NamedKey::Home) => {
                    self.scroll_y = 0.0;
                    self.request_redraw();
                }
                Key::Named(NamedKey::F12) => {
                    let dir = std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join(".local/share/incognidium/screenshots");
                    let _ = std::fs::create_dir_all(&dir);
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let filename = format!("screenshot_{ts}.png");
                    let path = dir.join(filename).to_string_lossy().to_string();
                    eprintln!("Screenshot requested: {path}");
                    self.pending_screenshot = Some(path);
                    self.request_redraw();
                }
                _ => {}
            }
            return;
        }

        // Address bar keyboard handling
        match key {
            Key::Named(NamedKey::Enter) => {
                let url = self.address_text.clone();
                self.address_focused = false;
                self.navigate(&url);
            }
            Key::Named(NamedKey::Backspace) => {
                if self.address_all_selected {
                    self.address_text.clear();
                    self.cursor_pos = 0;
                    self.address_all_selected = false;
                    self.request_redraw();
                } else if self.cursor_pos > 0 {
                    self.address_text.remove(self.cursor_pos - 1);
                    self.cursor_pos -= 1;
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::Delete) if self.cursor_pos < self.address_text.len() => {
                self.address_text.remove(self.cursor_pos);
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.address_all_selected = false;
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.address_all_selected = false;
                if self.cursor_pos < self.address_text.len() {
                    self.cursor_pos += 1;
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::Home) => {
                self.cursor_pos = 0;
                self.request_redraw();
            }
            Key::Named(NamedKey::End) => {
                self.cursor_pos = self.address_text.len();
                self.request_redraw();
            }
            Key::Named(NamedKey::Escape) => {
                self.address_text = self.current_url.clone();
                self.cursor_pos = self.address_text.len();
                self.address_focused = false;
                self.request_redraw();
            }
            Key::Character(ref ch) => {
                if self.address_all_selected {
                    // Replace all text with new input
                    self.address_text.clear();
                    self.cursor_pos = 0;
                    self.address_all_selected = false;
                }
                // Filter control chars
                for c in ch.chars() {
                    if c >= ' ' && c != '\x7f' {
                        self.address_text.insert(self.cursor_pos, c);
                        self.cursor_pos += 1;
                    }
                }
                self.request_redraw();
            }
            _ => {}
        }
    }

    fn handle_scroll(&mut self, delta_y: f32) {
        self.scroll_y = (self.scroll_y - delta_y * 30.0).max(0.0);
        self.request_redraw();
    }

    fn log_network(
        &self,
        method: &str,
        url: &str,
        status: Option<u16>,
        content_type: &str,
        size: usize,
        error: Option<&str>,
    ) {
        if let Some(ref dt) = self.devtools {
            dt.log_network(NetworkEntry {
                method: method.to_string(),
                url: url.to_string(),
                status,
                content_type: content_type.to_string(),
                size,
                error: error.map(|e| e.to_string()),
            });
        }
    }

    fn sync_devtools(&mut self, full_pixmap: &Pixmap) {
        let dt = match &self.devtools {
            Some(dt) => dt.clone(),
            None => return,
        };

        let cached = match &self.cached_layout {
            Some(c) => c,
            None => return,
        };

        let viewport = self
            .window
            .as_ref()
            .map(|w| {
                let s = w.inner_size();
                (s.width, s.height)
            })
            .unwrap_or((1024, 768));

        let title = extract_title(&cached.doc);
        let page_text = extract_page_text(&self.flat_boxes);
        let links = extract_links(&self.flat_boxes);
        let console_lines: Vec<String> = self.js_vm.console_output.lines.clone();

        dt.update_page_state(
            &self.current_url,
            &title,
            &self.html_content,
            &page_text,
            &console_lines,
            links,
            self.scroll_y,
            cached.layout_root.height,
            viewport,
            self.history_pos > 0,
            self.history_pos + 1 < self.history.len(),
        );

        dt.update_dom(&cached.doc);
        dt.update_layout(&cached.layout_root);
        dt.update_styles(&cached.doc, &cached.styles);

        let mut png_data = Vec::new();
        if encode_png_compressed(&full_pixmap, std::io::Cursor::new(&mut png_data)).is_ok() {
            dt.update_screenshot(png_data);
        }
    }

    fn process_devtools_commands(&mut self) {
        let dt = match &self.devtools {
            Some(dt) => dt.clone(),
            None => return,
        };

        if let Some(cmd) = dt.take_pending_command() {
            match cmd {
                DevToolsCommand::Navigate(url) => {
                    self.navigate(&url);
                    self.render();
                    dt.complete_command(format!("Navigated to {}", self.current_url), None);
                }
                DevToolsCommand::Back => {
                    self.go_back();
                    self.render();
                    dt.complete_command(format!("Back to {}", self.current_url), None);
                }
                DevToolsCommand::Forward => {
                    self.go_forward();
                    self.render();
                    dt.complete_command(format!("Forward to {}", self.current_url), None);
                }
                DevToolsCommand::Reload => {
                    self.reload();
                    self.render();
                    dt.complete_command(format!("Reloaded {}", self.current_url), None);
                }
                DevToolsCommand::Scroll(y) => {
                    self.scroll_y = y.max(0.0);
                    self.render();
                    dt.complete_command(format!("Scrolled to y={}", self.scroll_y), None);
                }
                DevToolsCommand::Click { x, y } => {
                    // Simulate click on page content (add toolbar offset for handle_click)
                    self.handle_click(x as f64, (y + TOOLBAR_HEIGHT as f32) as f64);
                    self.render();
                    dt.complete_command(format!("Clicked at ({x}, {y})"), None);
                }
                DevToolsCommand::ExecuteJs(code) => {
                    let result = match self.js_vm.eval(&code) {
                        Ok(val) => format!("{val}"),
                        Err(e) => format!("Error: {e}"),
                    };
                    let console = self.js_vm.console_output.lines.join("\n");
                    dt.complete_command(console, Some(result));
                }
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("Incognidium Browser")
            .with_window_icon(load_window_icon())
            .with_inner_size(winit::dpi::LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));

        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));

        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("surface");

        self.window = Some(window);
        self.surface = Some(surface);

        // Initial page setup: fetch external CSS, execute scripts, render text first
        let url = self.current_url.clone();
        let html = self.html_content.clone();
        self.fetch_external_css(&url, &html);
        self.execute_scripts();
        self.render();
        self.request_redraw();

        // Fetch images in background
        let css_for_images = self.external_css.clone();
        self.fetch_page_images_async(&url, &html, &css_for_images);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                self.render();
            }
            WindowEvent::Resized(_) => {
                self.render();
                self.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.last_cursor = Some((position.x, position.y));
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                if let Some((x, y)) = self.last_cursor {
                    self.handle_click(x, y);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some((x, y)) = self.last_cursor {
                    let scale = self
                        .window
                        .as_ref()
                        .map(|w| w.scale_factor() as f32)
                        .unwrap_or(1.0);
                    let sx = x as f32 / scale;
                    let sy = y as f32 / scale;
                    let win_width = self
                        .window
                        .as_ref()
                        .map(|w| w.inner_size().width)
                        .unwrap_or(DEFAULT_WIDTH);
                    let menu_x = win_width as f32 / scale - BTN_SIZE - MENU_BTN_MARGIN;
                    if (menu_x..menu_x + BTN_SIZE).contains(&sx)
                        && (BTN_Y..BTN_Y + BTN_SIZE).contains(&sy)
                    {
                        self.menu_open = !self.menu_open;
                        self.request_redraw();
                    } else if self.menu_open && sy > TOOLBAR_HEIGHT as f32 {
                        self.menu_open = false;
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 10.0,
                };
                self.handle_scroll(dy);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(event.logical_key, event.state);
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        self.process_devtools_commands();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Check for images arriving from background fetch
        if self.drain_pending_images() {
            self.request_redraw();
        }
        // Keep polling while images are still loading
        if self.images_loading.load(Ordering::SeqCst) {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::wait_duration(
                Duration::from_millis(100),
            ));
        }
    }
}

fn scale_flat_boxes(
    boxes: &[incognidium_layout::FlatBox],
    scale: f32,
) -> Vec<incognidium_layout::FlatBox> {
    boxes
        .iter()
        .map(|b| {
            let mut s = b.clone();
            s.x *= scale;
            s.y *= scale;
            s.width *= scale;
            s.height *= scale;
            if let Some((cx, cy, cw, ch)) = s.clip {
                s.clip = Some((cx * scale, cy * scale, cw * scale, ch * scale));
            }
            if let Some((indent, lines, is_left)) = s.float_text_indent {
                s.float_text_indent = Some((indent * scale, lines, is_left));
            }
            if let Some(ref mut v) = s.marker_font_size {
                *v *= scale;
            }
            if let Some(ref mut v) = s.first_letter_font_size {
                *v *= scale;
            }
            if let Some(ref mut v) = s.first_letter_border_width {
                *v *= scale;
            }
            if let Some((t, r, b, l)) = s.first_letter_margin {
                s.first_letter_margin = Some((t * scale, r * scale, b * scale, l * scale));
            }
            s
        })
        .collect()
}

fn scale_styles(styles: &incognidium_style::StyleMap, scale: f32) -> incognidium_style::StyleMap {
    let mut scaled = incognidium_style::StyleMap::new();
    for (id, style) in styles {
        let mut s = style.clone();
        s.font_size *= scale;
        if let Some(ref mut v) = s.font_min_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.font_max_size {
            *v *= scale;
        }
        s.text_indent *= scale;
        s.margin_top *= scale;
        s.margin_right *= scale;
        s.margin_bottom *= scale;
        s.margin_left *= scale;
        s.padding_top *= scale;
        s.padding_right *= scale;
        s.padding_bottom *= scale;
        s.padding_left *= scale;
        s.border_top_width *= scale;
        s.border_right_width *= scale;
        s.border_bottom_width *= scale;
        s.border_left_width *= scale;
        s.gap *= scale;
        s.column_gap *= scale;
        s.row_gap *= scale;
        s.letter_spacing *= scale;
        s.word_spacing *= scale;
        s.outline_width *= scale;
        s.outline_offset *= scale;
        s.offset_distance *= scale;
        if let Some(ref mut v) = s.marker_font_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.first_letter_font_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.first_letter_border_width {
            *v *= scale;
        }
        if let Some((t, r, b, l)) = s.first_letter_margin {
            s.first_letter_margin = Some((t * scale, r * scale, b * scale, l * scale));
        }
        if let Some(ref mut v) = s.first_line_font_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.first_line_letter_spacing {
            *v *= scale;
        }
        if let Some(ref mut v) = s.first_line_word_spacing {
            *v *= scale;
        }
        if let Some(ref mut v) = s.placeholder_font_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.cue_font_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.part_font_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.slotted_font_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.details_content_padding {
            *v *= scale;
        }
        if let Some(ref mut v) = s.column_width {
            *v *= scale;
        }
        s.column_rule_width *= scale;
        if let Some(ref mut v) = s.contain_intrinsic_width {
            *v *= scale;
        }
        if let Some(ref mut v) = s.contain_intrinsic_height {
            *v *= scale;
        }
        if let Some(ref mut v) = s.contain_intrinsic_block_size {
            *v *= scale;
        }
        if let Some(ref mut v) = s.contain_intrinsic_inline_size {
            *v *= scale;
        }
        scaled.insert(*id, s);
    }
    scaled
}

const ROUNDED_RECT_KAPPA: f32 = 0.5522847498;

// --- Toolbar drawing helpers ---

#[allow(clippy::too_many_arguments)]
fn draw_toolbar_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: u8, g: u8, b: u8) {
    if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(r, g, b, 255));
        let path = PathBuilder::from_rect(rect);
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn push_rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    if r > 0.0 {
        let k = r * ROUNDED_RECT_KAPPA;
        pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    }
    pb.line_to(x + w, y + h - r);
    if r > 0.0 {
        let k = r * ROUNDED_RECT_KAPPA;
        pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    }
    pb.line_to(x + r, y + h);
    if r > 0.0 {
        let k = r * ROUNDED_RECT_KAPPA;
        pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    }
    pb.line_to(x, y + r);
    if r > 0.0 {
        let k = r * ROUNDED_RECT_KAPPA;
        pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    }
    pb.close();
}

fn draw_rounded_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    r: u8,
    g: u8,
    b: u8,
) {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    let mut pb = PathBuilder::new();
    push_rounded_rect(&mut pb, x, y, w.max(1.0), h.max(1.0), radius);
    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn draw_rounded_rect_stroke(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    width: f32,
    r: u8,
    g: u8,
    b: u8,
) {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    let mut pb = PathBuilder::new();
    push_rounded_rect(&mut pb, x, y, w.max(1.0), h.max(1.0), radius);
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                dash: None,
                miter_limit: 4.0,
            },
            Transform::identity(),
            None,
        );
    }
}

#[derive(Clone, Copy)]
enum ButtonIcon {
    Back,
    Forward,
    Reload,
    Reloading,
    Menu,
}

fn draw_icon_button(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    icon: ButtonIcon,
    enabled: bool,
    scale: f32,
    frame_angle: f32,
) {
    let sz = BTN_SIZE * scale;
    let (bg_r, bg_g, bg_b) = if enabled {
        (0xe8, 0xe8, 0xe8)
    } else {
        (0xf5, 0xf5, 0xf5)
    };
    let (fg_r, fg_g, fg_b) = if enabled {
        (0x33, 0x33, 0x33)
    } else {
        (0xbb, 0xbb, 0xbb)
    };

    let radius = 5.0 * scale;
    draw_rounded_rect(pixmap, x, y, sz, sz, radius, bg_r, bg_g, bg_b);

    let cx = x + sz / 2.0;
    let cy = y + sz / 2.0;
    let s = 5.0 * scale;
    match icon {
        ButtonIcon::Back => draw_arrow(pixmap, cx, cy, s, fg_r, fg_g, fg_b, true),
        ButtonIcon::Forward => draw_arrow(pixmap, cx, cy, s, fg_r, fg_g, fg_b, false),
        ButtonIcon::Reload => draw_reload_icon(pixmap, cx, cy, s * 1.2, 0.0, fg_r, fg_g, fg_b),
        ButtonIcon::Reloading => {
            draw_reload_icon(pixmap, cx, cy, s * 1.2, frame_angle, fg_r, fg_g, fg_b)
        }
        ButtonIcon::Menu => draw_hamburger(pixmap, cx, cy, s * 1.3, fg_r, fg_g, fg_b),
    }
}

fn draw_arrow(pixmap: &mut Pixmap, cx: f32, cy: f32, s: f32, r: u8, g: u8, b: u8, left: bool) {
    let mut pb = PathBuilder::new();
    let head_x = if left { cx + s } else { cx - s };
    let tail_x = if left { cx - s } else { cx + s };
    let y0 = cy - s * 0.65;
    let y1 = cy;
    let y2 = cy + s * 0.65;
    pb.move_to(head_x, cy);
    pb.line_to(tail_x, y0);
    pb.move_to(head_x, cy);
    pb.line_to(tail_x, y1);
    pb.move_to(head_x, cy);
    pb.line_to(tail_x, y2);
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: 2.0,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                dash: None,
                miter_limit: 4.0,
            },
            Transform::identity(),
            None,
        );
    }
}

fn draw_chevron(
    pixmap: &mut Pixmap,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    x3: f32,
    y3: f32,
    r: u8,
    g: u8,
    b: u8,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    pb.line_to(x3, y3);
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: 2.0,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                dash: None,
                miter_limit: 4.0,
            },
            Transform::identity(),
            None,
        );
    }
}

fn draw_reload_icon(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    s: f32,
    angle_deg: f32,
    r: u8,
    g: u8,
    b: u8,
) {
    let angle = angle_deg.to_radians();
    let mut pb = PathBuilder::new();
    let k = 0.55;
    let points: [(f32, f32); 9] = [
        (cx + s, cy),
        (cx + s, cy - s * k),
        (cx + s * k, cy - s),
        (cx, cy - s),
        (cx - s * k, cy - s),
        (cx - s, cy - s * k),
        (cx - s, cy),
        (cx - s, cy + s * k),
        (cx - s * k, cy + s),
    ];
    let rotate = |(x, y): (f32, f32)| {
        let dx = x - cx;
        let dy = y - cy;
        (
            cx + dx * angle.cos() - dy * angle.sin(),
            cy + dx * angle.sin() + dy * angle.cos(),
        )
    };
    let p0 = rotate(points[0]);
    let p1 = rotate(points[1]);
    let p2 = rotate(points[2]);
    let p3 = rotate(points[3]);
    let p4 = rotate(points[4]);
    let p5 = rotate(points[5]);
    let p6 = rotate(points[6]);
    let p7 = rotate(points[7]);
    let p8 = rotate(points[8]);
    let p9 = rotate((cx + s * 0.4, cy + s));
    pb.move_to(p0.0, p0.1);
    pb.cubic_to(p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
    pb.cubic_to(p4.0, p4.1, p5.0, p5.1, p6.0, p6.1);
    pb.cubic_to(p7.0, p7.1, p8.0, p8.1, p9.0, p9.1);
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: 2.0,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                dash: None,
                miter_limit: 4.0,
            },
            Transform::identity(),
            None,
        );
    }
    let arr = s * 0.6;
    let arr_p = rotate((cx + s, cy - s * 0.4));
    draw_chevron(
        pixmap,
        arr_p.0 - arr * angle.sin() - arr * angle.cos(),
        arr_p.1 + arr * angle.cos() - arr * angle.sin(),
        arr_p.0,
        arr_p.1,
        arr_p.0 + arr * angle.sin() - arr * angle.cos(),
        arr_p.1 - arr * angle.cos() - arr * angle.sin(),
        r,
        g,
        b,
    );
}

fn draw_hamburger(pixmap: &mut Pixmap, cx: f32, cy: f32, s: f32, r: u8, g: u8, b: u8) {
    let y1 = cy - s * 0.45;
    let y2 = cy;
    let y3 = cy + s * 0.45;
    let line_w = s * 1.2;
    for y in [y1, y2, y3] {
        draw_toolbar_rect(pixmap, cx - line_w * 0.5, y - 0.75, line_w, 1.5, r, g, b);
    }
}

fn draw_lock_icon(pixmap: &mut Pixmap, cx: f32, cy: f32, s: f32, r: u8, g: u8, b: u8) {
    let body_w = s * 1.1;
    let body_h = s * 0.9;
    let shackle_r = s * 0.35;
    let body_x = cx - body_w / 2.0;
    let body_y = cy + shackle_r * 0.3;
    draw_rounded_rect(pixmap, body_x, body_y, body_w, body_h, 1.5, r, g, b);

    let arc_cx = cx;
    let arc_cy = body_y - shackle_r * 0.3;
    let mut a = PathBuilder::new();
    a.move_to(arc_cx - shackle_r, arc_cy + shackle_r);
    a.cubic_to(
        arc_cx - shackle_r,
        arc_cy - shackle_r,
        arc_cx + shackle_r,
        arc_cy - shackle_r,
        arc_cx + shackle_r,
        arc_cy + shackle_r,
    );
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    if let Some(path) = a.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: 2.0,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                dash: None,
                miter_limit: 4.0,
            },
            Transform::identity(),
            None,
        );
    }

    let keyhole_h = body_h * 0.45;
    draw_toolbar_rect(
        pixmap,
        cx - 1.0,
        body_y + body_h * 0.25,
        2.0,
        keyhole_h,
        0xff,
        0xff,
        0xff,
    );
    draw_rounded_rect(
        pixmap,
        cx - 2.0,
        body_y + body_h * 0.15,
        4.0,
        4.0,
        2.0,
        0xff,
        0xff,
        0xff,
    );
}

fn draw_insecure_icon(pixmap: &mut Pixmap, cx: f32, cy: f32, s: f32) {
    let r = 0xdd;
    let g = 0x66;
    let b = 0x22;
    draw_rounded_rect(pixmap, cx - s, cy - s, s * 2.0, s * 2.0, s * 0.35, r, g, b);
    let bar_h = s * 0.4;
    let bar_w = s * 1.2;
    draw_toolbar_rect(
        pixmap,
        cx - bar_w * 0.5,
        cy - bar_h * 0.5,
        bar_w,
        bar_h,
        0xff,
        0xff,
        0xff,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_address_bar(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    text: &str,
    focused: bool,
    cursor_pos: usize,
    all_selected: bool,
    is_https: bool,
    scale: f32,
) {
    let pad = 6.0 * scale;
    let icon_w = 22.0 * scale;
    let radius = 6.0 * scale;

    let (br, bg, bb) = if focused {
        (0x44, 0x88, 0xee)
    } else {
        (0xcc, 0xcc, 0xcc)
    };
    let bg_col = if focused {
        (0xff, 0xff, 0xff)
    } else {
        (0xf7, 0xf7, 0xf7)
    };
    let outer_r = radius + 1.5 * scale;

    draw_rounded_rect(
        pixmap,
        x - 1.5 * scale,
        y - 1.5 * scale,
        w + 3.0 * scale,
        h + 3.0 * scale,
        outer_r,
        br,
        bg,
        bb,
    );
    draw_rounded_rect(pixmap, x, y, w, h, radius, bg_col.0, bg_col.1, bg_col.2);

    let text_x = x + pad + icon_w;
    let max_text_w = w - icon_w - pad * 2.0;

    let mut display_text = text;
    while measure_toolbar_text(display_text, scale) > max_text_w && display_text.len() > 1 {
        display_text = &display_text[1..];
    }

    let icon_cx = x + pad + icon_w / 2.0;
    let icon_cy = y + h / 2.0;
    if is_https {
        draw_lock_icon(pixmap, icon_cx, icon_cy, 6.0 * scale, 0x22, 0x99, 0x44);
    } else {
        draw_insecure_icon(pixmap, icon_cx, icon_cy, 6.0 * scale);
    }

    let placeholder = "Search or enter address";
    let show_placeholder = !focused && text.is_empty();
    let display_text = if show_placeholder {
        placeholder
    } else {
        display_text
    };
    let text_color = if show_placeholder {
        (0x99, 0x99, 0x99)
    } else {
        (0x22, 0x22, 0x22)
    };

    if focused && all_selected && !text.is_empty() {
        let sel_w = measure_toolbar_text(display_text, scale);
        draw_rounded_rect(
            pixmap,
            text_x,
            y + 4.0 * scale,
            sel_w.min(max_text_w),
            h - 8.0 * scale,
            2.0 * scale,
            0x33,
            0x66,
            0xcc,
        );
        draw_toolbar_text(
            pixmap,
            text_x,
            y + 7.0 * scale,
            display_text,
            0xff,
            0xff,
            0xff,
            scale,
        );
    } else {
        draw_toolbar_text(
            pixmap,
            text_x,
            y + 7.0 * scale,
            display_text,
            text_color.0,
            text_color.1,
            text_color.2,
            scale,
        );
    }

    if focused && !all_selected {
        let trim_offset = text.len() - display_text.len();
        let visible_cursor_text = if cursor_pos > trim_offset {
            &text[trim_offset..cursor_pos]
        } else {
            ""
        };
        let cx = text_x + measure_toolbar_text(visible_cursor_text, scale);
        draw_toolbar_rect(
            pixmap,
            cx,
            y + 5.0 * scale,
            1.5 * scale,
            h - 10.0 * scale,
            0x00,
            0x00,
            0x00,
        );
    }
}

// -- Toolbar TTF font --

static TOOLBAR_FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();

fn get_toolbar_font() -> Option<&'static fontdue::Font> {
    TOOLBAR_FONT
        .get_or_init(|| {
            let paths = [
                "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ];
            for path in &paths {
                if let Ok(data) = std::fs::read(path) {
                    if let Ok(font) =
                        fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
                    {
                        return Some(font);
                    }
                }
            }
            // Fallback to the same embedded Roboto font used for page rendering so
            // the toolbar/address bar is readable even when no system fonts are
            // installed.
            fontdue::Font::from_bytes(
                include_bytes!("../../../assets/fonts/Roboto-Regular.ttf").to_vec(),
                fontdue::FontSettings::default(),
            )
            .ok()
        })
        .as_ref()
}

fn measure_toolbar_text(text: &str, ui_scale: f32) -> f32 {
    if let Some(font) = get_toolbar_font() {
        let px = 13.0 * ui_scale;
        text.chars()
            .map(|c| font.metrics(c, px).advance_width)
            .sum()
    } else {
        text.len() as f32 * 7.0 * ui_scale
    }
}

fn draw_toolbar_text(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    text: &str,
    r: u8,
    g: u8,
    b: u8,
    ui_scale: f32,
) {
    if let Some(font) = get_toolbar_font() {
        draw_toolbar_text_ttf(pixmap, x, y, text, r, g, b, font, ui_scale);
    } else {
        draw_toolbar_text_bitmap(pixmap, x, y, text, r, g, b, ui_scale);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_toolbar_text_ttf(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    text: &str,
    r: u8,
    g: u8,
    b: u8,
    font: &fontdue::Font,
    ui_scale: f32,
) {
    let px = 13.0 * ui_scale;
    let ascent = font
        .horizontal_line_metrics(px)
        .map(|m| m.ascent)
        .unwrap_or(px * 0.8);
    let mut cx = x;

    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, px);
        // Snap glyph origin to nearest pixel for sharper unhinted text.
        let glyph_x = (cx + metrics.bounds.xmin).round();
        // fontdue uses Y-up: ymin is the bottom edge relative to baseline.
        // Screen is Y-down, so the top of the glyph is at
        // baseline - (ymin + height) where baseline = y + ascent.
        let glyph_y = (y + ascent - metrics.bounds.ymin - metrics.height as f32).round();

        for (i, &coverage) in bitmap.iter().enumerate() {
            if coverage == 0 {
                continue;
            }
            let gx = (i % metrics.width) as i32;
            let gy = (i / metrics.width) as i32;
            let px = glyph_x as i32 + gx;
            let py = glyph_y as i32 + gy;
            if px >= 0 && py >= 0 {
                let px = px as u32;
                let py = py as u32;
                if px < pixmap.width() && py < pixmap.height() {
                    let w = pixmap.width();
                    let idx = ((py * w + px) * 4) as usize;
                    let data = pixmap.data_mut();
                    if idx + 3 < data.len() {
                        let sa = coverage as u32;
                        let inv = 255 - sa;
                        data[idx] = ((r as u32 * sa + data[idx] as u32 * inv) / 255) as u8;
                        data[idx + 1] = ((g as u32 * sa + data[idx + 1] as u32 * inv) / 255) as u8;
                        data[idx + 2] = ((b as u32 * sa + data[idx + 2] as u32 * inv) / 255) as u8;
                        data[idx + 3] = 255;
                    }
                }
            }
        }
        cx += metrics.advance_width;
    }
}

fn draw_toolbar_text_bitmap(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    text: &str,
    r: u8,
    g: u8,
    b: u8,
    ui_scale: f32,
) {
    let char_w = 7.0 * ui_scale;
    let scale = 0.75 * ui_scale;
    let mut cx = x;

    for ch in text.chars() {
        if ch == ' ' {
            cx += char_w;
            continue;
        }
        let segments = mini_glyph(ch);
        for (x1, y1, x2, y2) in segments {
            let sx = cx + x1 * scale;
            let sy = y + y1 * scale;
            let ex = cx + x2 * scale;
            let ey = y + y2 * scale;

            if (sx - ex).abs() < 0.5 {
                let min_y = sy.min(ey);
                let max_y = sy.max(ey);
                draw_toolbar_rect(pixmap, sx, min_y, 1.0, max_y - min_y, r, g, b);
            } else if (sy - ey).abs() < 0.5 {
                let min_x = sx.min(ex);
                let max_x = sx.max(ex);
                draw_toolbar_rect(pixmap, min_x, sy, max_x - min_x, 1.0, r, g, b);
            } else {
                let steps = ((ex - sx).abs().max((ey - sy).abs()) / 0.8) as u32;
                let steps = steps.max(2);
                for i in 0..steps {
                    let t = i as f32 / steps as f32;
                    let px = sx + (ex - sx) * t;
                    let py = sy + (ey - sy) * t;
                    draw_toolbar_rect(pixmap, px, py, 1.0, 1.0, r, g, b);
                }
            }
        }
        cx += char_w;
    }
}

fn mini_glyph(ch: char) -> Vec<(f32, f32, f32, f32)> {
    // Compact glyph definitions in a 8x14 grid
    match ch {
        'A' => vec![
            (1.0, 12.0, 4.0, 2.0),
            (4.0, 2.0, 7.0, 12.0),
            (2.5, 8.0, 5.5, 8.0),
        ],
        'a' => vec![
            (7.0, 5.0, 7.0, 12.0),
            (7.0, 5.0, 4.0, 5.0),
            (4.0, 5.0, 2.0, 7.0),
            (2.0, 7.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 12.0),
        ],
        'B' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 6.0, 7.0),
            (2.0, 7.0, 6.0, 7.0),
            (6.0, 7.0, 7.0, 9.0),
            (7.0, 9.0, 6.0, 12.0),
            (2.0, 12.0, 6.0, 12.0),
        ],
        'b' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 7.0, 5.0, 5.0),
            (5.0, 5.0, 7.0, 7.0),
            (7.0, 7.0, 7.0, 10.0),
            (7.0, 10.0, 5.0, 12.0),
            (2.0, 12.0, 5.0, 12.0),
        ],
        'C' => vec![
            (7.0, 3.0, 4.0, 2.0),
            (4.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 11.0),
        ],
        'c' => vec![
            (7.0, 6.0, 5.0, 5.0),
            (5.0, 5.0, 2.0, 7.0),
            (2.0, 7.0, 2.0, 10.0),
            (2.0, 10.0, 5.0, 12.0),
            (5.0, 12.0, 7.0, 11.0),
        ],
        'D' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 2.0, 5.0, 2.0),
            (5.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 7.0, 10.0),
            (7.0, 10.0, 5.0, 12.0),
            (2.0, 12.0, 5.0, 12.0),
        ],
        'd' => vec![
            (7.0, 2.0, 7.0, 12.0),
            (7.0, 7.0, 4.0, 5.0),
            (4.0, 5.0, 2.0, 7.0),
            (2.0, 7.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 12.0),
        ],
        'E' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 2.0, 7.0, 2.0),
            (2.0, 7.0, 6.0, 7.0),
            (2.0, 12.0, 7.0, 12.0),
        ],
        'e' => vec![
            (2.0, 8.0, 7.0, 8.0),
            (7.0, 8.0, 7.0, 6.0),
            (7.0, 6.0, 4.0, 5.0),
            (4.0, 5.0, 2.0, 7.0),
            (2.0, 7.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 11.0),
        ],
        'F' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 2.0, 7.0, 2.0),
            (2.0, 7.0, 6.0, 7.0),
        ],
        'f' => vec![
            (6.0, 3.0, 5.0, 2.0),
            (5.0, 2.0, 4.0, 4.0),
            (4.0, 4.0, 4.0, 12.0),
            (2.0, 6.0, 6.0, 6.0),
        ],
        'G' => vec![
            (7.0, 3.0, 4.0, 2.0),
            (4.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 10.0),
            (7.0, 10.0, 7.0, 7.0),
            (5.0, 7.0, 7.0, 7.0),
        ],
        'g' => vec![
            (2.0, 7.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 12.0),
            (7.0, 5.0, 7.0, 13.0),
            (7.0, 13.0, 5.0, 14.0),
            (5.0, 14.0, 2.0, 13.0),
            (7.0, 5.0, 4.0, 5.0),
            (4.0, 5.0, 2.0, 7.0),
        ],
        'H' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (7.0, 2.0, 7.0, 12.0),
            (2.0, 7.0, 7.0, 7.0),
        ],
        'h' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 7.0, 5.0, 5.0),
            (5.0, 5.0, 7.0, 7.0),
            (7.0, 7.0, 7.0, 12.0),
        ],
        'I' => vec![
            (3.0, 2.0, 6.0, 2.0),
            (4.5, 2.0, 4.5, 12.0),
            (3.0, 12.0, 6.0, 12.0),
        ],
        'i' => vec![(4.0, 3.0, 5.0, 4.0), (4.0, 6.0, 4.0, 12.0)],
        'J' => vec![
            (4.0, 2.0, 7.0, 2.0),
            (6.0, 2.0, 6.0, 10.0),
            (6.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 2.0, 10.0),
        ],
        'j' => vec![
            (5.0, 3.0, 6.0, 4.0),
            (5.0, 6.0, 5.0, 12.0),
            (5.0, 12.0, 3.0, 14.0),
            (3.0, 14.0, 2.0, 13.0),
        ],
        'K' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (7.0, 2.0, 2.0, 7.0),
            (2.0, 7.0, 7.0, 12.0),
        ],
        'k' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (7.0, 5.0, 2.0, 9.0),
            (2.0, 9.0, 7.0, 12.0),
        ],
        'L' => vec![(2.0, 2.0, 2.0, 12.0), (2.0, 12.0, 7.0, 12.0)],
        'l' => vec![(4.0, 2.0, 4.0, 12.0), (4.0, 12.0, 6.0, 12.0)],
        'M' => vec![
            (1.0, 12.0, 1.0, 2.0),
            (1.0, 2.0, 4.0, 7.0),
            (4.0, 7.0, 7.0, 2.0),
            (7.0, 2.0, 7.0, 12.0),
        ],
        'm' => vec![
            (1.0, 12.0, 1.0, 5.0),
            (1.0, 6.0, 3.0, 5.0),
            (3.0, 5.0, 4.0, 6.0),
            (4.0, 6.0, 4.0, 12.0),
            (4.0, 6.0, 6.0, 5.0),
            (6.0, 5.0, 7.0, 6.0),
            (7.0, 6.0, 7.0, 12.0),
        ],
        'N' => vec![
            (2.0, 12.0, 2.0, 2.0),
            (2.0, 2.0, 7.0, 12.0),
            (7.0, 12.0, 7.0, 2.0),
        ],
        'n' => vec![
            (2.0, 12.0, 2.0, 5.0),
            (2.0, 6.0, 5.0, 5.0),
            (5.0, 5.0, 7.0, 7.0),
            (7.0, 7.0, 7.0, 12.0),
        ],
        'O' => vec![
            (3.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 7.0, 10.0),
            (7.0, 10.0, 6.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (3.0, 12.0, 2.0, 10.0),
            (2.0, 10.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 2.0),
        ],
        'o' => vec![
            (3.0, 5.0, 6.0, 5.0),
            (6.0, 5.0, 7.0, 7.0),
            (7.0, 7.0, 7.0, 10.0),
            (7.0, 10.0, 6.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (3.0, 12.0, 2.0, 10.0),
            (2.0, 10.0, 2.0, 7.0),
            (2.0, 7.0, 3.0, 5.0),
        ],
        'P' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 6.0, 7.0),
            (2.0, 7.0, 6.0, 7.0),
        ],
        'p' => vec![
            (2.0, 5.0, 2.0, 14.0),
            (2.0, 7.0, 5.0, 5.0),
            (5.0, 5.0, 7.0, 7.0),
            (7.0, 7.0, 7.0, 10.0),
            (7.0, 10.0, 5.0, 12.0),
            (2.0, 12.0, 5.0, 12.0),
        ],
        'Q' => vec![
            (3.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 7.0, 10.0),
            (7.0, 10.0, 6.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (3.0, 12.0, 2.0, 10.0),
            (2.0, 10.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 2.0),
            (5.0, 9.0, 7.0, 13.0),
        ],
        'q' => vec![
            (7.0, 5.0, 7.0, 14.0),
            (7.0, 7.0, 4.0, 5.0),
            (4.0, 5.0, 2.0, 7.0),
            (2.0, 7.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 12.0),
        ],
        'R' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 6.0, 7.0),
            (2.0, 7.0, 6.0, 7.0),
            (4.0, 7.0, 7.0, 12.0),
        ],
        'r' => vec![
            (2.0, 5.0, 2.0, 12.0),
            (2.0, 6.0, 5.0, 5.0),
            (5.0, 5.0, 7.0, 6.0),
        ],
        'S' => vec![
            (7.0, 3.0, 4.0, 2.0),
            (4.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 6.0),
            (3.0, 6.0, 6.0, 8.0),
            (6.0, 8.0, 7.0, 10.0),
            (7.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 2.0, 11.0),
        ],
        's' => vec![
            (7.0, 6.0, 5.0, 5.0),
            (5.0, 5.0, 2.0, 7.0),
            (2.0, 7.0, 7.0, 10.0),
            (7.0, 10.0, 5.0, 12.0),
            (5.0, 12.0, 2.0, 11.0),
        ],
        'T' => vec![(1.0, 2.0, 8.0, 2.0), (4.5, 2.0, 4.5, 12.0)],
        't' => vec![
            (4.0, 2.0, 4.0, 10.0),
            (4.0, 10.0, 6.0, 12.0),
            (6.0, 12.0, 7.0, 12.0),
            (2.0, 6.0, 6.0, 6.0),
        ],
        'U' => vec![
            (2.0, 2.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 10.0),
            (7.0, 10.0, 7.0, 2.0),
        ],
        'u' => vec![
            (2.0, 5.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 12.0),
            (7.0, 5.0, 7.0, 12.0),
        ],
        'V' => vec![(1.0, 2.0, 4.0, 12.0), (4.0, 12.0, 7.0, 2.0)],
        'v' => vec![(2.0, 5.0, 4.5, 12.0), (4.5, 12.0, 7.0, 5.0)],
        'W' => vec![
            (0.0, 2.0, 2.0, 12.0),
            (2.0, 12.0, 4.0, 7.0),
            (4.0, 7.0, 6.0, 12.0),
            (6.0, 12.0, 8.0, 2.0),
        ],
        'w' => vec![
            (1.0, 5.0, 2.5, 12.0),
            (2.5, 12.0, 4.0, 7.0),
            (4.0, 7.0, 5.5, 12.0),
            (5.5, 12.0, 7.0, 5.0),
        ],
        'X' => vec![(2.0, 2.0, 7.0, 12.0), (7.0, 2.0, 2.0, 12.0)],
        'x' => vec![(2.0, 5.0, 7.0, 12.0), (7.0, 5.0, 2.0, 12.0)],
        'Y' => vec![
            (1.0, 2.0, 4.0, 7.0),
            (7.0, 2.0, 4.0, 7.0),
            (4.0, 7.0, 4.0, 12.0),
        ],
        'y' => vec![
            (2.0, 5.0, 4.5, 10.0),
            (7.0, 5.0, 4.5, 10.0),
            (4.5, 10.0, 3.0, 14.0),
        ],
        'Z' => vec![
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 2.0, 12.0),
            (2.0, 12.0, 7.0, 12.0),
        ],
        'z' => vec![
            (2.0, 5.0, 7.0, 5.0),
            (7.0, 5.0, 2.0, 12.0),
            (2.0, 12.0, 7.0, 12.0),
        ],
        '0' => vec![
            (3.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 7.0, 10.0),
            (7.0, 10.0, 6.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (3.0, 12.0, 2.0, 10.0),
            (2.0, 10.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 2.0),
        ],
        '1' => vec![
            (3.0, 4.0, 4.5, 2.0),
            (4.5, 2.0, 4.5, 12.0),
            (3.0, 12.0, 6.0, 12.0),
        ],
        '2' => vec![
            (2.0, 4.0, 3.0, 2.0),
            (3.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 7.0, 12.0),
        ],
        '3' => vec![
            (2.0, 3.0, 3.0, 2.0),
            (3.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 5.0, 7.0),
            (5.0, 7.0, 7.0, 10.0),
            (7.0, 10.0, 6.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (3.0, 12.0, 2.0, 11.0),
        ],
        '4' => vec![
            (6.0, 2.0, 2.0, 8.0),
            (2.0, 8.0, 7.0, 8.0),
            (6.0, 2.0, 6.0, 12.0),
        ],
        '5' => vec![
            (7.0, 2.0, 2.0, 2.0),
            (2.0, 2.0, 2.0, 6.0),
            (2.0, 6.0, 6.0, 6.0),
            (6.0, 6.0, 7.0, 9.0),
            (7.0, 9.0, 6.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (3.0, 12.0, 2.0, 11.0),
        ],
        '6' => vec![
            (6.0, 2.0, 3.0, 2.0),
            (3.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 10.0),
            (2.0, 10.0, 3.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (6.0, 12.0, 7.0, 10.0),
            (7.0, 10.0, 7.0, 7.0),
            (7.0, 7.0, 6.0, 6.0),
            (2.0, 6.0, 6.0, 6.0),
        ],
        '7' => vec![(2.0, 2.0, 7.0, 2.0), (7.0, 2.0, 4.0, 12.0)],
        '8' => vec![
            (3.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 6.0, 6.0),
            (3.0, 6.0, 6.0, 6.0),
            (3.0, 6.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 2.0),
            (3.0, 6.0, 2.0, 9.0),
            (2.0, 9.0, 3.0, 12.0),
            (3.0, 12.0, 6.0, 12.0),
            (6.0, 12.0, 7.0, 9.0),
            (7.0, 9.0, 6.0, 6.0),
        ],
        '9' => vec![
            (7.0, 6.0, 6.0, 2.0),
            (6.0, 2.0, 3.0, 2.0),
            (3.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 5.0),
            (2.0, 5.0, 3.0, 7.0),
            (3.0, 7.0, 7.0, 7.0),
            (7.0, 2.0, 7.0, 10.0),
            (7.0, 10.0, 4.0, 12.0),
        ],
        '.' => vec![
            (4.0, 11.0, 5.0, 11.0),
            (4.0, 11.0, 4.0, 12.0),
            (5.0, 11.0, 5.0, 12.0),
            (4.0, 12.0, 5.0, 12.0),
        ],
        ',' => vec![(4.5, 10.0, 4.5, 12.0), (4.5, 12.0, 3.5, 13.0)],
        ':' => vec![(4.0, 4.0, 5.0, 5.0), (4.0, 10.0, 5.0, 11.0)],
        '/' => vec![(7.0, 2.0, 2.0, 12.0)],
        '\\' => vec![(2.0, 2.0, 7.0, 12.0)],
        '-' => vec![(2.0, 7.0, 7.0, 7.0)],
        '_' => vec![(1.0, 12.0, 8.0, 12.0)],
        '=' => vec![(2.0, 5.0, 7.0, 5.0), (2.0, 9.0, 7.0, 9.0)],
        '?' => vec![
            (2.0, 3.0, 3.0, 2.0),
            (3.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 7.0, 4.0),
            (7.0, 4.0, 4.5, 7.0),
            (4.5, 7.0, 4.5, 9.0),
            (4.0, 11.0, 5.0, 12.0),
        ],
        '!' => vec![(4.5, 2.0, 4.5, 9.0), (4.0, 11.0, 5.0, 12.0)],
        '<' => vec![(7.0, 3.0, 2.0, 7.0), (2.0, 7.0, 7.0, 11.0)],
        '>' => vec![(2.0, 3.0, 7.0, 7.0), (7.0, 7.0, 2.0, 11.0)],
        '(' => vec![
            (5.0, 1.0, 3.0, 4.0),
            (3.0, 4.0, 3.0, 10.0),
            (3.0, 10.0, 5.0, 13.0),
        ],
        ')' => vec![
            (3.0, 1.0, 5.0, 4.0),
            (5.0, 4.0, 5.0, 10.0),
            (5.0, 10.0, 3.0, 13.0),
        ],
        '[' => vec![
            (3.0, 1.0, 6.0, 1.0),
            (3.0, 1.0, 3.0, 13.0),
            (3.0, 13.0, 6.0, 13.0),
        ],
        ']' => vec![
            (3.0, 1.0, 6.0, 1.0),
            (6.0, 1.0, 6.0, 13.0),
            (3.0, 13.0, 6.0, 13.0),
        ],
        '#' => vec![
            (3.0, 3.0, 3.0, 11.0),
            (6.0, 3.0, 6.0, 11.0),
            (1.0, 5.0, 8.0, 5.0),
            (1.0, 9.0, 8.0, 9.0),
        ],
        '@' => vec![
            (7.0, 4.0, 4.0, 2.0),
            (4.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 10.0),
            (5.0, 5.0, 5.0, 9.0),
            (5.0, 9.0, 7.0, 9.0),
            (7.0, 4.0, 7.0, 9.0),
        ],
        '&' => vec![
            (5.0, 2.0, 3.0, 2.0),
            (3.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 6.0),
            (3.0, 6.0, 2.0, 10.0),
            (2.0, 10.0, 4.0, 12.0),
            (4.0, 12.0, 7.0, 10.0),
            (3.0, 6.0, 7.0, 9.0),
        ],
        '+' => vec![(4.5, 3.0, 4.5, 11.0), (2.0, 7.0, 7.0, 7.0)],
        '*' => vec![
            (4.5, 3.0, 4.5, 11.0),
            (2.0, 5.0, 7.0, 9.0),
            (2.0, 9.0, 7.0, 5.0),
        ],
        '%' => vec![
            (2.0, 2.0, 3.0, 3.0),
            (7.0, 2.0, 2.0, 12.0),
            (6.0, 11.0, 7.0, 12.0),
        ],
        '^' => vec![(2.0, 5.0, 4.5, 2.0), (4.5, 2.0, 7.0, 5.0)],
        '~' => vec![
            (1.0, 7.0, 3.0, 5.0),
            (3.0, 5.0, 5.0, 7.0),
            (5.0, 7.0, 7.0, 5.0),
        ],
        '|' => vec![(4.5, 1.0, 4.5, 13.0)],
        '\'' | '"' => vec![(4.0, 2.0, 4.0, 4.0)],
        '{' => vec![
            (5.0, 1.0, 4.0, 2.0),
            (4.0, 2.0, 4.0, 5.0),
            (4.0, 5.0, 3.0, 7.0),
            (3.0, 7.0, 4.0, 9.0),
            (4.0, 9.0, 4.0, 12.0),
            (4.0, 12.0, 5.0, 13.0),
        ],
        '}' => vec![
            (4.0, 1.0, 5.0, 2.0),
            (5.0, 2.0, 5.0, 5.0),
            (5.0, 5.0, 6.0, 7.0),
            (6.0, 7.0, 5.0, 9.0),
            (5.0, 9.0, 5.0, 12.0),
            (5.0, 12.0, 4.0, 13.0),
        ],
        '$' => vec![
            (6.0, 3.0, 3.0, 3.0),
            (3.0, 3.0, 2.0, 5.0),
            (2.0, 5.0, 6.0, 8.0),
            (6.0, 8.0, 7.0, 10.0),
            (7.0, 10.0, 3.0, 12.0),
            (4.5, 1.0, 4.5, 13.0),
        ],
        '`' => vec![(3.0, 2.0, 5.0, 4.0)],
        _ => vec![
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 7.0, 12.0),
            (7.0, 12.0, 2.0, 12.0),
            (2.0, 12.0, 2.0, 2.0),
        ],
    }
}

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let mcp_mode = args.iter().any(|a| a == "--mcp");

    // Parse URL arg (skip --mcp)
    let url_arg = args.iter().skip(1).find(|a| *a != "--mcp");
    let (initial_url, initial_html) = if let Some(input) = url_arg {
        match fetch_url_solving_challenges(input) {
            Ok(resp) => (resp.url, resp.body),
            Err(e) => {
                eprintln!("Failed to load {input}: {e}");
                ("about:blank".into(), default_html().to_string())
            }
        }
    } else {
        ("about:home".into(), default_html().to_string())
    };

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new(initial_url, initial_html);

    if mcp_mode {
        let proxy = event_loop.create_proxy();
        let bridge = DevToolsBridge::new(Box::new(move || {
            let _ = proxy.send_event(());
        }));
        app.devtools = Some(bridge.clone());

        std::thread::spawn(move || {
            incognidium_devtools::run_mcp_server(bridge);
        });

        eprintln!("Incognidium MCP server started on stdio");
    }

    event_loop.run_app(&mut app).expect("event loop failed");
}

/// Extract URLs from `background-image: url(...)` declarations in CSS text.
/// Returns the raw URL string (without quotes) and the cleaned URL.
fn extract_css_image_urls(css: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let lower = css.to_ascii_lowercase();
    let mut start = 0usize;
    // Match both `background-image: url(...)` and the shorthand
    // `background: ... url(...) ...` declarations.
    while let Some(pos) = lower[start..].find("url(") {
        let pos = start + pos;
        // Walk back to the start of the declaration so we can skip data: URLs
        // that appear inside shorthand background values.
        let decl_start = lower[..pos].rfind(';').map(|p| p + 1).unwrap_or(0);
        let decl_end = lower[pos..]
            .find(';')
            .map(|p| pos + p)
            .unwrap_or(lower.len());
        let _decl = &css[decl_start..decl_end];

        let url_start = pos + 4;
        let rest = &css[url_start..];
        let rest = rest.trim_start();
        let (open, close) = if rest.starts_with('"') {
            ('"', '"')
        } else if rest.starts_with('\'') {
            ('\'', '\'')
        } else {
            (' ', ')')
        };
        let rest = if open == ' ' { rest } else { &rest[1..] };
        if let Some(end_idx) = rest.find(close) {
            let raw = rest[..end_idx].trim().to_string();
            if !raw.is_empty() && !raw.starts_with("data:") {
                out.push((raw.clone(), raw));
            }
        }
        start = pos + 1;
    }
    out
}

fn default_html() -> &'static str {
    r#"<!DOCTYPE html>
<html>
<head>
<style>
body {
    font-size: 16px;
    color: #222;
    background: white;
}
h1 {
    color: #1a1a2e;
    font-size: 36px;
}
h2 {
    color: #16213e;
    font-size: 24px;
}
p {
    color: #333;
}
.container {
    padding: 20px;
}
.flex-row {
    display: flex;
    gap: 16px;
}
.card {
    background-color: #e8f4fd;
    padding: 16px;
    flex-grow: 1;
    border-width: 2px;
    border-color: #1a1a2e;
}
.card-red {
    background-color: #fde8e8;
    padding: 16px;
    flex-grow: 1;
    border-width: 2px;
    border-color: #8b0000;
}
.card-green {
    background-color: #e8fde8;
    padding: 16px;
    flex-grow: 1;
    border-width: 2px;
    border-color: #006400;
}
.highlight {
    background-color: #fff3cd;
    padding: 10px;
}
.red { color: red; }
.blue { color: blue; }
.green { color: green; }
</style>
</head>
<body>
<div class="container">
    <h1>Incognidium Browser Engine</h1>
    <p>A custom HTML/CSS rendering engine written in Rust.</p>
    <h2>Features</h2>
    <div class="flex-row">
        <div class="card">
            <h3>HTML5 Parsing</h3>
            <p>Full HTML5 parsing via html5ever.</p>
        </div>
        <div class="card-red">
            <h3>CSS Styling</h3>
            <p>Selector matching, cascade, inheritance.</p>
        </div>
        <div class="card-green">
            <h3>Layout Engine</h3>
            <p>Block, inline, and flexbox layout.</p>
        </div>
    </div>
    <div class="highlight">
        <p>This page is rendered entirely by Incognidium!</p>
    </div>
    <h2>Links</h2>
    <p><a href="https://example.com">Click here to visit example.com</a></p>
    <h2>Color Test</h2>
    <p class="red">This text should be red.</p>
    <p class="blue">This text should be blue.</p>
    <p class="green">This text should be green.</p>
</div>
</body>
</html>"#
}
