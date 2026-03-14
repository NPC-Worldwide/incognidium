//! DOM bindings for Murkiu JS engine.
//!
//! Provides `document` global and DOM manipulation APIs:
//! - document.getElementById(id)
//! - document.createElement(tag)
//! - document.body
//! - element.innerHTML (get/set)
//! - element.textContent (get/set)
//! - element.setAttribute(name, value)
//! - element.getAttribute(name)
//! - element.style.* (set)
//! - element.appendChild(child)
//! - element.addEventListener(event, handler)
//!
//! Canvas 2D API:
//! - canvas.getContext("2d")
//! - ctx.fillRect(x, y, w, h)
//! - ctx.strokeRect(x, y, w, h)
//! - ctx.clearRect(x, y, w, h)
//! - ctx.fillText(text, x, y)
//! - ctx.beginPath() / moveTo / lineTo / arc / closePath / fill / stroke
//! - ctx.fillStyle / ctx.strokeStyle / ctx.lineWidth

use incognidium_dom::*;
use murkiu_vm::{JsValue, Vm, JsObject};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ─── Canvas 2D State ──────────────────────────────────────────────────────

/// RGBA pixel buffer for a canvas element.
pub struct CanvasState {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub fill_color: [u8; 4],
    pub stroke_color: [u8; 4],
    pub line_width: f32,
    path: Vec<PathCmd>,
    path_x: f32,
    path_y: f32,
}

#[derive(Clone)]
enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    Arc(f32, f32, f32, f32, f32), // cx, cy, r, start, end
    ClosePath,
}

impl CanvasState {
    pub fn new(width: u32, height: u32) -> Self {
        CanvasState {
            pixels: vec![0u8; (width * height * 4) as usize],
            width,
            height,
            fill_color: [0, 0, 0, 255],
            stroke_color: [0, 0, 0, 255],
            line_width: 1.0,
            path: Vec::new(),
            path_x: 0.0,
            path_y: 0.0,
        }
    }

