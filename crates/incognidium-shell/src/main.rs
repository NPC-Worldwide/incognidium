use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::{fetch_url, resolve_url, fetch_bytes};
use incognidium_paint::{paint_with_images, ImageData};
use incognidium_style::resolve_styles;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use incognidium_devtools::{
    DevToolsBridge, DevToolsCommand, NetworkEntry,
    extract_links, extract_page_text, extract_title,
};

use incognidium_shell::collect_scripts;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;
const TOOLBAR_HEIGHT: u32 = 40;
const ADDR_BAR_LEFT: f32 = 90.0;
const ADDR_BAR_TOP: f32 = 6.0;
const ADDR_BAR_HEIGHT: f32 = 28.0;
const ADDR_BAR_RIGHT_MARGIN: f32 = 10.0;
const BTN_SIZE: f32 = 28.0;
const BTN_Y: f32 = 6.0;

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
}

/// Cached results from parse -> style -> layout pipeline.
struct CachedLayout {
    doc: incognidium_dom::Document,
    styles: incognidium_style::StyleMap,
    layout_root: incognidium_layout::LayoutBox,
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
            external_css: String::new(),
            cached_layout: None,
            layout_dirty: true,
            last_layout_width: 0,
            devtools: None,
            pending_images: Arc::new(Mutex::new(Vec::new())),
            images_loading: Arc::new(AtomicBool::new(false)),
            js_modified_doc: None,
        }
    }

    fn navigate(&mut self, url_input: &str) {
        let url_str = url_input.to_string();

        match fetch_url(&url_str) {
            Ok(resp) => {
                self.log_network("GET", &url_str, Some(200), &resp.body.len().to_string(), resp.body.len(), None);
                self.html_content = resp.body.clone();
                self.current_url = resp.url.clone();
                self.address_text = resp.url.clone();
                self.cursor_pos = self.address_text.len();
                self.scroll_y = 0.0;

                // Clear JS-modified DOM from previous page
                self.js_modified_doc = None;

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
                self.fetch_page_images_async(&resp.url, &resp.body);
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
        match fetch_url(url) {
            Ok(resp) => {
                self.html_content = resp.body.clone();
                self.current_url = resp.url.clone();
                self.address_text = resp.url.clone();
                self.cursor_pos = self.address_text.len();
                self.scroll_y = 0.0;
                self.js_modified_doc = None;
                self.fetch_external_css(&resp.url, &resp.body);
                self.execute_scripts();
                self.layout_dirty = true;

                // Render text immediately, fetch images in background
                self.image_cache.clear();
                self.render();
                self.fetch_page_images_async(&resp.url, &resp.body);
            }
            Err(e) => {
                self.html_content = format!(
                    r#"<html><body><h1>Error</h1><p>{}</p></body></html>"#, e
                );
                self.image_cache.clear();
                self.external_css.clear();
                self.js_modified_doc = None;
                self.layout_dirty = true;
            }
        }
        self.request_redraw();
    }

    fn fetch_page_images_async(&mut self, base_url: &str, html: &str) {
        let doc = parse_html(html);
        let mut urls: Vec<(String, String)> = Vec::new(); // (src, resolved_url)
        const MAX_IMAGES: usize = 50;

        for node in &doc.nodes {
            if urls.len() >= MAX_IMAGES { break; }
            if let incognidium_dom::NodeData::Element(ref el) = node.data {
                if el.tag_name == "img" {
                    if let Some(src) = el.get_attr("src") {
                        if src.starts_with("data:") { continue; }
                        if let Ok(resolved) = resolve_url(base_url, src) {
                            urls.push((src.to_string(), resolved));
                        }
                    }
                }
            }
        }

        if urls.is_empty() { return; }

        let pending = self.pending_images.clone();
        let loading = self.images_loading.clone();
        loading.store(true, Ordering::SeqCst);

        // Clear pending buffer
        pending.lock().unwrap().clear();

        std::thread::spawn(move || {
            // Process in chunks of 8 for parallelism
            for chunk in urls.chunks(8) {
                let chunk: Vec<_> = chunk.to_vec();
                let handles: Vec<_> = chunk.into_iter().map(|(src, resolved)| {
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
                            Err(e) => {
                                log::warn!("Failed to fetch image {src}: {e}");
                            }
                        }
                        None
                    })
                }).collect();

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
        if pending.is_empty() { return false; }
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
        const MAX_STYLESHEETS: usize = 10;

        for node in &doc.nodes {
            if fetched >= MAX_STYLESHEETS {
                break;
            }
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
                                    self.external_css.push_str(&resp.body);
                                    self.external_css.push('\n');
                                    fetched += 1;
                                }
                                Err(e) => {
                                    log::warn!("Failed to fetch stylesheet {href}: {e}");
                                }
                            }
                        }
                    }
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
            let modified_doc = incognidium_shell::execute_scripts_on_doc(doc, &scripts, &mut image_cache);
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
        let width = size.width.max(1);
        let height = size.height.max(1);
        let page_height = height.saturating_sub(TOOLBAR_HEIGHT);

        // Re-layout only when content changed or window resized
        if self.layout_dirty || self.last_layout_width != width {
            // Use JS-modified DOM if available, otherwise re-parse from HTML
            let doc = if let Some(ref modified) = self.js_modified_doc {
                modified.clone()
            } else {
                parse_html(&self.html_content)
            };
            let css_text = doc.collect_style_text();
            let stylesheet = parse_css(&css_text);
            let styles = resolve_styles(&doc, &stylesheet);

            let mut image_sizes = ImageSizes::new();
            for (src, img) in &self.image_cache {
                image_sizes.insert(src.clone(), (img.width, img.height));
            }

            let layout_root = layout_with_images(&doc, &styles, width as f32, 10000.0, &image_sizes);

            self.cached_layout = Some(CachedLayout { doc, styles, layout_root });
            self.layout_dirty = false;
            self.last_layout_width = width;
        }

        let cached = match &self.cached_layout {
            Some(c) => c,
            None => return,
        };

        // Flatten with current scroll offset (cheap)
        let flat_boxes = flatten_layout(&cached.layout_root, 0.0, -self.scroll_y);

        // Paint page content (cheap compared to parse+layout)
        let pixmap = paint_with_images(&flat_boxes, &cached.styles, width, page_height, &self.image_cache);

        // Store flat boxes for link click detection
        self.flat_boxes = flat_boxes;

        // Create full window pixmap
        let mut full = Pixmap::new(width, height).expect("pixmap");
        full.fill(Color::WHITE);

        // Draw toolbar background
        draw_toolbar_rect(&mut full, 0.0, 0.0, width as f32, TOOLBAR_HEIGHT as f32,
            0xf0, 0xf0, 0xf0);
        // Toolbar bottom border
        draw_toolbar_rect(&mut full, 0.0, TOOLBAR_HEIGHT as f32 - 1.0, width as f32, 1.0,
            0xcc, 0xcc, 0xcc);

        // Back button
        let can_back = self.history_pos > 0;
        draw_nav_button(&mut full, 6.0, BTN_Y, "<", can_back);
        // Forward button
        let can_fwd = self.history_pos + 1 < self.history.len();
        draw_nav_button(&mut full, 36.0, BTN_Y, ">", can_fwd);
        // Reload button
        draw_nav_button(&mut full, 66.0, BTN_Y, "R", true);

        // Address bar
        let addr_width = width as f32 - ADDR_BAR_LEFT - ADDR_BAR_RIGHT_MARGIN;
        draw_address_bar(&mut full, ADDR_BAR_LEFT, ADDR_BAR_TOP, addr_width, ADDR_BAR_HEIGHT,
            &self.address_text, self.address_focused, self.cursor_pos, self.address_all_selected);

        // Copy page content below toolbar
        let page_data = pixmap.data();
        let full_data = full.data_mut();
        for y in 0..page_height {
            for x in 0..width {
                let src = ((y * width + x) * 4) as usize;
                let dst = (((y + TOOLBAR_HEIGHT) * width + x) * 4) as usize;
                if src + 3 < page_data.len() && dst + 3 < full_data.len() {
                    full_data[dst] = page_data[src];
                    full_data[dst + 1] = page_data[src + 1];
                    full_data[dst + 2] = page_data[src + 2];
                    full_data[dst + 3] = page_data[src + 3];
                }
            }
        }

        // Copy to window surface
        surface
            .resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            )
            .expect("resize");

        let mut buffer = surface.buffer_mut().expect("buffer");
        let data = full.data();
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                if idx + 3 < data.len() {
                    let r = data[idx] as u32;
                    let g = data[idx + 1] as u32;
                    let b = data[idx + 2] as u32;
                    buffer[(y * width + x) as usize] = (r << 16) | (g << 8) | b;
                }
            }
        }

        buffer.present().expect("present");

        // Sync state to devtools bridge
        self.sync_devtools(&full);
    }

    fn handle_click(&mut self, x: f64, y: f64) {
        let x = x as f32;
        let y = y as f32;

        // Check if click is in toolbar
        if y < TOOLBAR_HEIGHT as f32 {
            // Back button
            if x >= 6.0 && x < 6.0 + BTN_SIZE && y >= BTN_Y && y < BTN_Y + BTN_SIZE {
                self.go_back();
                return;
            }
            // Forward button
            if x >= 36.0 && x < 36.0 + BTN_SIZE && y >= BTN_Y && y < BTN_Y + BTN_SIZE {
                self.go_forward();
                return;
            }
            // Reload button
            if x >= 66.0 && x < 66.0 + BTN_SIZE && y >= BTN_Y && y < BTN_Y + BTN_SIZE {
                self.reload();
                return;
            }
            // Address bar
            if x >= ADDR_BAR_LEFT && y >= ADDR_BAR_TOP && y < ADDR_BAR_TOP + ADDR_BAR_HEIGHT {
                self.address_focused = true;
                self.address_all_selected = true;
                self.cursor_pos = self.address_text.len();
                self.request_redraw();
                return;
            }
        } else {
            // Click outside address bar unfocuses it
            self.address_focused = false;

            // Check for link clicks (y is relative to window, flat_boxes are relative to page)
            let page_y = y - TOOLBAR_HEIGHT as f32;
            for fbox in &self.flat_boxes {
                if let Some(ref href) = fbox.link_href {
                    if x >= fbox.x && x <= fbox.x + fbox.width
                        && page_y >= fbox.y && page_y <= fbox.y + fbox.height
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
            Key::Named(NamedKey::Delete) => {
                if self.cursor_pos < self.address_text.len() {
                    self.address_text.remove(self.cursor_pos);
                    self.request_redraw();
                }
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

    fn log_network(&self, method: &str, url: &str, status: Option<u16>, content_type: &str, size: usize, error: Option<&str>) {
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

        let viewport = self.window.as_ref()
            .map(|w| { let s = w.inner_size(); (s.width, s.height) })
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

        if let Ok(png_data) = full_pixmap.encode_png() {
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
            .with_inner_size(winit::dpi::LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));

        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));

        let context =
            softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("surface");

        self.window = Some(window);
        self.surface = Some(surface);

        // Initial page setup: fetch external CSS, execute scripts, render text first
        let url = self.current_url.clone();
        let html = self.html_content.clone();
        self.fetch_external_css(&url, &html);
        self.execute_scripts();
        self.render();

        // Fetch images in background
        self.fetch_page_images_async(&url, &html);
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
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                // We'll get position from CursorMoved
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Store cursor position for click handling
                // We handle click in MouseInput but need position -- use a stored pos
                self.last_cursor = Some((position.x, position.y));
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                if let Some((x, y)) = self.last_cursor {
                    self.handle_click(x, y);
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
            event_loop.set_control_flow(
                winit::event_loop::ControlFlow::wait_duration(Duration::from_millis(100))
            );
        }
    }
}

// --- Toolbar drawing helpers ---

fn draw_toolbar_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: u8, g: u8, b: u8) {
    if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(r, g, b, 255));
        let path = PathBuilder::from_rect(rect);
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn draw_nav_button(pixmap: &mut Pixmap, x: f32, y: f32, label: &str, enabled: bool) {
    let (bg_r, bg_g, bg_b) = if enabled { (0xe0, 0xe0, 0xe0) } else { (0xf0, 0xf0, 0xf0) };
    let (fg_r, fg_g, fg_b) = if enabled { (0x33, 0x33, 0x33) } else { (0xbb, 0xbb, 0xbb) };

    // Button background
    draw_toolbar_rect(pixmap, x, y, BTN_SIZE, BTN_SIZE, bg_r, bg_g, bg_b);
    // Button border
    draw_toolbar_rect(pixmap, x, y, BTN_SIZE, 1.0, 0xcc, 0xcc, 0xcc);
    draw_toolbar_rect(pixmap, x, y + BTN_SIZE - 1.0, BTN_SIZE, 1.0, 0xcc, 0xcc, 0xcc);
    draw_toolbar_rect(pixmap, x, y, 1.0, BTN_SIZE, 0xcc, 0xcc, 0xcc);
    draw_toolbar_rect(pixmap, x + BTN_SIZE - 1.0, y, 1.0, BTN_SIZE, 0xcc, 0xcc, 0xcc);

    // Draw label centered
    let label_w = measure_toolbar_text(label);
    let text_x = x + (BTN_SIZE - label_w) / 2.0;
    let text_y = y + 7.0;
    draw_toolbar_text(pixmap, text_x, text_y, label, fg_r, fg_g, fg_b);
}

fn draw_address_bar(
    pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32,
    text: &str, focused: bool, cursor_pos: usize, all_selected: bool,
) {
    // White background
    draw_toolbar_rect(pixmap, x, y, w, h, 0xff, 0xff, 0xff);
    // Border
    let (br, bg, bb) = if focused { (0x44, 0x88, 0xee) } else { (0xaa, 0xaa, 0xaa) };
    draw_toolbar_rect(pixmap, x, y, w, 1.0, br, bg, bb);
    draw_toolbar_rect(pixmap, x, y + h - 1.0, w, 1.0, br, bg, bb);
    draw_toolbar_rect(pixmap, x, y, 1.0, h, br, bg, bb);
    draw_toolbar_rect(pixmap, x + w - 1.0, y, 1.0, h, br, bg, bb);
    if focused {
        // Double-thick border
        draw_toolbar_rect(pixmap, x + 1.0, y + 1.0, w - 2.0, 1.0, br, bg, bb);
        draw_toolbar_rect(pixmap, x + 1.0, y + h - 2.0, w - 2.0, 1.0, br, bg, bb);
        draw_toolbar_rect(pixmap, x + 1.0, y + 1.0, 1.0, h - 2.0, br, bg, bb);
        draw_toolbar_rect(pixmap, x + w - 2.0, y + 1.0, 1.0, h - 2.0, br, bg, bb);
    }

    // Text (clipped to bar width)
    let padding = 6.0;
    let max_text_w = w - padding * 2.0;

    // Trim text from the left if it overflows
    let mut display_text = text;
    while measure_toolbar_text(display_text) > max_text_w && display_text.len() > 1 {
        display_text = &display_text[1..];
    }

    // Selection highlight
    if focused && all_selected && !text.is_empty() {
        let sel_w = measure_toolbar_text(display_text);
        draw_toolbar_rect(pixmap, x + padding, y + 4.0, sel_w.min(max_text_w), h - 8.0,
            0x33, 0x66, 0xcc);
        draw_toolbar_text(pixmap, x + padding, y + 7.0, display_text, 0xff, 0xff, 0xff);
    } else {
        draw_toolbar_text(pixmap, x + padding, y + 7.0, display_text, 0x22, 0x22, 0x22);
    }

    // Cursor
    if focused && !all_selected {
        // If text was trimmed from left, adjust
        let trim_offset = text.len() - display_text.len();
        let visible_cursor_text = if cursor_pos > trim_offset {
            &text[trim_offset..cursor_pos]
        } else {
            ""
        };
        let cx = x + padding + measure_toolbar_text(visible_cursor_text);
        draw_toolbar_rect(pixmap, cx, y + 5.0, 1.5, h - 10.0, 0x00, 0x00, 0x00);
    }
}

// -- Toolbar TTF font --

static TOOLBAR_FONT: OnceLock<Option<FontVec>> = OnceLock::new();

fn get_toolbar_font() -> Option<&'static FontVec> {
    TOOLBAR_FONT.get_or_init(|| {
        let paths = [
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        for path in &paths {
            if let Ok(data) = std::fs::read(path) {
                if let Ok(font) = FontVec::try_from_vec(data) {
                    return Some(font);
                }
            }
        }
        None
    }).as_ref()
}

fn measure_toolbar_text(text: &str) -> f32 {
    if let Some(font) = get_toolbar_font() {
        let scale = PxScale::from(13.0);
        let scaled = font.as_scaled(scale);
        text.chars().map(|c| scaled.h_advance(scaled.glyph_id(c))).sum()
    } else {
        text.len() as f32 * 7.0
    }
}

fn draw_toolbar_text(pixmap: &mut Pixmap, x: f32, y: f32, text: &str, r: u8, g: u8, b: u8) {
    if let Some(font) = get_toolbar_font() {
        draw_toolbar_text_ttf(pixmap, x, y, text, r, g, b, font);
    } else {
        draw_toolbar_text_bitmap(pixmap, x, y, text, r, g, b);
    }
}

fn draw_toolbar_text_ttf(pixmap: &mut Pixmap, x: f32, y: f32, text: &str, r: u8, g: u8, b: u8, font: &FontVec) {
    let font_size = 13.0;
    let scale = PxScale::from(font_size);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();
    let mut cx = x;

    for ch in text.chars() {
        let glyph_id = scaled.glyph_id(ch);
        let glyph = glyph_id.with_scale_and_position(scale, point(cx, y + ascent));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = gx as i32 + bounds.min.x as i32;
                let py = gy as i32 + bounds.min.y as i32;
                if px >= 0 && py >= 0 {
                    let px = px as u32;
                    let py = py as u32;
                    if px < pixmap.width() && py < pixmap.height() {
                        let alpha = (coverage * 255.0) as u8;
                        if alpha > 0 {
                            let w = pixmap.width();
                            let idx = ((py * w + px) * 4) as usize;
                            let data = pixmap.data_mut();
                            if idx + 3 < data.len() {
                                let sa = alpha as u32;
                                let inv = 255 - sa;
                                data[idx]     = ((r as u32 * sa + data[idx] as u32 * inv) / 255) as u8;
                                data[idx + 1] = ((g as u32 * sa + data[idx + 1] as u32 * inv) / 255) as u8;
                                data[idx + 2] = ((b as u32 * sa + data[idx + 2] as u32 * inv) / 255) as u8;
                                data[idx + 3] = 255;
                            }
                        }
                    }
                }
            });
        }
        cx += scaled.h_advance(glyph_id);
    }
}

