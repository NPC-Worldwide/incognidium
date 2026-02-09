use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout};
use incognidium_paint::paint;
use incognidium_style::resolve_styles;

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;

struct App {
    html_content: String,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl App {
    fn new(html_content: String) -> Self {
        App {
            html_content,
            window: None,
            surface: None,
        }
    }

    fn render(&mut self) {
        let window = match &self.window {
            Some(w) => w,
            None => return,
        };
        let surface = match &mut self.surface {
            Some(s) => s,
            None => return,
        };

        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        // Parse HTML
        let doc = parse_html(&self.html_content);

        // Parse CSS from <style> elements
        let css_text = doc.collect_style_text();
        let stylesheet = parse_css(&css_text);

        // Resolve styles
        let styles = resolve_styles(&doc, &stylesheet);

        // Layout
        let layout_root = layout(&doc, &styles, width as f32, height as f32);

        // Flatten
        let flat_boxes = flatten_layout(&layout_root, 0.0, 0.0);

        // Paint
        let pixmap = paint(&flat_boxes, &styles, width, height);

        // Copy to window surface
        surface
            .resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            )
            .expect("failed to resize surface");

        let mut buffer = surface.buffer_mut().expect("failed to get buffer");

        // Convert RGBA (tiny-skia) to 0RGB (softbuffer)
        let data = pixmap.data();
        for y in 0..height {
            for x in 0..width {
                let src_idx = ((y * width + x) * 4) as usize;
                if src_idx + 3 < data.len() {
                    let r = data[src_idx] as u32;
                    let g = data[src_idx + 1] as u32;
                    let b = data[src_idx + 2] as u32;
                    buffer[(y * width + x) as usize] = (r << 16) | (g << 8) | b;
                }
            }
        }

        buffer.present().expect("failed to present buffer");
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

        let window = Rc::new(event_loop.create_window(attrs).expect("failed to create window"));

        let context =
            softbuffer::Context::new(window.clone()).expect("failed to create softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("failed to create surface");

        self.window = Some(window);
        self.surface = Some(surface);

        self.render();
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
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let html_content = if args.len() > 1 {
        std::fs::read_to_string(&args[1]).unwrap_or_else(|e| {
            eprintln!("Failed to read {}: {}", args[1], e);
            default_html().to_string()
        })
    } else {
        eprintln!("Usage: incognidium-shell <file.html>");
        eprintln!("No file specified, using built-in test page.");
        default_html().to_string()
    };

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new(html_content);
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
    <h2>Color Test</h2>
    <p class="red">This text should be red.</p>
    <p class="blue">This text should be blue.</p>
    <p class="green">This text should be green.</p>
</div>
</body>
</html>"#
}