    fn set_pixel(&mut self, x: i32, y: i32, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let idx = ((y as u32 * self.width + x as u32) * 4) as usize;
        if idx + 3 < self.pixels.len() {
            let sa = color[3] as u32;
            if sa == 255 {
                self.pixels[idx] = color[0];
                self.pixels[idx + 1] = color[1];
                self.pixels[idx + 2] = color[2];
                self.pixels[idx + 3] = 255;
            } else if sa > 0 {
                let inv_a = 255 - sa;
                self.pixels[idx] = ((color[0] as u32 * sa + self.pixels[idx] as u32 * inv_a) / 255) as u8;
                self.pixels[idx + 1] = ((color[1] as u32 * sa + self.pixels[idx + 1] as u32 * inv_a) / 255) as u8;
                self.pixels[idx + 2] = ((color[2] as u32 * sa + self.pixels[idx + 2] as u32 * inv_a) / 255) as u8;
                self.pixels[idx + 3] = 255;
            }
        }
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let x0 = x as i32;
        let y0 = y as i32;
        let x1 = (x + w) as i32;
        let y1 = (y + h) as i32;
        let color = self.fill_color;
        for py in y0..y1 {
            for px in x0..x1 {
                self.set_pixel(px, py, color);
            }
        }
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let lw = self.line_width.max(1.0) as i32;
        let color = self.stroke_color;
        // Top
        for dy in 0..lw {
            for px in x as i32..(x + w) as i32 {
                self.set_pixel(px, y as i32 + dy, color);
            }
        }
        // Bottom
        for dy in 0..lw {
            for px in x as i32..(x + w) as i32 {
                self.set_pixel(px, (y + h) as i32 - 1 - dy, color);
            }
        }
        // Left
        for dy in y as i32..(y + h) as i32 {
            for dx in 0..lw {
                self.set_pixel(x as i32 + dx, dy, color);
            }
        }
        // Right
        for dy in y as i32..(y + h) as i32 {
            for dx in 0..lw {
                self.set_pixel((x + w) as i32 - 1 - dx, dy, color);
            }
        }
    }

    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let x0 = x as i32;
        let y0 = y as i32;
        let x1 = (x + w) as i32;
        let y1 = (y + h) as i32;
        for py in y0..y1 {
            for px in x0..x1 {
                if px < 0 || py < 0 || px >= self.width as i32 || py >= self.height as i32 {
                    continue;
                }
                let idx = ((py as u32 * self.width + px as u32) * 4) as usize;
                if idx + 3 < self.pixels.len() {
                    self.pixels[idx] = 0;
                    self.pixels[idx + 1] = 0;
                    self.pixels[idx + 2] = 0;
                    self.pixels[idx + 3] = 0;
                }
            }
        }
    }

    fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let steps = dx.max(dy).max(1.0) as u32;
        let lw = (self.line_width / 2.0).max(0.5);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let px = x0 + (x1 - x0) * t;
            let py = y0 + (y1 - y0) * t;
            // Draw thick point
            let ilw = lw.ceil() as i32;
            for dy in -ilw..=ilw {
                for dx in -ilw..=ilw {
                    self.set_pixel(px as i32 + dx, py as i32 + dy, color);
                }
            }
        }
    }

    pub fn fill_path(&mut self) {
        // Simplified: collect path points, fill using scanline
        let points = self.collect_path_points();
        if points.is_empty() {
            return;
        }
        // Find bounding box
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for &(_, y) in &points {
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let color = self.fill_color;
        // Simple scanline fill
        for y in min_y as i32..=max_y as i32 {
            let mut intersections = Vec::new();
            let yf = y as f32 + 0.5;
            for i in 0..points.len() {
                let j = (i + 1) % points.len();
                let (x0, y0) = points[i];
                let (x1, y1) = points[j];
                if (y0 <= yf && y1 > yf) || (y1 <= yf && y0 > yf) {
                    let t = (yf - y0) / (y1 - y0);
                    intersections.push(x0 + t * (x1 - x0));
                }
            }
            intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for pair in intersections.chunks(2) {
                if pair.len() == 2 {
                    for x in pair[0] as i32..=pair[1] as i32 {
                        self.set_pixel(x, y, color);
                    }
                }
            }
        }
    }

    pub fn stroke_path(&mut self) {
        let color = self.stroke_color;
        let mut last_x = 0.0f32;
        let mut last_y = 0.0f32;
        let cmds = self.path.clone();
        for cmd in &cmds {
            match cmd {
                PathCmd::MoveTo(x, y) => {
                    last_x = *x;
                    last_y = *y;
                }
                PathCmd::LineTo(x, y) => {
                    self.draw_line(last_x, last_y, *x, *y, color);
                    last_x = *x;
                    last_y = *y;
                }
                PathCmd::Arc(cx, cy, r, start, end) => {
                    let steps = (r * (end - start).abs()).max(20.0) as u32;
                    let mut prev_ax = *cx + r * start.cos();
                    let mut prev_ay = *cy + r * start.sin();
                    for i in 1..=steps {
                        let t = *start + (*end - *start) * (i as f32 / steps as f32);
                        let ax = *cx + r * t.cos();
                        let ay = *cy + r * t.sin();
                        self.draw_line(prev_ax, prev_ay, ax, ay, color);
                        prev_ax = ax;
                        prev_ay = ay;
                    }
                    last_x = prev_ax;
                    last_y = prev_ay;
                }
                PathCmd::ClosePath => {
                    // Close handled by collecting first point
                }
            }
        }
    }

    fn collect_path_points(&self) -> Vec<(f32, f32)> {
        let mut points = Vec::new();
        for cmd in &self.path {
            match cmd {
                PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => {
                    points.push((*x, *y));
                }
                PathCmd::Arc(cx, cy, r, start, end) => {
                    let steps = (r * (end - start).abs()).max(20.0) as u32;
                    for i in 0..=steps {
                        let t = *start + (*end - *start) * (i as f32 / steps as f32);
                        points.push((*cx + r * t.cos(), *cy + r * t.sin()));
                    }
                }
                PathCmd::ClosePath => {
                    if let Some(&first) = points.first() {
                        points.push(first);
                    }
                }
            }
        }
        points
    }

    pub fn fill_text(&mut self, text: &str, x: f32, y: f32) {
        // Simple 5x7 pixel font for canvas text
        let color = self.fill_color;
        let mut cx = x;
        for ch in text.chars() {
            let bitmap = canvas_char_bitmap(ch);
            for (row_idx, row) in bitmap.iter().enumerate() {
                for col in 0..5 {
                    if (row >> (4 - col)) & 1 == 1 {
                        self.set_pixel(cx as i32 + col, y as i32 + row_idx as i32, color);
                    }
                }
            }
            cx += 6.0;
        }
    }
}

