use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use arboard::Clipboard;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::ModifiersState;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Fullscreen, Icon, Window, WindowId};

use incognidium_chrome::{
    build_local_page, load_bookmarks, load_history, load_settings, save_bookmarks, save_history,
    save_settings, Bookmark, HistoryEntry, Settings,
};
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
use incognidium_shell::engine_bridge::{take_pending_form_submission, FormSubmission};

use incognidium_shell::{
    collect_form_fields, collect_scripts, decode_and_downscale_image, encode_png_compressed,
    input_value, propagate_canvas_background, resolve_styles_with_container_sizes,
    save_png_compressed, set_input_value,
};

const DEFAULT_WIDTH: u32 = 1400;
const DEFAULT_HEIGHT: u32 = 900;
const TOOLBAR_HEIGHT: u32 = 40;
const MENU_BAR_HEIGHT: u32 = 32;
const CHROME_HEIGHT: u32 = TOOLBAR_HEIGHT + MENU_BAR_HEIGHT;
const ADDR_BAR_LEFT: f32 = 90.0;
const ADDR_BAR_TOP: f32 = 6.0;
const ADDR_BAR_HEIGHT: f32 = 28.0;
const ADDR_BAR_RIGHT_MARGIN: f32 = 12.0;
const BTN_SIZE: f32 = 28.0;
const BTN_Y: f32 = 6.0;

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

fn resolve_search_or_url(settings: &Settings, input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "about:blank".to_string();
    }
    if trimmed.starts_with("about:")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://")
    {
        return trimmed.to_string();
    }
    let has_space = trimmed.contains(' ');
    let looks_like_url =
        !has_space && (trimmed.contains('.') || trimmed.eq_ignore_ascii_case("localhost"));
    if looks_like_url {
        return format!("http://{trimmed}");
    }
    let encoded = urlencoding::encode(trimmed).into_owned();
    if settings.search_engine_url.contains("%s") {
        settings.search_engine_url.replace("%s", &encoded)
    } else {
        format!(
            "{}?q={}",
            settings.search_engine_url.trim_end_matches('/'),
            encoded
        )
    }
}

fn parse_settings_query(url: &str) -> (Option<String>, Option<String>) {
    let Some(qs) = url.splitn(2, '?').nth(1) else {
        return (None, None);
    };
    let mut search = None;
    let mut home = None;
    for pair in qs.split('&') {
        let mut it = pair.splitn(2, '=');
        let key = it.next().unwrap_or("");
        let val = it.next().unwrap_or("");
        if let Ok(decoded) = urlencoding::decode(val) {
            match key {
                "search_engine_url" => search = Some(decoded.into_owned()),
                "homepage" => home = Some(decoded.into_owned()),
                _ => {}
            }
        }
    }
    (search, home)
}

fn record_history_entry(history: &mut Vec<HistoryEntry>, url: &str, title: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if history.last().map(|e| e.url.as_str()) == Some(url) {
        return;
    }
    if history.len() > 5000 {
        history.remove(0);
    }
    history.push(HistoryEntry {
        url: url.to_string(),
        title: title.to_string(),
        visited_at: now,
    });
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
    max_scroll_y: f32,

    // Focused form control
    focused_input: Option<incognidium_dom::NodeId>,
    input_cursor: usize,

    // JS engine
    js_vm: murkiu_vm::Vm,

    // Mouse
    last_cursor: Option<(f64, f64)>,

    // Address bar selection state
    address_all_selected: bool,

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

    // User-configurable browser settings
    settings: Settings,

    // Async image loading
    pending_images: Arc<Mutex<Vec<(String, ImageData)>>>,
    images_loading: Arc<AtomicBool>,
    images_cancel: Arc<AtomicBool>,

    // DOM document modified by JavaScript execution
    js_modified_doc: Option<incognidium_dom::Document>,

    // Screenshot requested via F12; saved during next render
    pending_screenshot: Option<String>,

    // URL preview shown when hovering a link
    hover_url: Option<String>,

    // Status message shown during navigation / loading
    status_message: Option<String>,

    // System clipboard for copy/paste in the address bar
    clipboard: Option<Clipboard>,

    // Current keyboard modifiers (Ctrl, Alt, etc.)
    modifiers: ModifiersState,

    // User bookmarks
    bookmarks: Vec<Bookmark>,

    // Persistent browser history
    history_entries: Vec<HistoryEntry>,

    // Active menu-bar dropdown (index of top-level menu)
    menu_active: Option<usize>,
    menu_item_hover: Option<usize>,

    // Fullscreen state
    is_fullscreen: bool,

    // Animation frame counter for toolbar spinners
    frame_count: u32,
    zoom_factor: f32,
    zoom_flash: Option<(f32, Instant)>,
}

/// Cached results from parse -> style -> layout pipeline.
struct CachedLayout {
    doc: incognidium_dom::Document,
    styles: incognidium_style::StyleMap,
    layout_root: Box<incognidium_layout::LayoutBox>,
}

impl App {
    fn user_data(&self) -> incognidium_chrome::UserData {
        incognidium_chrome::UserData {
            settings: self.settings.clone(),
            bookmarks: self.bookmarks.clone(),
            history: self.history_entries.clone(),
        }
    }