fn draw_toolbar_text_bitmap(pixmap: &mut Pixmap, x: f32, y: f32, text: &str, r: u8, g: u8, b: u8) {
    let char_w = 7.0;
    let scale = 0.75;
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
        'A' => vec![(1.0,12.0,4.0,2.0),(4.0,2.0,7.0,12.0),(2.5,8.0,5.5,8.0)],
        'a' => vec![(7.0,5.0,7.0,12.0),(7.0,5.0,4.0,5.0),(4.0,5.0,2.0,7.0),(2.0,7.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,12.0)],
        'B' => vec![(2.0,2.0,2.0,12.0),(2.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,6.0,7.0),(2.0,7.0,6.0,7.0),(6.0,7.0,7.0,9.0),(7.0,9.0,6.0,12.0),(2.0,12.0,6.0,12.0)],
        'b' => vec![(2.0,2.0,2.0,12.0),(2.0,7.0,5.0,5.0),(5.0,5.0,7.0,7.0),(7.0,7.0,7.0,10.0),(7.0,10.0,5.0,12.0),(2.0,12.0,5.0,12.0)],
        'C' => vec![(7.0,3.0,4.0,2.0),(4.0,2.0,2.0,4.0),(2.0,4.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,11.0)],
        'c' => vec![(7.0,6.0,5.0,5.0),(5.0,5.0,2.0,7.0),(2.0,7.0,2.0,10.0),(2.0,10.0,5.0,12.0),(5.0,12.0,7.0,11.0)],
        'D' => vec![(2.0,2.0,2.0,12.0),(2.0,2.0,5.0,2.0),(5.0,2.0,7.0,4.0),(7.0,4.0,7.0,10.0),(7.0,10.0,5.0,12.0),(2.0,12.0,5.0,12.0)],
        'd' => vec![(7.0,2.0,7.0,12.0),(7.0,7.0,4.0,5.0),(4.0,5.0,2.0,7.0),(2.0,7.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,12.0)],
        'E' => vec![(2.0,2.0,2.0,12.0),(2.0,2.0,7.0,2.0),(2.0,7.0,6.0,7.0),(2.0,12.0,7.0,12.0)],
        'e' => vec![(2.0,8.0,7.0,8.0),(7.0,8.0,7.0,6.0),(7.0,6.0,4.0,5.0),(4.0,5.0,2.0,7.0),(2.0,7.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,11.0)],
        'F' => vec![(2.0,2.0,2.0,12.0),(2.0,2.0,7.0,2.0),(2.0,7.0,6.0,7.0)],
        'f' => vec![(6.0,3.0,5.0,2.0),(5.0,2.0,4.0,4.0),(4.0,4.0,4.0,12.0),(2.0,6.0,6.0,6.0)],
        'G' => vec![(7.0,3.0,4.0,2.0),(4.0,2.0,2.0,4.0),(2.0,4.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,10.0),(7.0,10.0,7.0,7.0),(5.0,7.0,7.0,7.0)],
        'g' => vec![(2.0,7.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,12.0),(7.0,5.0,7.0,13.0),(7.0,13.0,5.0,14.0),(5.0,14.0,2.0,13.0),(7.0,5.0,4.0,5.0),(4.0,5.0,2.0,7.0)],
        'H' => vec![(2.0,2.0,2.0,12.0),(7.0,2.0,7.0,12.0),(2.0,7.0,7.0,7.0)],
        'h' => vec![(2.0,2.0,2.0,12.0),(2.0,7.0,5.0,5.0),(5.0,5.0,7.0,7.0),(7.0,7.0,7.0,12.0)],
        'I' => vec![(3.0,2.0,6.0,2.0),(4.5,2.0,4.5,12.0),(3.0,12.0,6.0,12.0)],
        'i' => vec![(4.0,3.0,5.0,4.0),(4.0,6.0,4.0,12.0)],
        'J' => vec![(4.0,2.0,7.0,2.0),(6.0,2.0,6.0,10.0),(6.0,10.0,4.0,12.0),(4.0,12.0,2.0,10.0)],
        'j' => vec![(5.0,3.0,6.0,4.0),(5.0,6.0,5.0,12.0),(5.0,12.0,3.0,14.0),(3.0,14.0,2.0,13.0)],
        'K' => vec![(2.0,2.0,2.0,12.0),(7.0,2.0,2.0,7.0),(2.0,7.0,7.0,12.0)],
        'k' => vec![(2.0,2.0,2.0,12.0),(7.0,5.0,2.0,9.0),(2.0,9.0,7.0,12.0)],
        'L' => vec![(2.0,2.0,2.0,12.0),(2.0,12.0,7.0,12.0)],
        'l' => vec![(4.0,2.0,4.0,12.0),(4.0,12.0,6.0,12.0)],
        'M' => vec![(1.0,12.0,1.0,2.0),(1.0,2.0,4.0,7.0),(4.0,7.0,7.0,2.0),(7.0,2.0,7.0,12.0)],
        'm' => vec![(1.0,12.0,1.0,5.0),(1.0,6.0,3.0,5.0),(3.0,5.0,4.0,6.0),(4.0,6.0,4.0,12.0),(4.0,6.0,6.0,5.0),(6.0,5.0,7.0,6.0),(7.0,6.0,7.0,12.0)],
        'N' => vec![(2.0,12.0,2.0,2.0),(2.0,2.0,7.0,12.0),(7.0,12.0,7.0,2.0)],
        'n' => vec![(2.0,12.0,2.0,5.0),(2.0,6.0,5.0,5.0),(5.0,5.0,7.0,7.0),(7.0,7.0,7.0,12.0)],
        'O' => vec![(3.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,7.0,10.0),(7.0,10.0,6.0,12.0),(3.0,12.0,6.0,12.0),(3.0,12.0,2.0,10.0),(2.0,10.0,2.0,4.0),(2.0,4.0,3.0,2.0)],
        'o' => vec![(3.0,5.0,6.0,5.0),(6.0,5.0,7.0,7.0),(7.0,7.0,7.0,10.0),(7.0,10.0,6.0,12.0),(3.0,12.0,6.0,12.0),(3.0,12.0,2.0,10.0),(2.0,10.0,2.0,7.0),(2.0,7.0,3.0,5.0)],
        'P' => vec![(2.0,2.0,2.0,12.0),(2.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,6.0,7.0),(2.0,7.0,6.0,7.0)],
        'p' => vec![(2.0,5.0,2.0,14.0),(2.0,7.0,5.0,5.0),(5.0,5.0,7.0,7.0),(7.0,7.0,7.0,10.0),(7.0,10.0,5.0,12.0),(2.0,12.0,5.0,12.0)],
        'Q' => vec![(3.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,7.0,10.0),(7.0,10.0,6.0,12.0),(3.0,12.0,6.0,12.0),(3.0,12.0,2.0,10.0),(2.0,10.0,2.0,4.0),(2.0,4.0,3.0,2.0),(5.0,9.0,7.0,13.0)],
        'q' => vec![(7.0,5.0,7.0,14.0),(7.0,7.0,4.0,5.0),(4.0,5.0,2.0,7.0),(2.0,7.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,12.0)],
        'R' => vec![(2.0,2.0,2.0,12.0),(2.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,6.0,7.0),(2.0,7.0,6.0,7.0),(4.0,7.0,7.0,12.0)],
        'r' => vec![(2.0,5.0,2.0,12.0),(2.0,6.0,5.0,5.0),(5.0,5.0,7.0,6.0)],
        'S' => vec![(7.0,3.0,4.0,2.0),(4.0,2.0,2.0,4.0),(2.0,4.0,3.0,6.0),(3.0,6.0,6.0,8.0),(6.0,8.0,7.0,10.0),(7.0,10.0,4.0,12.0),(4.0,12.0,2.0,11.0)],
        's' => vec![(7.0,6.0,5.0,5.0),(5.0,5.0,2.0,7.0),(2.0,7.0,7.0,10.0),(7.0,10.0,5.0,12.0),(5.0,12.0,2.0,11.0)],
        'T' => vec![(1.0,2.0,8.0,2.0),(4.5,2.0,4.5,12.0)],
        't' => vec![(4.0,2.0,4.0,10.0),(4.0,10.0,6.0,12.0),(6.0,12.0,7.0,12.0),(2.0,6.0,6.0,6.0)],
        'U' => vec![(2.0,2.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,10.0),(7.0,10.0,7.0,2.0)],
        'u' => vec![(2.0,5.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,12.0),(7.0,5.0,7.0,12.0)],
        'V' => vec![(1.0,2.0,4.0,12.0),(4.0,12.0,7.0,2.0)],
        'v' => vec![(2.0,5.0,4.5,12.0),(4.5,12.0,7.0,5.0)],
        'W' => vec![(0.0,2.0,2.0,12.0),(2.0,12.0,4.0,7.0),(4.0,7.0,6.0,12.0),(6.0,12.0,8.0,2.0)],
        'w' => vec![(1.0,5.0,2.5,12.0),(2.5,12.0,4.0,7.0),(4.0,7.0,5.5,12.0),(5.5,12.0,7.0,5.0)],
        'X' => vec![(2.0,2.0,7.0,12.0),(7.0,2.0,2.0,12.0)],
        'x' => vec![(2.0,5.0,7.0,12.0),(7.0,5.0,2.0,12.0)],
        'Y' => vec![(1.0,2.0,4.0,7.0),(7.0,2.0,4.0,7.0),(4.0,7.0,4.0,12.0)],
        'y' => vec![(2.0,5.0,4.5,10.0),(7.0,5.0,4.5,10.0),(4.5,10.0,3.0,14.0)],
        'Z' => vec![(2.0,2.0,7.0,2.0),(7.0,2.0,2.0,12.0),(2.0,12.0,7.0,12.0)],
        'z' => vec![(2.0,5.0,7.0,5.0),(7.0,5.0,2.0,12.0),(2.0,12.0,7.0,12.0)],
        '0' => vec![(3.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,7.0,10.0),(7.0,10.0,6.0,12.0),(3.0,12.0,6.0,12.0),(3.0,12.0,2.0,10.0),(2.0,10.0,2.0,4.0),(2.0,4.0,3.0,2.0)],
        '1' => vec![(3.0,4.0,4.5,2.0),(4.5,2.0,4.5,12.0),(3.0,12.0,6.0,12.0)],
        '2' => vec![(2.0,4.0,3.0,2.0),(3.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,2.0,12.0),(2.0,12.0,7.0,12.0)],
        '3' => vec![(2.0,3.0,3.0,2.0),(3.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,5.0,7.0),(5.0,7.0,7.0,10.0),(7.0,10.0,6.0,12.0),(3.0,12.0,6.0,12.0),(3.0,12.0,2.0,11.0)],
        '4' => vec![(6.0,2.0,2.0,8.0),(2.0,8.0,7.0,8.0),(6.0,2.0,6.0,12.0)],
        '5' => vec![(7.0,2.0,2.0,2.0),(2.0,2.0,2.0,6.0),(2.0,6.0,6.0,6.0),(6.0,6.0,7.0,9.0),(7.0,9.0,6.0,12.0),(3.0,12.0,6.0,12.0),(3.0,12.0,2.0,11.0)],
        '6' => vec![(6.0,2.0,3.0,2.0),(3.0,2.0,2.0,4.0),(2.0,4.0,2.0,10.0),(2.0,10.0,3.0,12.0),(3.0,12.0,6.0,12.0),(6.0,12.0,7.0,10.0),(7.0,10.0,7.0,7.0),(7.0,7.0,6.0,6.0),(2.0,6.0,6.0,6.0)],
        '7' => vec![(2.0,2.0,7.0,2.0),(7.0,2.0,4.0,12.0)],
        '8' => vec![(3.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,6.0,6.0),(3.0,6.0,6.0,6.0),(3.0,6.0,2.0,4.0),(2.0,4.0,3.0,2.0),(3.0,6.0,2.0,9.0),(2.0,9.0,3.0,12.0),(3.0,12.0,6.0,12.0),(6.0,12.0,7.0,9.0),(7.0,9.0,6.0,6.0)],
        '9' => vec![(7.0,6.0,6.0,2.0),(6.0,2.0,3.0,2.0),(3.0,2.0,2.0,4.0),(2.0,4.0,2.0,5.0),(2.0,5.0,3.0,7.0),(3.0,7.0,7.0,7.0),(7.0,2.0,7.0,10.0),(7.0,10.0,4.0,12.0)],
        '.' => vec![(4.0,11.0,5.0,11.0),(4.0,11.0,4.0,12.0),(5.0,11.0,5.0,12.0),(4.0,12.0,5.0,12.0)],
        ',' => vec![(4.5,10.0,4.5,12.0),(4.5,12.0,3.5,13.0)],
        ':' => vec![(4.0,4.0,5.0,5.0),(4.0,10.0,5.0,11.0)],
        '/' => vec![(7.0,2.0,2.0,12.0)],
        '\\' => vec![(2.0,2.0,7.0,12.0)],
        '-' => vec![(2.0,7.0,7.0,7.0)],
        '_' => vec![(1.0,12.0,8.0,12.0)],
        '=' => vec![(2.0,5.0,7.0,5.0),(2.0,9.0,7.0,9.0)],
        '?' => vec![(2.0,3.0,3.0,2.0),(3.0,2.0,6.0,2.0),(6.0,2.0,7.0,4.0),(7.0,4.0,4.5,7.0),(4.5,7.0,4.5,9.0),(4.0,11.0,5.0,12.0)],
        '!' => vec![(4.5,2.0,4.5,9.0),(4.0,11.0,5.0,12.0)],
        '<' => vec![(7.0,3.0,2.0,7.0),(2.0,7.0,7.0,11.0)],
        '>' => vec![(2.0,3.0,7.0,7.0),(7.0,7.0,2.0,11.0)],
        '(' => vec![(5.0,1.0,3.0,4.0),(3.0,4.0,3.0,10.0),(3.0,10.0,5.0,13.0)],
        ')' => vec![(3.0,1.0,5.0,4.0),(5.0,4.0,5.0,10.0),(5.0,10.0,3.0,13.0)],
        '[' => vec![(3.0,1.0,6.0,1.0),(3.0,1.0,3.0,13.0),(3.0,13.0,6.0,13.0)],
        ']' => vec![(3.0,1.0,6.0,1.0),(6.0,1.0,6.0,13.0),(3.0,13.0,6.0,13.0)],
        '#' => vec![(3.0,3.0,3.0,11.0),(6.0,3.0,6.0,11.0),(1.0,5.0,8.0,5.0),(1.0,9.0,8.0,9.0)],
        '@' => vec![(7.0,4.0,4.0,2.0),(4.0,2.0,2.0,4.0),(2.0,4.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,10.0),(5.0,5.0,5.0,9.0),(5.0,9.0,7.0,9.0),(7.0,4.0,7.0,9.0)],
        '&' => vec![(5.0,2.0,3.0,2.0),(3.0,2.0,2.0,4.0),(2.0,4.0,3.0,6.0),(3.0,6.0,2.0,10.0),(2.0,10.0,4.0,12.0),(4.0,12.0,7.0,10.0),(3.0,6.0,7.0,9.0)],
        '+' => vec![(4.5,3.0,4.5,11.0),(2.0,7.0,7.0,7.0)],
        '*' => vec![(4.5,3.0,4.5,11.0),(2.0,5.0,7.0,9.0),(2.0,9.0,7.0,5.0)],
        '%' => vec![(2.0,2.0,3.0,3.0),(7.0,2.0,2.0,12.0),(6.0,11.0,7.0,12.0)],
        '^' => vec![(2.0,5.0,4.5,2.0),(4.5,2.0,7.0,5.0)],
        '~' => vec![(1.0,7.0,3.0,5.0),(3.0,5.0,5.0,7.0),(5.0,7.0,7.0,5.0)],
        '|' => vec![(4.5,1.0,4.5,13.0)],
        '\'' | '"' => vec![(4.0,2.0,4.0,4.0)],
        '{' => vec![(5.0,1.0,4.0,2.0),(4.0,2.0,4.0,5.0),(4.0,5.0,3.0,7.0),(3.0,7.0,4.0,9.0),(4.0,9.0,4.0,12.0),(4.0,12.0,5.0,13.0)],
        '}' => vec![(4.0,1.0,5.0,2.0),(5.0,2.0,5.0,5.0),(5.0,5.0,6.0,7.0),(6.0,7.0,5.0,9.0),(5.0,9.0,5.0,12.0),(5.0,12.0,4.0,13.0)],
        '$' => vec![(6.0,3.0,3.0,3.0),(3.0,3.0,2.0,5.0),(2.0,5.0,6.0,8.0),(6.0,8.0,7.0,10.0),(7.0,10.0,3.0,12.0),(4.5,1.0,4.5,13.0)],
        '`' => vec![(3.0,2.0,5.0,4.0)],
        _ => vec![(2.0,2.0,7.0,2.0),(7.0,2.0,7.0,12.0),(7.0,12.0,2.0,12.0),(2.0,12.0,2.0,2.0)],
    }
}

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let mcp_mode = args.iter().any(|a| a == "--mcp");

    // Parse URL arg (skip --mcp)
    let url_arg = args.iter().skip(1).find(|a| *a != "--mcp");
    let (initial_url, initial_html) = if let Some(input) = url_arg {
        match fetch_url(input) {
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