/// Simple 5x7 bitmap font for canvas fillText.
fn canvas_char_bitmap(ch: char) -> [u8; 7] {
    match ch {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b11110, 0b10001, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b01110, 0b00001],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01110, 0b10001, 0b10000, 0b01110, 0b00001, 0b10001, 0b01110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        'a'..='z' => canvas_char_bitmap((ch as u8 - b'a' + b'A') as char),
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
        '3' => [0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b01110, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b00100, 0b00100, 0b00100],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b01000],
        ':' => [0b00000, 0b00100, 0b00000, 0b00000, 0b00000, 0b00100, 0b00000],
        '=' => [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        ' ' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

/// Parse a CSS color string into RGBA.
fn parse_css_color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim();
    // Named colors
    match s.to_lowercase().as_str() {
        "red" => return Some([255, 0, 0, 255]),
        "green" => return Some([0, 128, 0, 255]),
        "blue" => return Some([0, 0, 255, 255]),
        "white" => return Some([255, 255, 255, 255]),
        "black" => return Some([0, 0, 0, 255]),
        "yellow" => return Some([255, 255, 0, 255]),
        "cyan" => return Some([0, 255, 255, 255]),
        "magenta" => return Some([255, 0, 255, 255]),
        "orange" => return Some([255, 165, 0, 255]),
        "purple" => return Some([128, 0, 128, 255]),
        "gray" | "grey" => return Some([128, 128, 128, 255]),
        "transparent" => return Some([0, 0, 0, 0]),
        _ => {}
    }
    // #RGB or #RRGGBB
    if s.starts_with('#') {
        let hex = &s[1..];
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            return Some([r, g, b, 255]);
        }
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some([r, g, b, 255]);
        }
    }
    // rgb(r, g, b)
    if s.starts_with("rgb(") && s.ends_with(')') {
        let inner = &s[4..s.len() - 1];
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
            let r = parts[0].trim().parse::<u8>().ok()?;
            let g = parts[1].trim().parse::<u8>().ok()?;
            let b = parts[2].trim().parse::<u8>().ok()?;
            return Some([r, g, b, 255]);
        }
    }
    // rgba(r, g, b, a)
    if s.starts_with("rgba(") && s.ends_with(')') {
        let inner = &s[5..s.len() - 1];
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 4 {
            let r = parts[0].trim().parse::<u8>().ok()?;
            let g = parts[1].trim().parse::<u8>().ok()?;
            let b = parts[2].trim().parse::<u8>().ok()?;
            let a = parts[3].trim().parse::<f32>().ok()?;
            return Some([r, g, b, (a * 255.0) as u8]);
        }
    }
    None
}

// ─── DomBridge ────────────────────────────────────────────────────────────

/// Stores the bridge between JS element references and DOM NodeIds.
pub struct DomBridge {
    /// The DOM document
    pub document: Document,
    /// Map from JS object ID to DOM node ID
    pub obj_to_node: HashMap<usize, NodeId>,
    /// Map from DOM node ID to JS object ID
    pub node_to_obj: HashMap<NodeId, usize>,
    /// Event listeners: (node_id, event_name) -> list of handler object IDs
    pub event_listeners: HashMap<(NodeId, String), Vec<usize>>,
    /// Canvas states indexed by DOM node ID
    pub canvas_states: HashMap<NodeId, CanvasState>,
}

impl DomBridge {
    pub fn new(document: Document) -> Self {
        DomBridge {
            document,
            obj_to_node: HashMap::new(),
            node_to_obj: HashMap::new(),
            event_listeners: HashMap::new(),
            canvas_states: HashMap::new(),
        }
    }