    fn new(initial_url: String, initial_html: String, settings: Settings) -> Self {
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
            max_scroll_y: 0.0,
            focused_input: None,
            input_cursor: 0,
            last_cursor: None,
            js_vm: murkiu_vm::Vm::new(),
            settings,
            image_cache: std::collections::HashMap::new(),
            flat_boxes: Vec::new(),
            address_all_selected: false,
            external_css: String::new(),
            cached_layout: None,
            layout_dirty: true,
            last_layout_width: 0,
            devtools: None,
            pending_images: Arc::new(Mutex::new(Vec::new())),
            images_loading: Arc::new(AtomicBool::new(false)),
            images_cancel: Arc::new(AtomicBool::new(false)),
            js_modified_doc: None,
            pending_screenshot: None,
            hover_url: None,
            status_message: None,
            modifiers: ModifiersState::empty(),
            bookmarks: load_bookmarks(),
            history_entries: load_history(),
            menu_active: None,
            menu_item_hover: None,
            is_fullscreen: false,
            frame_count: 0,
            zoom_factor: 1.0,
            zoom_flash: None,
            clipboard: Clipboard::new().ok(),
        }
    }

    fn navigate(&mut self, url_input: &str) {
        let url_str = url_input.to_string();
        self.status_message = Some(format!("Loading {}...", url_str));
        self.request_redraw();

        // Handle page source view (uses current html_content)
        if url_str == "about:source" {
            let html = incognidium_chrome::build_source_page(&self.html_content);
            self.show_local_page_source(&html);
            return;
        }

        // Handle home-page search submissions before any other navigation.
        if url_str.starts_with("about:home/search") {
            if let Some(qs) = url_str.splitn(2, '?').nth(1) {
                for pair in qs.split('&') {
                    let mut it = pair.splitn(2, '=');
                    if it.next() == Some("q") {
                        if let Ok(decoded) = urlencoding::decode(it.next().unwrap_or("")) {
                            let query = decoded.into_owned();
                            let resolved = resolve_search_or_url(&self.settings, &query);
                            self.navigate(&resolved);
                            return;
                        }
                    }
                }
            }
            self.navigate("about:home");
            return;
        }
        // Handle settings form submissions before any other navigation.
        if url_str.starts_with("about:settings/set") {
            if let (Some(new_search), Some(new_home)) = parse_settings_query(&url_str) {
                self.settings.search_engine_url = new_search;
                self.settings.homepage = new_home;
                if let Err(e) = save_settings(&self.settings) {
                    eprintln!("Failed to save settings: {e}");
                }
            }
            self.navigate("about:settings");
            return;
        }

        if build_local_page(&url_str, &self.user_data(), Some(&self.html_content)).is_some() {
            if self.history_pos + 1 < self.history.len() {
                self.history.truncate(self.history_pos + 1);
            }
            self.history.push(url_str.clone());
            self.history_pos = self.history.len() - 1;
            self.show_local_page(&url_str);
            return;
        }

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
                self.update_window_title();

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
                self.record_current_history();

                // Fetch external CSS from <link> tags
                self.fetch_external_css(&resp.url, &resp.body);

                // Execute <script> tags
                self.execute_scripts();
                self.layout_dirty = true;
                self.status_message = None;

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
        self.status_message = None;

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
        if self.show_local_page(url) {
            return;
        }
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

                self.html_content = resp.body.clone();
                self.current_url = resp.url.clone();
                self.address_text = resp.url.clone();
                self.cursor_pos = self.address_text.len();
                self.scroll_y = 0.0;
                self.update_window_title();
                self.js_modified_doc = None;
                self.cached_layout = None;
                self.fetch_external_css(&resp.url, &resp.body);
                self.execute_scripts();
                self.layout_dirty = true;
                self.status_message = None;

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
        self.status_message = None;
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
        let cancel = self.images_cancel.clone();
        let referer = base_url.to_string();
        loading.store(true, Ordering::SeqCst);

        // Clear pending buffer
        pending.lock().unwrap().clear();

        std::thread::spawn(move || {
            // Limit concurrent subresource fetches to avoid triggering CDN
            // rate-limiters. A small gap between chunks is polite without
            // materially slowing most pages.
            for (i, chunk) in urls.chunks(4).enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
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

    /// Show a built-in `about:` page without touching the network stack.
    /// Returns false when the URL is not a known local page.
    fn show_local_page_source(&mut self, html: &str) {
        self.html_content = html.to_string();
        self.current_url = "about:source".to_string();
        self.address_text = "about:source".to_string();
        self.cursor_pos = self.address_text.len();
        self.scroll_y = 0.0;
        self.image_cache.clear();
        self.external_css.clear();
        self.js_modified_doc = None;
        self.cached_layout = None;
        self.layout_dirty = true;
        self.update_window_title();
        self.render();
        self.request_redraw();
    }

    fn show_local_page(&mut self, url: &str) -> bool {
        let Some(html) = build_local_page(url, &self.user_data(), Some(&self.html_content)) else {
            return false;
        };
        self.html_content = html;
        self.current_url = url.to_string();
        self.address_text = url.to_string();
        self.cursor_pos = self.address_text.len();
        self.scroll_y = 0.0;
        self.update_window_title();
        self.image_cache.clear();
        self.external_css.clear();
        self.js_modified_doc = None;
        self.cached_layout = None;
        self.layout_dirty = true;
        self.render();
        self.request_redraw();
        true
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
            // If the script requested a form submission (e.g. an anti-bot
            // challenge), perform the HTTP POST and navigate to the result.
            if let Some(submission) = take_pending_form_submission() {
                self.submit_form(&submission);
            }
        }
    }

    fn handle_input_key(&mut self, event: &KeyEvent) -> bool {
        let node_id = match self.focused_input {
            Some(id) => id,
            None => return false,
        };
        let doc = match &mut self.js_modified_doc {
            Some(d) => d,
            None => {
                self.focused_input = None;
                return false;
            }
        };

        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                if let Some(form_id) = self.find_enclosing_form(node_id) {
                    self.submit_dom_form(form_id);
                }
                return true;
            }
            Key::Named(NamedKey::Escape) => {
                self.focused_input = None;
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::Backspace) => {
                let value = input_value(doc, node_id);
                if self.input_cursor > 0 {
                    let mut chars: Vec<char> = value.chars().collect();
                    chars.remove(self.input_cursor - 1);
                    let new_value: String = chars.into_iter().collect();
                    set_input_value(doc, node_id, &new_value);
                    self.input_cursor -= 1;
                    self.layout_dirty = true;
                    self.render();
                    self.request_redraw();
                }
                return true;
            }
            Key::Named(NamedKey::Delete) => {
                let value = input_value(doc, node_id);
                let mut chars: Vec<char> = value.chars().collect();
                if self.input_cursor < chars.len() {
                    chars.remove(self.input_cursor);
                    let new_value: String = chars.into_iter().collect();
                    set_input_value(doc, node_id, &new_value);
                    self.layout_dirty = true;
                    self.render();
                    self.request_redraw();
                }
                return true;
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if self.input_cursor > 0 {
                    self.input_cursor -= 1;
                    self.request_redraw();
                }
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => {
                let value = input_value(doc, node_id);
                if self.input_cursor < value.chars().count() {
                    self.input_cursor += 1;
                    self.request_redraw();
                }
                return true;
            }
            Key::Named(NamedKey::Home) => {
                self.input_cursor = 0;
                self.request_redraw();
                return true;
            }
            Key::Named(NamedKey::End) => {
                let value = input_value(doc, node_id);
                self.input_cursor = value.chars().count();
                self.request_redraw();
                return true;
            }
            Key::Character(ref ch) if self.modifiers.is_empty() => {
                let value = input_value(doc, node_id);
                let mut chars: Vec<char> = value.chars().collect();
                for c in ch.chars() {
                    if c >= ' ' && c != '\x7f' {
                        chars.insert(self.input_cursor.min(chars.len()), c);
                        self.input_cursor += 1;
                    }
                }
                let new_value: String = chars.into_iter().collect();
                set_input_value(doc, node_id, &new_value);
                self.layout_dirty = true;
                self.render();
                self.request_redraw();
                return true;
            }
            _ => {}
        }
        false
    }

    fn submit_form(&mut self, submission: &FormSubmission) {
        eprintln!("[chrome] submit_form: {:?}", submission);
        let request = match incognidium_shell::form_submission_to_fetch_request(
            submission,
            &self.current_url,
        ) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("Cannot resolve form action: {}", e);
                return;
            }
        };
        let action_url = request.url.clone();
        match incognidium_net::fetch_with_options(request) {
            Ok(resp) => {
                eprintln!(
                    "[chrome] form submission response: status={} len={} url={}",
                    resp.status,
                    resp.body.len(),
                    resp.url
                );
                if !resp.body.is_empty() {
                    let preview = &resp.body[..resp.body.len().min(512)];
                    eprintln!(
                        "[chrome] form submission body preview: {}",
                        preview.replace('\n', " ")
                    );
                }
                // Optionally log the post-challenge response body for analysis.
                if std::env::var_os("INCOGNIDIUM_DUMP_CHALLENGE_HTML").is_some() {
                    let _ = std::fs::write("/tmp/incognidium_after_challenge.html", &resp.body);
                }
                self.status_message = Some(format!("Form submission: HTTP {}", resp.status));
                self.history.push(self.current_url.clone());
                self.history_pos = self.history.len().saturating_sub(1);
                self.current_url = resp.url.clone();
                self.address_text = self.current_url.clone();
                self.address_all_selected = false;
                self.html_content = resp.body.clone();
                self.external_css.clear();
                self.images_loading.store(false, Ordering::SeqCst);
                self.images_cancel.store(true, Ordering::SeqCst);
                self.pending_images.lock().unwrap().clear();
                self.flat_boxes.clear();
                self.cached_layout = None;
                self.layout_dirty = true;
                self.scroll_y = 0.0;
                self.execute_scripts();
                self.render();
                self.request_redraw();
            }
            Err(e) => {
                eprintln!("Form submission failed for {}: {}", action_url, e);
            }
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
        let scale = window.scale_factor() as f32 * self.zoom_factor;
        // Use physical size for the window buffer, logical size for layout/paint
        let phys_width = size.width.max(1);
        let phys_height = size.height.max(1);
        let (cursor_x, cursor_y) = self
            .last_cursor
            .map(|(x, y)| (x as f32 / scale, y as f32 / scale))
            .unwrap_or((-1.0, -1.0));
        let (phys_cursor_x, phys_cursor_y) = self
            .last_cursor
            .map(|(x, y)| (x as f32, y as f32))
            .unwrap_or((-1.0, -1.0));
        let toolbar_top = MENU_BAR_HEIGHT as f32 + BTN_Y;
        let hover_back = cursor_y >= toolbar_top
            && cursor_y <= toolbar_top + BTN_SIZE
            && cursor_x >= 6.0
            && cursor_x <= 6.0 + BTN_SIZE;
        let hover_fwd = cursor_y >= toolbar_top
            && cursor_y <= toolbar_top + BTN_SIZE
            && cursor_x >= 36.0
            && cursor_x <= 36.0 + BTN_SIZE;
        let hover_reload = cursor_y >= toolbar_top
            && cursor_y <= toolbar_top + BTN_SIZE
            && cursor_x >= 66.0
            && cursor_x <= 66.0 + BTN_SIZE;
        // No hamburger menu; menu bar handles all actions.
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

        // Flatten with current scroll offset (cheap) — logical coordinates.
        // The flatten pass also hoists high-z-index interactive overlays (e.g.
        // modals rendered after a full-page background) into the viewport.
        let flat_boxes = flatten_layout(&cached.layout_root, 0.0, -self.scroll_y, &cached.styles);
        self.flat_boxes = flat_boxes.clone();

        // Keep the allowed scroll range in sync with the laid-out page height
        // so the user can always scroll to the actual content.
        let page_height = cached.layout_root.height.max(0.0);
        let viewport_height = height.saturating_sub(CHROME_HEIGHT) as f32;
        self.max_scroll_y = (page_height - viewport_height).max(0.0);
        if self.scroll_y > self.max_scroll_y {
            self.scroll_y = self.max_scroll_y;
        }

        // DEBUG: dump visible layout boxes for diagnosis
        {
            let mut out = String::new();
            out.push_str(&format!(
                "layout dump: url={} boxes={}\n",
                self.current_url,
                flat_boxes.len()
            ));
            for (i, b) in flat_boxes.iter().enumerate() {
                let text = b.text.as_deref().unwrap_or("");
                let preview: String = text.chars().take(80).collect();
                out.push_str(&format!(
                    "[{}] {:?} pos=({:.1},{:.1}) size=({:.1}x{:.1}) text={:?} img={:?} link={:?} clip={:?}\n",
                    i, b.box_type, b.x, b.y, b.width, b.height,
                    preview,
                    b.image_src.as_deref().unwrap_or(""),
                    b.link_href.as_deref().unwrap_or(""),
                    b.clip
                ));
            }
            let _ = std::fs::write("/tmp/incognidium_layout_dump.txt", out);
        }

        // Scale flat boxes and styles for physical-resolution painting        // Scale flat boxes and styles for physical-resolution painting        // Scale flat boxes and styles for physical-resolution painting
        let scaled_flat_boxes = scale_flat_boxes(&flat_boxes, scale);
        let scaled_styles = scale_styles(&cached.styles, scale);
        let chrome_h = (CHROME_HEIGHT as f32 * scale) as u32;
        let phys_page_height = phys_height.saturating_sub(chrome_h);

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
        let mbh = MENU_BAR_HEIGHT as f32 * scale;

        // Menu bar at the very top
        draw_toolbar_rect(
            &mut full,
            0.0,
            0.0,
            phys_width as f32,
            mbh,
            0xf7,
            0xf7,
            0xf7,
        );
        draw_toolbar_rect(
            &mut full,
            0.0,
            mbh - 1.0,
            phys_width as f32,
            1.0,
            0xdd,
            0xdd,
            0xdd,
        );
        Self::draw_menu_bar(
            &mut full,
            scale,
            self.menu_active,
            phys_cursor_x,
            phys_cursor_y,
        );

        // Toolbar below the menu bar
        self.frame_count = self.frame_count.wrapping_add(1);
        draw_toolbar_rect(&mut full, 0.0, mbh, phys_width as f32, th, 0xf0, 0xf0, 0xf0);
        // Toolbar bottom border
        draw_toolbar_rect(
            &mut full,
            0.0,
            mbh + th - 1.0,
            phys_width as f32,
            1.0,
            0xcc,
            0xcc,
            0xcc,
        );

        let frame_angle = (self.frame_count % 60) as f32 * 6.0;
        let can_back = self.history_pos > 0;
        draw_icon_button(
            &mut full,
            6.0 * scale,
            (MENU_BAR_HEIGHT as f32 + BTN_Y) * scale,
            ButtonIcon::Back,
            can_back,
            hover_back,
            scale,
            frame_angle,
        );
        let can_fwd = self.history_pos + 1 < self.history.len();
        draw_icon_button(
            &mut full,
            36.0 * scale,
            (MENU_BAR_HEIGHT as f32 + BTN_Y) * scale,
            ButtonIcon::Forward,
            can_fwd,
            hover_fwd,
            scale,
            frame_angle,
        );
        let reloading = self.images_loading.load(Ordering::SeqCst);
        let reload_icon = if reloading {
            ButtonIcon::Loading
        } else {
            ButtonIcon::Reload
        };
        draw_icon_button(
            &mut full,
            66.0 * scale,
            (MENU_BAR_HEIGHT as f32 + BTN_Y) * scale,
            reload_icon,
            true,
            hover_reload,
            scale,
            frame_angle,
        );

        // Address bar
        let addr_width = phys_width as f32 - ADDR_BAR_LEFT * scale - ADDR_BAR_RIGHT_MARGIN * scale;
        let is_https = self.current_url.starts_with("https://");
        draw_address_bar(
            &mut full,
            ADDR_BAR_LEFT * scale,
            (MENU_BAR_HEIGHT as f32 + ADDR_BAR_TOP) * scale,
            addr_width,
            ADDR_BAR_HEIGHT * scale,
            &self.address_text,
            self.address_focused,
            self.cursor_pos,
            self.address_all_selected,
            is_https,
            scale,
        );

        // Loading bar under toolbar when async resources are still fetching
        if reloading {
            let bar_y = mbh + th - 3.0 * scale;
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
                let dst = (((y + chrome_h) * phys_width + x) * 4) as usize;
                if src + 3 < page_data.len() && dst + 3 < full_data.len() {
                    full_data[dst] = page_data[src];
                    full_data[dst + 1] = page_data[src + 1];
                    full_data[dst + 2] = page_data[src + 2];
                    full_data[dst + 3] = page_data[src + 3];
                }
            }
        }

        // Draw menu dropdown on top of page content
        if let Some(idx) = self.menu_active {
            let layout = Self::menu_bar_layout(scale);
            Self::draw_menu_dropdown(
                &mut full,
                scale,
                idx,
                &layout,
                phys_cursor_x,
                phys_cursor_y,
                self.menu_item_hover,
            );
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

        // Show link URL preview or loading status at the bottom-left
        let status_text = self.status_message.as_deref().or(self.hover_url.as_deref());
        if let Some(text) = status_text {
            let bar_h = 20.0 * scale;
            let pad = 8.0 * scale;
            let label = if text.len() > 120 {
                format!("{}…", &text[..119])
            } else {
                text.to_string()
            };
            let text_w = measure_toolbar_text(&label, scale);
            let bar_w = (text_w + pad * 2.0).min(phys_width as f32);
            let bar_x = 0.0;
            let bar_y = phys_height as f32 - bar_h;
            draw_toolbar_rect(&mut full, bar_x, bar_y, bar_w, bar_h, 0x33, 0x33, 0x33);
            draw_toolbar_text(
                &mut full,
                bar_x + pad,
                bar_y + bar_h * 0.68,
                &label,
                0xff,
                0xff,
                0xff,
                scale,
            );
        }
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
            .unwrap_or(1.0)
            * self.zoom_factor;
        let phys_x = x as f32;
        let phys_y = y as f32;

        // If a menu dropdown is open, handle (or close) it first so clicks don't
        // pass through to the address bar or page content behind the menu.
        if let Some(idx) = self.menu_active {
            let layout = Self::menu_bar_layout(scale);
            let (x0, label_w, _label, items) = layout[idx];
            let line_h = 22.0 * scale;
            let pad = 10.0 * scale;
            let max_text_w = items
                .iter()
                .map(|item| measure_toolbar_text(item, scale))
                .fold(0.0, f32::max);
            let dw = (max_text_w + pad * 2.0).max(label_w);
            let dh = (items.len() as f32 * (line_h + 4.0) + 16.0) * scale;
            let dx = x0;
            let dy = MENU_BAR_HEIGHT as f32 * scale + 2.0 * scale;
            let inside_dropdown =
                phys_x >= dx && phys_x <= dx + dw && phys_y >= dy && phys_y <= dy + dh;
            if inside_dropdown {
                let rel_y = phys_y - dy - 8.0 * scale;
                let item_h = line_h + 4.0 * scale;
                let item = (rel_y / item_h).floor() as usize;
                if item < items.len() {
                    self.execute_menu_item(items[item]);
                }
                self.menu_active = None;
                self.menu_item_hover = None;
                self.request_redraw();
                return;
            }
            // Click outside the dropdown closes it and is consumed so it does not
            // activate controls behind the menu.
            self.menu_active = None;
            self.menu_item_hover = None;
            self.request_redraw();
            // If the click landed on a menu-bar label, open that menu instead.
            if phys_y < MENU_BAR_HEIGHT as f32 * scale {
                self.handle_menu_bar_click(phys_x, phys_y);
            }
            return;
        }

        // Menu bar at the very top (physical coordinates)
        if phys_y < MENU_BAR_HEIGHT as f32 * scale {
            self.handle_menu_bar_click(phys_x, phys_y);
            return;
        }

        let x = phys_x / scale;
        let y = phys_y / scale;

        // Check if click is in toolbar
        if y >= MENU_BAR_HEIGHT as f32 && y < CHROME_HEIGHT as f32 {
            let toolbar_btn_y = MENU_BAR_HEIGHT as f32 + BTN_Y;
            // Back button
            if (6.0..6.0 + BTN_SIZE).contains(&x)
                && (toolbar_btn_y..toolbar_btn_y + BTN_SIZE).contains(&y)
            {
                self.go_back();
                return;
            }
            // Forward button
            if (36.0..36.0 + BTN_SIZE).contains(&x)
                && (toolbar_btn_y..toolbar_btn_y + BTN_SIZE).contains(&y)
            {
                self.go_forward();
                return;
            }
            // Reload button (acts as Stop while loading)
            if (66.0..66.0 + BTN_SIZE).contains(&x)
                && (toolbar_btn_y..toolbar_btn_y + BTN_SIZE).contains(&y)
            {
                if self.images_loading.load(Ordering::SeqCst) {
                    self.stop_loading();
                } else {
                    self.reload();
                }
                return;
            }
            let addr_y = MENU_BAR_HEIGHT as f32 + ADDR_BAR_TOP;
            // Address bar
            if x >= ADDR_BAR_LEFT && (addr_y..addr_y + ADDR_BAR_HEIGHT).contains(&y) {
                let was_focused = self.address_focused;
                self.address_focused = true;
                if was_focused {
                    // Already editing: place the caret at the click point instead
                    // of re-selecting everything on every click.
                    self.address_all_selected = false;
                    self.cursor_pos = address_bar_cursor_index(&self.address_text, x, scale);
                } else {
                    // First focus: select the whole URL so typing replaces it.
                    self.address_all_selected = true;
                    self.cursor_pos = self.address_text.len();
                }
                self.request_redraw();
                return;
            }
        } else {
            // Click outside address bar unfocuses it
            self.address_focused = false;
            self.menu_active = None;
            self.menu_item_hover = None;

            // Check for link and form-control clicks (y is relative to window,
            // flat_boxes are relative to page)
            let page_y = y - CHROME_HEIGHT as f32;
            if let Some(fbox) = self.flat_box_at(x, page_y) {
                if let Some(ref href) = fbox.link_href {
                    let resolved = match resolve_url(&self.current_url, href) {
                        Ok(u) => u,
                        Err(_) => href.clone(),
                    };
                    self.navigate(&resolved);
                    return;
                }

                if self.try_focus_or_submit_form_control(fbox.node_id) {
                    return;
                }
            }

            // Clicking blank page area unfocuses any active input.
            self.focused_input = None;
            self.request_redraw();
        }
    }

    fn handle_key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.handle_global_shortcut(event) {
            return;
        }
        if self.handle_menu_keys(event) {
            return;
        }
        if !self.address_focused && self.focused_input.is_some() {
            if self.handle_input_key(event) {
                return;
            }
        }
        if !self.address_focused {
            // Page scrolling
            match &event.logical_key {
                Key::Named(NamedKey::ArrowDown) => {
                    self.scroll_y = (self.scroll_y + 40.0).min(self.max_scroll_y);
                    self.request_redraw();
                }
                Key::Named(NamedKey::ArrowUp) => {
                    self.scroll_y = (self.scroll_y - 40.0).max(0.0);
                    self.request_redraw();
                }
                Key::Named(NamedKey::PageDown) => {
                    self.scroll_y = (self.scroll_y + 400.0).min(self.max_scroll_y);
                    self.request_redraw();
                }
                Key::Named(NamedKey::PageUp) => {
                    self.scroll_y = (self.scroll_y - 400.0).max(0.0);
                    self.request_redraw();
                }
                Key::Named(NamedKey::End) => {
                    self.scroll_y = self.max_scroll_y;
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
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => {
                let resolved = resolve_search_or_url(&self.settings, &self.address_text);
                self.address_focused = false;
                self.navigate(&resolved);
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
            Key::Character(ref ch)
                if self.modifiers.control_key() || self.modifiers.super_key() =>
            {
                let lower = ch.to_lowercase();
                match lower.as_str() {
                    "a" => {
                        self.address_all_selected = true;
                        self.cursor_pos = self.address_text.len();
                        self.request_redraw();
                    }
                    "c" => {
                        if self.address_all_selected {
                            let to_copy = self.address_text.clone();
                            if let Some(cb) = self.clipboard.as_mut() {
                                let _ = cb.set_text(to_copy);
                            }
                        }
                    }
                    "x" => {
                        if self.address_all_selected {
                            let to_copy = self.address_text.clone();
                            if let Some(cb) = self.clipboard.as_mut() {
                                let _ = cb.set_text(to_copy);
                            }
                            self.address_text.clear();
                            self.cursor_pos = 0;
                            self.address_all_selected = false;
                            self.request_redraw();
                        }
                    }
                    "v" => {
                        let pasted = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok());
                        if let Some(text) = pasted {
                            if self.address_all_selected {
                                self.address_text.clear();
                                self.cursor_pos = 0;
                                self.address_all_selected = false;
                            }
                            for c in text.chars() {
                                if c >= ' ' && c != '\x7f' {
                                    self.address_text.insert(self.cursor_pos, c);
                                    self.cursor_pos += 1;
                                }
                            }
                            self.request_redraw();
                        }
                    }
                    _ => {}
                }
            }
            Key::Character(ref ch) if self.modifiers.is_empty() => {
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
        self.scroll_y = (self.scroll_y - delta_y * 30.0).clamp(0.0, self.max_scroll_y);
        self.request_redraw();
    }

    /// Return the flat box that encloses the given page-local point, preferring
    /// interactive controls and links over plain containers.
    fn flat_box_at(&self, page_x: f32, page_y: f32) -> Option<&incognidium_layout::FlatBox> {
        let mut best: Option<&incognidium_layout::FlatBox> = None;
        for fbox in &self.flat_boxes {
            let hit = page_x >= fbox.x
                && page_x <= fbox.x + fbox.width
                && page_y >= fbox.y
                && page_y <= fbox.y + fbox.height;
            if !hit {
                continue;
            }
            let interactive = fbox.input_type.is_some()
                || fbox
                    .link_href
                    .as_deref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
            let replace = match best {
                None => true,
                Some(b) if interactive && b.input_type.is_none() && b.link_href.is_none() => true,
                _ => false,
            };
            if replace {
                best = Some(fbox);
            }
        }
        best
    }

    fn find_hover_url(&self, phys_x: f64, phys_y: f64) -> Option<String> {
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor() as f32)
            .unwrap_or(1.0);
        let x = phys_x as f32 / scale;
        let y = phys_y as f32 / scale;
        if y < CHROME_HEIGHT as f32 {
            return None;
        }
        let page_y = y - CHROME_HEIGHT as f32;
        for fbox in &self.flat_boxes {
            if let Some(ref href) = fbox.link_href {
                if x >= fbox.x
                    && x <= fbox.x + fbox.width
                    && page_y >= fbox.y
                    && page_y <= fbox.y + fbox.height
                {
                    return match resolve_url(&self.current_url, href) {
                        Ok(u) => Some(u),
                        Err(_) => Some(href.clone()),
                    };
                }
            }
        }
        None
    }

    fn update_hover(&mut self, phys_x: f64, phys_y: f64) {
        let new_hover = self.find_hover_url(phys_x, phys_y);
        let changed = self.hover_url.as_deref() != new_hover.as_deref();
        if changed {
            self.hover_url = new_hover;
            if let Some(w) = &self.window {
                let cursor = if self.hover_url.is_some() {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                };
                w.set_cursor(cursor);
            }
            self.request_redraw();
        }
    }

    /// Focus or activate the form control at `node_id`.
    ///
    /// - Text/password/textarea inputs become the focused control.
    /// - Button and submit clicks submit the containing form.
    /// - Label clicks focus the associated control.
    fn try_focus_or_submit_form_control(&mut self, node_id: incognidium_dom::NodeId) -> bool {
        let doc = match &self.js_modified_doc {
            Some(d) => d,
            None => return false,
        };
        let node = match doc.nodes.get(node_id) {
            Some(n) => n,
            None => return false,
        };
        let el = match &node.data {
            incognidium_dom::NodeData::Element(e) => e,
            _ => return false,
        };

        if el.tag_name == "label" {
            if let Some(for_id) = el
                .get_attr("for")
                .and_then(|v| Self::find_node_by_id_static(doc, v))
            {
                return self.try_focus_or_submit_form_control(for_id);
            }
            return false;
        }

        let is_text_input = el.tag_name == "input"
            && matches!(
                el.get_attr("type").as_deref().unwrap_or("text"),
                "text" | "password" | "email" | "search" | "url" | "tel"
            );
        let is_textarea = el.tag_name == "textarea";

        if is_text_input || is_textarea {
            self.focused_input = Some(node_id);
            let value = input_value(doc, node_id);
            self.input_cursor = value.chars().count();
            self.request_redraw();
            return true;
        }

        let is_button = el.tag_name == "button"
            || (el.tag_name == "input"
                && matches!(
                    el.get_attr("type").as_deref().unwrap_or("submit"),
                    "button" | "submit" | "reset"
                ));

        if is_button {
            if let Some(form_id) = self.find_enclosing_form(node_id) {
                self.submit_dom_form(form_id);
                return true;
            }
        }

        false
    }

    /// Find the first element in `doc` with `id == target_id`.
    fn find_node_by_id_static(
        doc: &incognidium_dom::Document,
        target_id: &str,
    ) -> Option<incognidium_dom::NodeId> {
        for (id, node) in doc.nodes.iter().enumerate() {
            if let incognidium_dom::NodeData::Element(el) = &node.data {
                if el.get_attr("id").as_deref() == Some(target_id) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Walk up the DOM from `node_id` to find the nearest ancestor `<form>`.
    fn find_enclosing_form(
        &self,
        node_id: incognidium_dom::NodeId,
    ) -> Option<incognidium_dom::NodeId> {
        let doc = self.js_modified_doc.as_ref()?;
        let mut current = node_id;
        while let Some(node) = doc.nodes.get(current) {
            if let incognidium_dom::NodeData::Element(el) = &node.data {
                if el.tag_name == "form" {
                    return Some(current);
                }
            }
            let parent = node.parent?;
            if parent == current {
                break;
            }
            current = parent;
        }
        None
    }

    /// Build a `FormSubmission` from a `\u003cform\u003e` element in the live DOM and
    /// submit it through the shared network path.
    fn submit_dom_form(&mut self, form_id: incognidium_dom::NodeId) {
        let doc = match &self.js_modified_doc {
            Some(d) => d,
            None => return,
        };
        let (action, method) = match doc.nodes.get(form_id) {
            Some(n) => match &n.data {
                incognidium_dom::NodeData::Element(el) => (
                    el.get_attr("action").unwrap_or("").to_string(),
                    el.get_attr("method").unwrap_or("get").to_string(),
                ),
                _ => return,
            },
            None => return,
        };

        let mut fields: Vec<(String, String)> = Vec::new();
        collect_form_fields(doc, form_id, &mut fields);

        let submission = FormSubmission {
            action,
            method,
            fields,
        };
        self.submit_form(&submission);
        self.focused_input = None;
    }

    fn focus_address_bar(&mut self) {
        self.address_focused = true;
        self.address_all_selected = true;
        self.cursor_pos = self.address_text.len();
        self.request_redraw();
    }

    fn zoom_in(&mut self) {
        self.zoom_factor = (self.zoom_factor + 0.1).min(3.0);
        self.zoom_flash = Some((self.zoom_factor, Instant::now()));
        self.render();
        self.request_redraw();
    }
    fn zoom_out(&mut self) {
        self.zoom_factor = (self.zoom_factor - 0.1).max(0.5);
        self.zoom_flash = Some((self.zoom_factor, Instant::now()));
        self.render();
        self.request_redraw();
    }
    fn zoom_reset(&mut self) {
        self.zoom_factor = 1.0;
        self.zoom_flash = Some((self.zoom_factor, Instant::now()));
        self.render();
        self.request_redraw();
    }

    fn handle_menu_keys(&mut self, event: &KeyEvent) -> bool {
        let idx = match self.menu_active {
            Some(i) => i,
            None => return false,
        };
        let count = MENU_BAR.len();
        match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => {
                self.menu_active = Some(if idx == 0 { count - 1 } else { idx - 1 });
                self.menu_item_hover = None;
                self.request_redraw();
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.menu_active = Some((idx + 1) % count);
                self.menu_item_hover = None;
                self.request_redraw();
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                if self.menu_item_hover.is_none() {
                    self.menu_item_hover = Some(0);
                }
                self.request_redraw();
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.menu_item_hover = self.menu_item_hover.map(|i| i.saturating_sub(1));
                self.request_redraw();
                true
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(item_idx) = self.menu_item_hover {
                    let item = MENU_BAR[idx].1[item_idx];
                    self.execute_menu_item(item);
                    self.menu_active = None;
                    self.menu_item_hover = None;
                    self.request_redraw();
                }
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.menu_active = None;
                self.menu_item_hover = None;
                self.request_redraw();
                true
            }
            _ => false,
        }
    }

    fn clear_history(&mut self) {
        self.history_entries.clear();
        self.history.clear();
        self.history_pos = 0;
        let _ = std::fs::remove_file(
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config/incognidium/history.json"),
        );
        self.request_redraw();
    }
    fn handle_global_shortcut(&mut self, event: &KeyEvent) -> bool {
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        match &event.logical_key {
            Key::Named(NamedKey::F5) => {
                self.reload();
                true
            }
            Key::Character(ch) if ctrl && ch.eq_ignore_ascii_case("r") => {
                self.reload();
                true
            }
            Key::Character(ch) if ctrl && ch.eq_ignore_ascii_case("l") => {
                self.focus_address_bar();
                true
            }
            Key::Named(NamedKey::ArrowLeft) if alt => {
                self.go_back();
                true
            }
            Key::Named(NamedKey::ArrowRight) if alt => {
                self.go_forward();
                true
            }
            Key::Named(NamedKey::Escape) if self.menu_active.is_some() => {
                self.menu_active = None;
                self.request_redraw();
                true
            }
            Key::Named(NamedKey::Escape) if self.images_loading.load(Ordering::SeqCst) => {
                self.stop_loading();
                true
            }
            Key::Named(NamedKey::F11) => {
                self.toggle_fullscreen();
                true
            }
            Key::Character(ch) if ctrl && ch.eq_ignore_ascii_case("u") => {
                self.navigate("about:source");
                true
            }
            Key::Character(ch) if ctrl && ch.eq_ignore_ascii_case("0") => {
                self.zoom_reset();
                true
            }
            Key::Character(ch) if ctrl && (ch == "+" || ch == "=") => {
                self.zoom_in();
                true
            }
            Key::Character(ch)
                if ctrl && (ch.eq_ignore_ascii_case("minus") || ch == "-" || ch == "_") =>
            {
                self.zoom_out();
                true
            }
            _ => false,
        }
    }

    fn stop_loading(&mut self) {
        self.images_cancel.store(true, Ordering::SeqCst);
        self.images_loading.store(false, Ordering::SeqCst);
        self.pending_images.lock().unwrap().clear();
        self.request_redraw();
    }

    fn current_page_title(&self) -> String {
        if let Some(ref cached) = self.cached_layout {
            extract_title(&cached.doc)
        } else {
            self.current_url.clone()
        }
    }

    fn add_bookmark(&mut self) {
        let title = self.current_page_title();
        let url = self.current_url.clone();
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = Bookmark {
            title,
            url,
            created_at: created,
        };
        // Avoid duplicates: replace existing entry with same URL
        self.bookmarks.retain(|b| b.url != entry.url);
        self.bookmarks.push(entry);
        if let Err(e) = save_bookmarks(&self.bookmarks) {
            eprintln!("Failed to save bookmarks: {e}");
        }
    }

    fn record_current_history(&mut self) {
        let title = self.current_page_title();
        record_history_entry(&mut self.history_entries, &self.current_url, &title);
        if let Err(e) = save_history(&self.history_entries) {
            eprintln!("Failed to save history: {e}");
        }
    }

    fn update_window_title(&self) {
        if let Some(w) = &self.window {
            let page_title = self.current_page_title();
            let label = if page_title.is_empty() {
                &self.current_url
            } else {
                &page_title
            };
            w.set_title(&format!("{label} - Incognidium"));
        }
    }

    fn toggle_fullscreen(&mut self) {
        self.is_fullscreen = !self.is_fullscreen;
        if let Some(w) = &self.window {
            if self.is_fullscreen {
                w.set_fullscreen(Some(Fullscreen::Borderless(None)));
            } else {
                w.set_fullscreen(None);
            }
        }
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

    fn menu_bar_layout(scale: f32) -> Vec<(f32, f32, &'static str, &'static [&'static str])> {
        let start = 14.0 * scale;
        let pad_x = 16.0 * scale;
        let mut x = start;
        let mut out = Vec::new();
        for (label, items) in MENU_BAR {
            let text_w = measure_toolbar_text(label, scale);
            let width = text_w + pad_x * 2.0;
            out.push((x, width, *label, *items));
            x += width;
        }
        out
    }

    fn draw_menu_bar(
        full: &mut Pixmap,
        scale: f32,
        menu_active: Option<usize>,
        cursor_x: f32,
        cursor_y: f32,
    ) {
        let layout = Self::menu_bar_layout(scale);
        let (cx, cy) = (cursor_x, cursor_y);
        let hover_idx = if cy >= 0.0 && cy <= MENU_BAR_HEIGHT as f32 * scale {
            layout
                .iter()
                .position(|(x, w, _, _)| cx >= *x && cx <= x + w)
        } else {
            None
        };
        let line_h = 14.0 * scale;
        let text_pad = 16.0 * scale;
        let text_y = (MENU_BAR_HEIGHT as f32 * scale - line_h) * 0.5 + line_h * 0.62;
        for (i, (x, width, label, _)) in layout.iter().enumerate() {
            let active = menu_active == Some(i);
            let hovered = hover_idx == Some(i);
            if active {
                draw_toolbar_rect(
                    full,
                    *x,
                    0.0,
                    *width,
                    MENU_BAR_HEIGHT as f32 * scale,
                    0x22,
                    0x77,
                    0xee,
                );
                draw_toolbar_text(full, x + text_pad, text_y, label, 0xff, 0xff, 0xff, scale);
            } else if hovered {
                draw_toolbar_rect(
                    full,
                    *x,
                    0.0,
                    *width,
                    MENU_BAR_HEIGHT as f32 * scale,
                    0xe8,
                    0xee,
                    0xf7,
                );
                draw_toolbar_text(full, x + text_pad, text_y, label, 0x22, 0x77, 0xee, scale);
            } else {
                draw_toolbar_text(full, x + text_pad, text_y, label, 0x44, 0x44, 0x44, scale);
            }
        }
    }

    fn draw_menu_dropdown(
        full: &mut Pixmap,
        scale: f32,
        idx: usize,
        layout: &[(f32, f32, &str, &[&str])],
        cursor_x: f32,
        cursor_y: f32,
        kb_hover: Option<usize>,
    ) {
        let (x, label_w, _label, items) = layout[idx];
        let line_h = 24.0 * scale;
        let pad = 14.0 * scale;
        let max_text_w = items
            .iter()
            .map(|item| measure_toolbar_text(item, scale))
            .fold(0.0, f32::max);
        let dw = (max_text_w + pad * 2.0).max(label_w + 12.0 * scale);
        let dh = (items.len() as f32 * line_h + 12.0) * scale;
        let dx = x;
        let dy = MENU_BAR_HEIGHT as f32 * scale + 2.0 * scale;
        // soft shadow
        draw_rounded_rect(
            full,
            dx + 2.0 * scale,
            dy + 2.0 * scale,
            dw,
            dh,
            6.0 * scale,
            0xcc,
            0xcc,
            0xcc,
        );
        draw_rounded_rect(full, dx, dy, dw, dh, 6.0 * scale, 0xff, 0xff, 0xff);
        draw_rounded_rect_stroke(full, dx, dy, dw, dh, 6.0 * scale, 1.0, 0xcc, 0xcc, 0xcc);
        let hover_item = if let Some(i) = kb_hover {
            Some(i)
        } else if cursor_x >= dx && cursor_x <= dx + dw && cursor_y >= dy && cursor_y <= dy + dh {
            let rel_y = cursor_y - dy - 6.0 * scale;
            let idx = (rel_y / line_h).floor() as usize;
            if idx < items.len() {
                Some(idx)
            } else {
                None
            }
        } else {
            None
        };
        for (i, item) in items.iter().enumerate() {
            let iy = dy + 6.0 * scale + i as f32 * line_h;
            let item_y = iy + line_h * 0.68;
            let hovered = hover_item == Some(i);
            if hovered {
                draw_toolbar_rect(
                    full,
                    dx + 4.0 * scale,
                    iy,
                    dw - 8.0 * scale,
                    line_h,
                    0x22,
                    0x77,
                    0xee,
                );
            }
            let is_wired = matches!(
                *item,
                "Back"
                    | "Forward"
                    | "Reload"
                    | "Stop"
                    | "Home"
                    | "Add Bookmark"
                    | "Show Bookmarks"
                    | "Show All History"
                    | "Settings"
                    | "About"
                    | "Select All"
                    | "Full Screen"
            );
            let (r, g, b) = if hovered {
                (0xff, 0xff, 0xff)
            } else if is_wired {
                (0x33, 0x33, 0x33)
            } else {
                (0x99, 0x99, 0x99)
            };
            draw_toolbar_text(full, dx + pad, item_y, item, r, g, b, scale);
            if i + 1 < items.len() && [3, 7].contains(&i) {
                draw_toolbar_rect(
                    full,
                    dx + pad * 0.6,
                    iy + line_h - 1.0 * scale,
                    dw - pad * 1.2,
                    1.0,
                    0xee,
                    0xee,
                    0xee,
                );
            }
        }
    }
    fn handle_menu_bar_click(&mut self, x: f32, y: f32) -> bool {
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor() as f32)
            .unwrap_or(1.0)
            * self.zoom_factor;
        let layout = Self::menu_bar_layout(scale);
        // Check dropdown first if a menu is open
        if let Some(idx) = self.menu_active {
            let (x0, label_w, _label, items) = layout[idx];
            let line_h = 22.0 * scale;
            let pad = 10.0 * scale;
            let max_text_w = items
                .iter()
                .map(|item| measure_toolbar_text(item, scale))
                .fold(0.0, f32::max);
            let dw = (max_text_w + pad * 2.0).max(label_w);
            let dh = (items.len() as f32 * (line_h + 4.0) + 16.0) * scale;
            let dx = x0;
            let dy = MENU_BAR_HEIGHT as f32 * scale + 2.0 * scale;
            let inside_dropdown = x >= dx && x <= dx + dw && y >= dy && y <= dy + dh;
            if inside_dropdown {
                let rel_y = y - dy - 8.0 * scale;
                let item_h = line_h + 4.0 * scale;
                let item = (rel_y / item_h).floor() as usize;
                if item < items.len() {
                    self.execute_menu_item(items[item]);
                }
                self.menu_active = None;
                self.menu_item_hover = None;
                self.request_redraw();
                return true;
            }
            // Click outside dropdown closes it, but might land on another label below
            self.menu_active = None;
            self.menu_item_hover = None;
        }
        // Check top-level labels
        if let Some(idx) = layout
            .iter()
            .position(|(x0, w, _, _)| x >= *x0 && x <= x0 + w)
        {
            self.menu_active = Some(idx);
            self.menu_item_hover = None;
            self.request_redraw();
            return true;
        }
        false
    }

    fn execute_menu_item(&mut self, item: &str) {
        match item {
            "Back" => self.go_back(),
            "Forward" => self.go_forward(),
            "Reload" => self.reload(),
            "Stop" => self.stop_loading(),
            "Home" => {
                let home = self.settings.homepage.clone();
                self.navigate(&home);
            }
            "Add Bookmark" => {
                self.add_bookmark();
                self.navigate("about:bookmarks");
            }
            "Show Bookmarks" => self.navigate("about:bookmarks"),
            "Show All History" => self.navigate("about:history"),
            "Clear History" => self.clear_history(),
            "Settings" => self.navigate("about:settings"),
            "About" => self.navigate("about:home"),
            "Zoom In" => self.zoom_in(),
            "Zoom Out" => self.zoom_out(),
            "Reset Zoom" => self.zoom_reset(),
            "Select All" => self.focus_address_bar(),
            _ => {}
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
        self.update_window_title();

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
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.modifiers = new_modifiers.state();
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
                self.update_hover(position.x, position.y);
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
                    let _sx = x as f32 / scale;
                    let _sy = y as f32 / scale;
                    let _win_width = self
                        .window
                        .as_ref()
                        .map(|w| w.inner_size().width)
                        .unwrap_or(DEFAULT_WIDTH);
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
                self.handle_key(&event);
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
    Loading,
}

fn draw_icon_button(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    icon: ButtonIcon,
    enabled: bool,
    hovered: bool,
    scale: f32,
    frame_angle: f32,
) {
    let sz = BTN_SIZE * scale;
    let (bg_r, bg_g, bg_b) = if enabled {
        if hovered {
            (0xdd, 0xdd, 0xdd)
        } else {
            (0xe8, 0xe8, 0xe8)
        }
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
        ButtonIcon::Reload => draw_reload_icon(pixmap, cx, cy, s * 1.2, fg_r, fg_g, fg_b),
        ButtonIcon::Loading => {
            draw_loading_spinner(pixmap, cx, cy, s * 1.3, frame_angle, fg_r, fg_g, fg_b)
        }
    }
}

fn draw_loading_spinner(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    r: f32,
    angle_deg: f32,
    red: u8,
    green: u8,
    blue: u8,
) {
    let mut pb = PathBuilder::new();
    let start = angle_deg.to_radians();
    let sweep = 1.8; // radians (~103 degrees)
    let steps = 24;
    for i in 0..=steps {
        let a = start + sweep * (i as f32 / steps as f32);
        let px = cx + r * a.cos();
        let py = cy + r * a.sin();
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(red, green, blue, 255));
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: 2.5,
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

fn draw_arrow(pixmap: &mut Pixmap, cx: f32, cy: f32, s: f32, r: u8, g: u8, b: u8, left: bool) {
    let mut pb = PathBuilder::new();
    // The head is the point of the chevron: a left-pointing (back) arrow
    // has its head on the left, a right-pointing (forward) arrow on the right.
    let head_x = if left { cx - s } else { cx + s };
    let tail_x = if left { cx + s } else { cx - s };
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

fn draw_reload_icon(pixmap: &mut Pixmap, cx: f32, cy: f32, s: f32, r: u8, g: u8, b: u8) {
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
    let p0 = points[0];
    let p1 = points[1];
    let p2 = points[2];
    let p3 = points[3];
    let p4 = points[4];
    let p5 = points[5];
    let p6 = points[6];
    let p7 = points[7];
    let p8 = points[8];
    let p9 = (cx + s * 0.4, cy + s);
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
    let ax = cx + s;
    let ay = cy - s * 0.4;
    draw_chevron(
        pixmap,
        ax - arr * 0.7,
        ay - arr,
        ax,
        ay,
        ax - arr * 0.7,
        ay + arr,
        r,
        g,
        b,
    );
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
/// Map a click x-coordinate (logical px) inside the address bar to the nearest
/// character index in `text`, accounting for the leading lock icon and padding.
fn address_bar_cursor_index(text: &str, click_x: f32, scale: f32) -> usize {
    // Matches draw_address_bar: text starts at ADDR_BAR_LEFT + pad + icon_w.
    let pad = 6.0;
    let icon_w = 22.0;
    let text_start = ADDR_BAR_LEFT + pad + icon_w;
    let rel = (click_x - text_start).max(0.0);
    if rel <= 0.0 {
        return 0;
    }
    let px_per_scaled = if scale > 0.0 { scale } else { 1.0 };
    let mut acc = 0.0f32;
    for (i, ch) in text.chars().enumerate() {
        let adv = measure_toolbar_text(&ch.to_string(), scale) / px_per_scaled;
        if acc + adv / 2.0 >= rel {
            return i;
        }
        acc += adv;
    }
    text.chars().count()
}

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

const MENU_BAR: &[(&str, &[&str])] = &[
    ("File", &["New Window", "Open Location...", "Close Window"]),
    ("Edit", &["Undo", "Cut", "Copy", "Paste", "Select All"]),
    ("View", &["Reload", "Stop", "Home", "Full Screen"]),
    (
        "History",
        &["Back", "Forward", "Show All History", "Clear History"],
    ),
    ("Bookmarks", &["Add Bookmark", "Show Bookmarks"]),
    ("Tools", &["Settings", "Downloads", "Developer Tools"]),
    ("Help", &["About"]),
];

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let mcp_mode = args.iter().any(|a| a == "--mcp");
    let settings = load_settings();
    let bookmarks = load_bookmarks();
    let history = load_history();

    // Parse URL arg (skip --mcp)
    let url_arg = args.iter().skip(1).find(|a| *a != "--mcp");
    let (initial_url, initial_html) = if let Some(input) = url_arg {
        let ud = incognidium_chrome::UserData {
            settings: settings.clone(),
            bookmarks: bookmarks.clone(),
            history: history.clone(),
        };
        if let Some(html) = build_local_page(input, &ud, None) {
            (input.clone(), html)
        } else {
            match fetch_url_solving_challenges(input) {
                Ok(resp) => (resp.url, resp.body),
                Err(e) => {
                    eprintln!("Failed to load {input}: {e}");
                    (
                        "about:blank".into(),
                        build_local_page("about:blank", &ud, None)
                            .unwrap_or_else(|| "<html><body></body></html>".to_string()),
                    )
                }
            }
        }
    } else {
        let home = settings.homepage.clone();
        let ud = incognidium_chrome::UserData {
            settings: settings.clone(),
            bookmarks: bookmarks.clone(),
            history: history.clone(),
        };
        if let Some(html) = build_local_page(&home, &ud, None) {
            (home, html)
        } else {
            match fetch_url_solving_challenges(&home) {
                Ok(resp) => (resp.url, resp.body),
                Err(e) => {
                    eprintln!("Failed to load homepage {home}: {e}");
                    (
                        "about:blank".into(),
                        build_local_page("about:blank", &ud, None)
                            .unwrap_or_else(|| "<html><body></body></html>".to_string()),
                    )
                }
            }
        }
    };

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new(initial_url, initial_html, settings.clone());

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