    /// Wrap a DOM node as a JS object in the VM, returning the JsValue.
    pub fn wrap_node(&mut self, vm: &mut Vm, node_id: NodeId) -> JsValue {
        if let Some(&obj_id) = self.node_to_obj.get(&node_id) {
            return JsValue::Object(obj_id);
        }

        let obj_id = vm.heap.len();
        vm.heap.push(JsObject {
            properties: HashMap::new(),
            prototype: None,
            marked: false,
        });

        // Set up properties based on node type
        let node = &self.document.nodes[node_id];
        match &node.data {
            NodeData::Element(el) => {
                let tag_upper = el.tag_name.to_uppercase();
                vm.heap[obj_id].properties.insert(
                    "tagName".into(),
                    JsValue::Str(tag_upper.clone()),
                );
                vm.heap[obj_id].properties.insert(
                    "nodeName".into(),
                    JsValue::Str(tag_upper),
                );
                vm.heap[obj_id].properties.insert(
                    "nodeType".into(),
                    JsValue::Number(1.0),
                );
                if let Some(id) = el.attributes.get("id") {
                    vm.heap[obj_id].properties.insert(
                        "id".into(),
                        JsValue::Str(id.clone()),
                    );
                }
                if let Some(class) = el.attributes.get("class") {
                    vm.heap[obj_id].properties.insert(
                        "className".into(),
                        JsValue::Str(class.clone()),
                    );
                }

                // Canvas elements get getContext method
                if el.tag_name == "canvas" {
                    vm.heap[obj_id].properties.insert(
                        "getContext".into(),
                        JsValue::NativeFunction(native_get_context),
                    );
                    // Read canvas dimensions from HTML attributes
                    let w: u32 = el.attributes.get("width")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(300);
                    let h: u32 = el.attributes.get("height")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(150);
                    vm.heap[obj_id].properties.insert("width".into(), JsValue::Number(w as f64));
                    vm.heap[obj_id].properties.insert("height".into(), JsValue::Number(h as f64));
                    // Create canvas state
                    self.canvas_states.insert(node_id, CanvasState::new(w, h));
                }
            }
            NodeData::Text(t) => {
                vm.heap[obj_id].properties.insert(
                    "nodeType".into(),
                    JsValue::Number(3.0),
                );
                vm.heap[obj_id].properties.insert(
                    "textContent".into(),
                    JsValue::Str(t.content.clone()),
                );
            }
            NodeData::Document => {
                vm.heap[obj_id].properties.insert(
                    "nodeType".into(),
                    JsValue::Number(9.0),
                );
            }
            NodeData::Comment(c) => {
                vm.heap[obj_id].properties.insert(
                    "nodeType".into(),
                    JsValue::Number(8.0),
                );
                vm.heap[obj_id].properties.insert(
                    "textContent".into(),
                    JsValue::Str(c.clone()),
                );
            }
        }

        // Store internal __node_id__ for lookups
        vm.heap[obj_id].properties.insert(
            "__node_id__".into(),
            JsValue::Number(node_id as f64),
        );

        self.obj_to_node.insert(obj_id, node_id);
        self.node_to_obj.insert(node_id, obj_id);

        JsValue::Object(obj_id)
    }

    /// Get the node ID from a JS element value.
    pub fn get_node_id(&self, vm: &Vm, val: &JsValue) -> Option<NodeId> {
        if let JsValue::Object(obj_id) = val {
            if let Some(JsValue::Number(n)) = vm.heap.get(*obj_id)
                .and_then(|o| o.properties.get("__node_id__"))
            {
                return Some(*n as usize);
            }
            self.obj_to_node.get(obj_id).copied()
        } else {
            None
        }
    }

    /// Collect text content from a DOM subtree.
    pub fn get_text_content(&self, node_id: NodeId) -> String {
        let mut result = String::new();
        self.collect_text(node_id, &mut result);
        result
    }

    fn collect_text(&self, node_id: NodeId, out: &mut String) {
        let node = &self.document.nodes[node_id];
        match &node.data {
            NodeData::Text(t) => out.push_str(&t.content),
            _ => {
                for &child in &node.children {
                    self.collect_text(child, out);
                }
            }
        }
    }

    /// Set text content of a node (removes all children, adds text node).
    pub fn set_text_content(&mut self, node_id: NodeId, text: &str) {
        self.document.nodes[node_id].children.clear();
        self.document.add_node(node_id, NodeData::Text(TextData {
            content: text.to_string(),
        }));
    }

    /// Set innerHTML (simplified: just sets text content for now).
    pub fn set_inner_html(&mut self, node_id: NodeId, html: &str) {
        self.document.nodes[node_id].children.clear();
        self.document.add_node(node_id, NodeData::Text(TextData {
            content: html.to_string(),
        }));
    }

    /// Get innerHTML (simplified).
    pub fn get_inner_html(&self, node_id: NodeId) -> String {
        self.get_text_content(node_id)
    }

    /// Set an attribute on an element.
    pub fn set_attribute(&mut self, node_id: NodeId, name: &str, value: &str) {
        if let NodeData::Element(ref mut el) = self.document.nodes[node_id].data {
            el.attributes.insert(name.to_string(), value.to_string());
        }
    }

    /// Get an attribute from an element.
    pub fn get_attribute(&self, node_id: NodeId, name: &str) -> Option<String> {
        if let NodeData::Element(ref el) = self.document.nodes[node_id].data {
            el.attributes.get(name).cloned()
        } else {
            None
        }
    }

    /// Create a new element and add to DOM.
    pub fn create_element(&mut self, tag: &str) -> NodeId {
        let id = self.document.nodes.len();
        self.document.nodes.push(Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Element(ElementData::new(tag)),
        });
        id
    }

    /// Append a child node to a parent.
    pub fn append_child(&mut self, parent_id: NodeId, child_id: NodeId) {
        self.document.nodes[child_id].parent = Some(parent_id);
        self.document.nodes[parent_id].children.push(child_id);
    }
}

// ─── DOM bindings installation ────────────────────────────────────────────

/// Install DOM globals (`document`) into the VM.
pub fn install_dom_bindings(vm: &mut Vm, bridge: Arc<Mutex<DomBridge>>) {
    let doc_obj_id = vm.heap.len();
    vm.heap.push(JsObject {
        properties: HashMap::new(),
        prototype: None,
        marked: false,
    });

    vm.globals.insert("__dom_doc_id__".into(), JsValue::Number(doc_obj_id as f64));

    vm.heap[doc_obj_id].properties.insert(
        "getElementById".into(),
        JsValue::NativeFunction(native_get_element_by_id),
    );
    vm.heap[doc_obj_id].properties.insert(
        "createElement".into(),
        JsValue::NativeFunction(native_create_element),
    );
    vm.heap[doc_obj_id].properties.insert(
        "body".into(),
        JsValue::Null,
    );

    vm.globals.insert("document".into(), JsValue::Object(doc_obj_id));

    BRIDGE.with(|b| {
        *b.borrow_mut() = Some(bridge);
    });
}

thread_local! {
    static BRIDGE: std::cell::RefCell<Option<Arc<Mutex<DomBridge>>>> = std::cell::RefCell::new(None);
}

fn with_bridge<F, R>(f: F) -> R
where
    F: FnOnce(&mut DomBridge) -> R,
{
    BRIDGE.with(|b| {
        let borrow = b.borrow();
        let bridge_arc = borrow.as_ref().expect("DOM bridge not installed");
        let mut bridge = bridge_arc.lock().unwrap();
        f(&mut bridge)
    })
}

/// Get the current bridge Arc (for extracting canvas data from the shell).
pub fn get_bridge() -> Option<Arc<Mutex<DomBridge>>> {
    BRIDGE.with(|b| {
        b.borrow().clone()
    })
}

// ─── DOM native functions ─────────────────────────────────────────────────

fn native_get_element_by_id(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let id_str = match args.first() {
        Some(JsValue::Str(s)) => s.clone(),
        _ => return JsValue::Null,
    };

    let node_id = with_bridge(|bridge| {
        bridge.document.get_element_by_id(&id_str)
    });

    match node_id {
        Some(nid) => {
            with_bridge(|bridge| bridge.wrap_node(vm, nid))
        }
        None => JsValue::Null,
    }
}

fn native_create_element(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let tag = match args.first() {
        Some(JsValue::Str(s)) => s.clone(),
        _ => return JsValue::Null,
    };

    with_bridge(|bridge| {
        let node_id = bridge.create_element(&tag);
        bridge.wrap_node(vm, node_id)
    })
}

// ─── Canvas 2D native functions ───────────────────────────────────────────

/// Helper: extract __canvas_id__ from the context object via vm.this_value.
fn get_canvas_id_from_this(vm: &Vm) -> Option<NodeId> {
    if let JsValue::Object(obj_id) = &vm.this_value {
        if let Some(JsValue::Number(n)) = vm.heap.get(*obj_id)
            .and_then(|o| o.properties.get("__canvas_id__"))
        {
            return Some(*n as usize);
        }
    }
    None
}

/// canvas.getContext("2d") — returns a context object with drawing methods.
fn native_get_context(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    // Verify arg is "2d"
    match args.first() {
        Some(JsValue::Str(s)) if s == "2d" => {}
        _ => return JsValue::Null,
    }

    // Get the canvas element's node_id from this_value (set by VM's GetProp)
    let canvas_node_id = if let JsValue::Object(obj_id) = &vm.this_value {
        vm.heap.get(*obj_id)
            .and_then(|o| o.properties.get("__node_id__"))
            .and_then(|v| if let JsValue::Number(n) = v { Some(*n as usize) } else { None })
    } else {
        None
    };

    let canvas_node_id = match canvas_node_id {
        Some(id) => id,
        None => return JsValue::Null,
    };

    // Create the 2D context object
    let ctx_id = vm.heap.len();
    vm.heap.push(JsObject {
        properties: HashMap::new(),
        prototype: None,
        marked: false,
    });

    // Store canvas ID so methods can find the right canvas
    vm.heap[ctx_id].properties.insert("__canvas_id__".into(), JsValue::Number(canvas_node_id as f64));

    // Drawing methods
    vm.heap[ctx_id].properties.insert("fillRect".into(), JsValue::NativeFunction(native_ctx_fill_rect));
    vm.heap[ctx_id].properties.insert("strokeRect".into(), JsValue::NativeFunction(native_ctx_stroke_rect));
    vm.heap[ctx_id].properties.insert("clearRect".into(), JsValue::NativeFunction(native_ctx_clear_rect));
    vm.heap[ctx_id].properties.insert("fillText".into(), JsValue::NativeFunction(native_ctx_fill_text));
    vm.heap[ctx_id].properties.insert("beginPath".into(), JsValue::NativeFunction(native_ctx_begin_path));
    vm.heap[ctx_id].properties.insert("moveTo".into(), JsValue::NativeFunction(native_ctx_move_to));
    vm.heap[ctx_id].properties.insert("lineTo".into(), JsValue::NativeFunction(native_ctx_line_to));
    vm.heap[ctx_id].properties.insert("arc".into(), JsValue::NativeFunction(native_ctx_arc));
    vm.heap[ctx_id].properties.insert("closePath".into(), JsValue::NativeFunction(native_ctx_close_path));
    vm.heap[ctx_id].properties.insert("fill".into(), JsValue::NativeFunction(native_ctx_fill));
    vm.heap[ctx_id].properties.insert("stroke".into(), JsValue::NativeFunction(native_ctx_stroke));

    // Style properties (initial values)
    vm.heap[ctx_id].properties.insert("fillStyle".into(), JsValue::Str("#000000".into()));
    vm.heap[ctx_id].properties.insert("strokeStyle".into(), JsValue::Str("#000000".into()));
    vm.heap[ctx_id].properties.insert("lineWidth".into(), JsValue::Number(1.0));

    JsValue::Object(ctx_id)
}

fn native_ctx_fill_rect(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    let x = args.get(0).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let y = args.get(1).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let w = args.get(2).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let h = args.get(3).map(|v| v.to_number() as f32).unwrap_or(0.0);

    // Read fillStyle from context object
    sync_ctx_style_to_canvas(vm, canvas_id);

    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.fill_rect(x, y, w, h);
        }
    });
    JsValue::Undefined
}

fn native_ctx_stroke_rect(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    let x = args.get(0).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let y = args.get(1).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let w = args.get(2).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let h = args.get(3).map(|v| v.to_number() as f32).unwrap_or(0.0);

    sync_ctx_style_to_canvas(vm, canvas_id);

    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.stroke_rect(x, y, w, h);
        }
    });
    JsValue::Undefined
}

fn native_ctx_clear_rect(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    let x = args.get(0).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let y = args.get(1).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let w = args.get(2).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let h = args.get(3).map(|v| v.to_number() as f32).unwrap_or(0.0);

    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.clear_rect(x, y, w, h);
        }
    });
    JsValue::Undefined
}

fn native_ctx_fill_text(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    let text = args.get(0).map(|v| v.to_string_val()).unwrap_or_default();
    let x = args.get(1).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let y = args.get(2).map(|v| v.to_number() as f32).unwrap_or(0.0);

    sync_ctx_style_to_canvas(vm, canvas_id);

    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.fill_text(&text, x, y);
        }
    });
    JsValue::Undefined
}

fn native_ctx_begin_path(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.path.clear();
            canvas.path_x = 0.0;
            canvas.path_y = 0.0;
        }
    });
    JsValue::Undefined
}

fn native_ctx_move_to(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    let x = args.get(0).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let y = args.get(1).map(|v| v.to_number() as f32).unwrap_or(0.0);
    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.path.push(PathCmd::MoveTo(x, y));
            canvas.path_x = x;
            canvas.path_y = y;
        }
    });
    JsValue::Undefined
}

fn native_ctx_line_to(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    let x = args.get(0).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let y = args.get(1).map(|v| v.to_number() as f32).unwrap_or(0.0);
    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.path.push(PathCmd::LineTo(x, y));
            canvas.path_x = x;
            canvas.path_y = y;
        }
    });
    JsValue::Undefined
}

fn native_ctx_arc(vm: &mut Vm, args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    let cx = args.get(0).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let cy = args.get(1).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let r = args.get(2).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let start = args.get(3).map(|v| v.to_number() as f32).unwrap_or(0.0);
    let end = args.get(4).map(|v| v.to_number() as f32).unwrap_or(std::f32::consts::TAU);
    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.path.push(PathCmd::Arc(cx, cy, r, start, end));
        }
    });
    JsValue::Undefined
}

fn native_ctx_close_path(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.path.push(PathCmd::ClosePath);
        }
    });
    JsValue::Undefined
}

fn native_ctx_fill(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    sync_ctx_style_to_canvas(vm, canvas_id);
    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.fill_path();
        }
    });
    JsValue::Undefined
}

fn native_ctx_stroke(vm: &mut Vm, _args: Vec<JsValue>) -> JsValue {
    let canvas_id = match get_canvas_id_from_this(vm) { Some(id) => id, None => return JsValue::Undefined };
    sync_ctx_style_to_canvas(vm, canvas_id);
    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.stroke_path();
        }
    });
    JsValue::Undefined
}

/// Sync fillStyle/strokeStyle/lineWidth from the JS context object to the CanvasState.
fn sync_ctx_style_to_canvas(vm: &Vm, canvas_id: NodeId) {
    let (fill_color, stroke_color, line_width) = if let JsValue::Object(obj_id) = &vm.this_value {
        if let Some(obj) = vm.heap.get(*obj_id) {
            let fc = obj.properties.get("fillStyle")
                .and_then(|v| if let JsValue::Str(s) = v { parse_css_color(s) } else { None })
                .unwrap_or([0, 0, 0, 255]);
            let sc = obj.properties.get("strokeStyle")
                .and_then(|v| if let JsValue::Str(s) = v { parse_css_color(s) } else { None })
                .unwrap_or([0, 0, 0, 255]);
            let lw = obj.properties.get("lineWidth")
                .map(|v| v.to_number() as f32)
                .unwrap_or(1.0);
            (fc, sc, lw)
        } else {
            ([0, 0, 0, 255], [0, 0, 0, 255], 1.0)
        }
    } else {
        ([0, 0, 0, 255], [0, 0, 0, 255], 1.0)
    };

    with_bridge(|bridge| {
        if let Some(canvas) = bridge.canvas_states.get_mut(&canvas_id) {
            canvas.fill_color = fill_color;
            canvas.stroke_color = stroke_color;
            canvas.line_width = line_width;
        }
    });
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_doc() -> Document {
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut div_data = ElementData::new("div");
        div_data.attributes.insert("id".into(), "test".into());
        let div = doc.add_node(body, NodeData::Element(div_data));

        doc.add_node(div, NodeData::Text(TextData { content: "Hello World".into() }));
        doc
    }

    #[test]
    fn test_bridge_wrap_node() {
        let doc = make_test_doc();
        let mut vm = Vm::new();
        let mut bridge = DomBridge::new(doc);

        let body_id = bridge.document.body().unwrap();
        let val = bridge.wrap_node(&mut vm, body_id);
        assert!(matches!(val, JsValue::Object(_)));

        let val2 = bridge.wrap_node(&mut vm, body_id);
        if let (JsValue::Object(a), JsValue::Object(b)) = (&val, &val2) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_bridge_get_text_content() {
        let doc = make_test_doc();
        let bridge = DomBridge::new(doc);
        let div_id = bridge.document.get_element_by_id("test").unwrap();
        assert_eq!(bridge.get_text_content(div_id), "Hello World");
    }

    #[test]
    fn test_bridge_set_text_content() {
        let doc = make_test_doc();
        let mut bridge = DomBridge::new(doc);
        let div_id = bridge.document.get_element_by_id("test").unwrap();
        bridge.set_text_content(div_id, "New Text");
        assert_eq!(bridge.get_text_content(div_id), "New Text");
    }

    #[test]
    fn test_bridge_set_attribute() {
        let doc = make_test_doc();
        let mut bridge = DomBridge::new(doc);
        let div_id = bridge.document.get_element_by_id("test").unwrap();
        bridge.set_attribute(div_id, "class", "myclass");
        assert_eq!(bridge.get_attribute(div_id, "class"), Some("myclass".into()));
    }

    #[test]
    fn test_bridge_create_element() {
        let doc = make_test_doc();
        let mut bridge = DomBridge::new(doc);
        let new_id = bridge.create_element("span");
        assert!(new_id > 0);
        if let NodeData::Element(el) = &bridge.document.nodes[new_id].data {
            assert_eq!(el.tag_name, "span");
        }
    }

    #[test]
    fn test_bridge_append_child() {
        let doc = make_test_doc();
        let mut bridge = DomBridge::new(doc);
        let body_id = bridge.document.body().unwrap();
        let new_id = bridge.create_element("p");
        bridge.append_child(body_id, new_id);
        assert!(bridge.document.nodes[body_id].children.contains(&new_id));
    }

    #[test]
    fn test_get_element_by_id_via_js() {
        let doc = make_test_doc();
        let bridge = Arc::new(Mutex::new(DomBridge::new(doc)));
        let mut vm = Vm::new();
        install_dom_bindings(&mut vm, bridge.clone());

        let result = vm.eval("var el = document.getElementById('test');");
        assert!(result.is_ok());

        let el = vm.globals.get("el").cloned().unwrap_or(JsValue::Undefined);
        assert!(matches!(el, JsValue::Object(_)));
    }

    #[test]
    fn test_canvas_state() {
        let mut canvas = CanvasState::new(100, 100);
        canvas.fill_color = [255, 0, 0, 255];
        canvas.fill_rect(10.0, 10.0, 20.0, 20.0);

        // Check that pixel at (15, 15) is red
        let idx = (15 * 100 + 15) * 4;
        assert_eq!(canvas.pixels[idx as usize], 255);
        assert_eq!(canvas.pixels[idx as usize + 1], 0);
        assert_eq!(canvas.pixels[idx as usize + 2], 0);
        assert_eq!(canvas.pixels[idx as usize + 3], 255);
    }

    #[test]
    fn test_parse_css_color() {
        assert_eq!(parse_css_color("red"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("#ff0000"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(parse_css_color("rgb(0, 128, 255)"), Some([0, 128, 255, 255]));
        assert_eq!(parse_css_color("blue"), Some([0, 0, 255, 255]));
    }

    #[test]
    fn test_canvas_stroke_rect() {
        let mut canvas = CanvasState::new(100, 100);
        canvas.stroke_color = [0, 255, 0, 255];
        canvas.stroke_rect(10.0, 10.0, 30.0, 30.0);
        // Top-left corner should be green
        let idx = (10 * 100 + 10) * 4;
        assert_eq!(canvas.pixels[idx as usize + 1], 255); // green
    }

    #[test]
    fn test_canvas_clear_rect() {
        let mut canvas = CanvasState::new(100, 100);
        canvas.fill_color = [255, 0, 0, 255];
        canvas.fill_rect(0.0, 0.0, 100.0, 100.0);
        canvas.clear_rect(10.0, 10.0, 20.0, 20.0);
        // Pixel in cleared area should be transparent
        let idx = (15 * 100 + 15) * 4;
        assert_eq!(canvas.pixels[idx as usize + 3], 0);
    }
}
