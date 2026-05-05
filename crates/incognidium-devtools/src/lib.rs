use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use incognidium_dom::{Document, NodeData, NodeId};
use incognidium_layout::{BoxType, FlatBox, LayoutBox};
use incognidium_style::{ComputedStyle, StyleMap};

// ── Public types ──────────────────────────────────────────────

/// A logged network request.
pub struct NetworkEntry {
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub content_type: String,
    pub size: usize,
    pub error: Option<String>,
}

/// A link found on the page.
pub struct LinkInfo {
    pub href: String,
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Command from MCP → browser.
pub enum DevToolsCommand {
    Navigate(String),
    Back,
    Forward,
    Reload,
    Scroll(f32),
    ExecuteJs(String),
    Click { x: f32, y: f32 },
}

// ── Bridge (shared state) ─────────────────────────────────────

struct BridgeInner {
    // Page state (written by browser, read by MCP)
    current_url: String,
    page_title: String,
    html_content: String,
    page_text: String,
    console_lines: Vec<String>,
    network_log: Vec<NetworkEntry>,
    screenshot_png: Option<Vec<u8>>,
    links: Vec<LinkInfo>,
    dom_json: String,
    layout_json: String,
    styles_json: String,
    scroll_y: f32,
    page_height: f32,
    viewport_size: (u32, u32),
    can_go_back: bool,
    can_go_forward: bool,

    // Command queue (written by MCP, consumed by browser)
    pending_command: Option<DevToolsCommand>,

    // Response (written by browser after command, read by MCP)
    command_result: Option<String>,
    js_result: Option<String>,
}

/// Shared bridge between the browser event loop and the MCP server thread.
pub struct DevToolsBridge {
    inner: Mutex<BridgeInner>,
    command_done: Condvar,
    wake_browser: Box<dyn Fn() + Send + Sync>,
}

impl DevToolsBridge {
    pub fn new(wake_fn: Box<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(BridgeInner {
                current_url: String::new(),
                page_title: String::new(),
                html_content: String::new(),
                page_text: String::new(),
                console_lines: Vec::new(),
                network_log: Vec::new(),
                screenshot_png: None,
                links: Vec::new(),
                dom_json: String::from("null"),
                layout_json: String::from("null"),
                styles_json: String::from("null"),
                scroll_y: 0.0,
                page_height: 0.0,
                viewport_size: (1024, 768),
                can_go_back: false,
                can_go_forward: false,
                pending_command: None,
                command_result: None,
                js_result: None,
            }),
            command_done: Condvar::new(),
            wake_browser: wake_fn,
        })
    }

    // ── Called by browser ──────────────────────────────────────

    /// Snapshot page state after render. Called by the browser on each render.
    pub fn update_page_state(
        &self,
        url: &str,
        title: &str,
        html: &str,
        page_text: &str,
        console: &[String],
        links: Vec<LinkInfo>,
        scroll_y: f32,
        page_height: f32,
        viewport: (u32, u32),
        can_back: bool,
        can_fwd: bool,
    ) {
        let mut s = self.inner.lock().unwrap();
        s.current_url = url.to_string();
        s.page_title = title.to_string();
        s.html_content = html.to_string();
        s.page_text = page_text.to_string();
        s.console_lines = console.to_vec();
        s.links = links;
        s.scroll_y = scroll_y;
        s.page_height = page_height;
        s.viewport_size = viewport;
        s.can_go_back = can_back;
        s.can_go_forward = can_fwd;
    }

    /// Update DOM snapshot (serialized JSON).
    pub fn update_dom(&self, doc: &Document) {
        let json = serialize_dom(doc);
        let mut s = self.inner.lock().unwrap();
        s.dom_json = json;
    }

    /// Update layout snapshot (serialized JSON).
    pub fn update_layout(&self, layout_root: &LayoutBox) {
        let json = serialize_layout(layout_root);
        let mut s = self.inner.lock().unwrap();
        s.layout_json = json;
    }

    /// Update computed styles snapshot.
    pub fn update_styles(&self, doc: &Document, styles: &StyleMap) {
        let json = serialize_styles(doc, styles);
        let mut s = self.inner.lock().unwrap();
        s.styles_json = json;
    }

    /// Update screenshot PNG bytes.
    pub fn update_screenshot(&self, png_data: Vec<u8>) {
        let mut s = self.inner.lock().unwrap();
        s.screenshot_png = Some(png_data);
    }

    /// Log a network request.
    pub fn log_network(&self, entry: NetworkEntry) {
        let mut s = self.inner.lock().unwrap();
        s.network_log.push(entry);
    }

    /// Check for a pending command from MCP. Non-blocking.
    pub fn take_pending_command(&self) -> Option<DevToolsCommand> {
        let mut s = self.inner.lock().unwrap();
        s.pending_command.take()
    }

    /// Signal that a command has been processed.
    pub fn complete_command(&self, result: String, js_result: Option<String>) {
        let mut s = self.inner.lock().unwrap();
        s.command_result = Some(result);
        s.js_result = js_result;
        self.command_done.notify_all();
    }

    // ── Called by MCP server ──────────────────────────────────

    fn send_command_and_wait(&self, cmd: DevToolsCommand, timeout_secs: u64) -> (String, Option<String>) {
        let mut s = self.inner.lock().unwrap();
        s.pending_command = Some(cmd);
        s.command_result = None;
        s.js_result = None;
        drop(s);

        // Wake the browser event loop
        (self.wake_browser)();

        // Wait for browser to process
        let mut s = self.inner.lock().unwrap();
        let timeout = Duration::from_secs(timeout_secs);
        while s.command_result.is_none() {
            let (guard, wait_result) = self.command_done.wait_timeout(s, timeout).unwrap();
            s = guard;
            if wait_result.timed_out() {
                return ("Timed out waiting for browser".to_string(), None);
            }
        }
        let result = s.command_result.take().unwrap_or_default();
        let js_result = s.js_result.take();
        (result, js_result)
    }
}

// ── MCP Server ────────────────────────────────────────────────

/// Run the MCP server on stdio. Blocks forever (call from a spawned thread).
pub fn run_mcp_server(bridge: Arc<DevToolsBridge>) {
    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("Parse error: {e}") }
                });
                write_response(&stdout, &err_resp);
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        // Notifications (no id) — just acknowledge
        if id.is_null() && method == "notifications/initialized" {
            continue;
        }
        if id.is_null() {
            // Other notifications — ignore
            continue;
        }

        let response = match method {
            "initialize" => handle_initialize(&id),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => handle_tools_list(&id),
            "tools/call" => handle_tools_call(&id, &params, &bridge),
            "resources/list" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "resources": [] }
            }),
            _ => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("Unknown method: {method}") }
            }),
        };

        write_response(&stdout, &response);
    }
}

fn write_response(stdout: &io::Stdout, response: &Value) {
    let msg = serde_json::to_string(response).unwrap();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

fn handle_initialize(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "incognidium-devtools",
                "version": "0.1.0"
            }
        }
    })
}

fn handle_tools_list(id: &Value) -> Value {
    let tools = vec![
        tool_def("navigate", "Navigate to a URL", json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "URL to navigate to" }
            },
            "required": ["url"]
        })),
        tool_def("get_url", "Get the current page URL", json!({ "type": "object", "properties": {} })),
        tool_def("get_page_info", "Get page summary: URL, title, scroll, viewport size, history state", json!({ "type": "object", "properties": {} })),
        tool_def("get_page_source", "Get the raw HTML source of the current page", json!({ "type": "object", "properties": {} })),
        tool_def("get_page_text", "Get visible text content extracted from the rendered page", json!({ "type": "object", "properties": {} })),
        tool_def("get_dom_tree", "Get the DOM tree as a JSON structure", json!({ "type": "object", "properties": {} })),
        tool_def("get_links", "Get all links on the page with text, href, and position", json!({ "type": "object", "properties": {} })),
        tool_def("get_computed_styles", "Get computed CSS styles for a DOM node by node ID", json!({
            "type": "object",
            "properties": {
                "node_id": { "type": "integer", "description": "DOM node ID" }
            },
            "required": ["node_id"]
        })),
        tool_def("get_layout_tree", "Get the layout/box tree with positions and dimensions", json!({ "type": "object", "properties": {} })),
        tool_def("screenshot", "Capture a screenshot of the current page as PNG", json!({ "type": "object", "properties": {} })),
        tool_def("get_console", "Get JavaScript console output", json!({ "type": "object", "properties": {} })),
        tool_def("get_network_log", "Get the log of all network requests made", json!({ "type": "object", "properties": {} })),
        tool_def("execute_js", "Execute JavaScript in the page context and return the result", json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "JavaScript code to execute" }
            },
            "required": ["code"]
        })),
        tool_def("scroll", "Scroll the page to a vertical position in pixels", json!({
            "type": "object",
            "properties": {
                "y": { "type": "number", "description": "Vertical scroll position in pixels" }
            },
            "required": ["y"]
        })),
        tool_def("click", "Click at page coordinates (relative to page content, not viewport)", json!({
            "type": "object",
            "properties": {
                "x": { "type": "number", "description": "X coordinate" },
                "y": { "type": "number", "description": "Y coordinate" }
            },
            "required": ["x", "y"]
        })),
        tool_def("back", "Navigate back in browser history", json!({ "type": "object", "properties": {} })),
        tool_def("forward", "Navigate forward in browser history", json!({ "type": "object", "properties": {} })),
        tool_def("reload", "Reload the current page", json!({ "type": "object", "properties": {} })),
    ];

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "tools": tools }
    })
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn handle_tools_call(id: &Value, params: &Value, bridge: &Arc<DevToolsBridge>) -> Value {
    let tool_name = params
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(json!({}));

    let content = match tool_name {
        "navigate" => {
            let url = args.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if url.is_empty() {
                text_content("Error: url is required")
            } else {
                let (result, _) = bridge.send_command_and_wait(
                    DevToolsCommand::Navigate(url.to_string()),
                    30,
                );
                text_content(&result)
            }
        }
        "get_url" => {
            let s = bridge.inner.lock().unwrap();
            text_content(&s.current_url)
        }
        "get_page_info" => {
            let s = bridge.inner.lock().unwrap();
            let info = json!({
                "url": s.current_url,
                "title": s.page_title,
                "scroll_y": s.scroll_y,
                "page_height": s.page_height,
                "viewport_width": s.viewport_size.0,
                "viewport_height": s.viewport_size.1,
                "can_go_back": s.can_go_back,
                "can_go_forward": s.can_go_forward,
                "network_requests": s.network_log.len(),
                "links_count": s.links.len(),
            });
            text_content(&serde_json::to_string_pretty(&info).unwrap())
        }
        "get_page_source" => {
            let s = bridge.inner.lock().unwrap();
            text_content(&s.html_content)
        }
        "get_page_text" => {
            let s = bridge.inner.lock().unwrap();
            text_content(&s.page_text)
        }
        "get_dom_tree" => {
            let s = bridge.inner.lock().unwrap();
            text_content(&s.dom_json)
        }
        "get_links" => {
            let s = bridge.inner.lock().unwrap();
            let links: Vec<Value> = s.links.iter().map(|l| {
                json!({
                    "href": l.href,
                    "text": l.text,
                    "x": l.x, "y": l.y,
                    "width": l.width, "height": l.height,
                })
            }).collect();
            text_content(&serde_json::to_string_pretty(&links).unwrap())
        }
        "get_computed_styles" => {
            let s = bridge.inner.lock().unwrap();
            // styles_json is the full map; for a specific node, parse and extract
            if let Some(node_id) = args.get("node_id").and_then(|n| n.as_u64()) {
                let all: Value = serde_json::from_str(&s.styles_json).unwrap_or(json!({}));
                let key = node_id.to_string();
                if let Some(node_style) = all.get(&key) {
                    text_content(&serde_json::to_string_pretty(node_style).unwrap())
                } else {
                    text_content(&format!("No style found for node {node_id}"))
                }
            } else {
                text_content("Error: node_id is required")
            }
        }
        "get_layout_tree" => {
            let s = bridge.inner.lock().unwrap();
            text_content(&s.layout_json)
        }
        "screenshot" => {
            let s = bridge.inner.lock().unwrap();
            if let Some(ref png) = s.screenshot_png {
                let b64 = base64::engine::general_purpose::STANDARD.encode(png);
                json!([{ "type": "image", "data": b64, "mimeType": "image/png" }])
            } else {
                text_content("No screenshot available yet")
            }
        }
        "get_console" => {
            let s = bridge.inner.lock().unwrap();
            let output = s.console_lines.join("\n");
            text_content(if output.is_empty() { "(no console output)" } else { &output })
        }
        "get_network_log" => {
            let s = bridge.inner.lock().unwrap();
            let entries: Vec<Value> = s.network_log.iter().map(|e| {
                json!({
                    "method": e.method,
                    "url": e.url,
                    "status": e.status,
                    "content_type": e.content_type,
                    "size": e.size,
                    "error": e.error,
                })
            }).collect();
            text_content(&serde_json::to_string_pretty(&entries).unwrap())
        }
        "execute_js" => {
            let code = args.get("code").and_then(|c| c.as_str()).unwrap_or("");
            if code.is_empty() {
                text_content("Error: code is required")
            } else {
                let (result, js_val) = bridge.send_command_and_wait(
                    DevToolsCommand::ExecuteJs(code.to_string()),
                    10,
                );
                let output = if let Some(val) = js_val {
                    format!("Result: {val}\n\nConsole:\n{result}")
                } else {
                    result
                };
                text_content(&output)
            }
        }
        "scroll" => {
            let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let (result, _) = bridge.send_command_and_wait(
                DevToolsCommand::Scroll(y as f32),
                5,
            );
            text_content(&result)
        }
        "click" => {
            let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let (result, _) = bridge.send_command_and_wait(
                DevToolsCommand::Click { x: x as f32, y: y as f32 },
                10,
            );
            text_content(&result)
        }
        "back" => {
            let (result, _) = bridge.send_command_and_wait(DevToolsCommand::Back, 10);
            text_content(&result)
        }
        "forward" => {
            let (result, _) = bridge.send_command_and_wait(DevToolsCommand::Forward, 10);
            text_content(&result)
        }
        "reload" => {
            let (result, _) = bridge.send_command_and_wait(DevToolsCommand::Reload, 30);
            text_content(&result)
        }
        _ => {
            return json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32602, "message": format!("Unknown tool: {tool_name}") }
            });
        }
    };

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "content": content }
    })
}

fn text_content(text: &str) -> Value {
    json!([{ "type": "text", "text": text }])
}

// ── Serialization helpers ─────────────────────────────────────

fn serialize_dom(doc: &Document) -> String {
    fn ser_node(doc: &Document, nid: NodeId) -> Value {
        let node = doc.node(nid);
        match &node.data {
            NodeData::Document => {
                let children: Vec<Value> = node.children.iter().map(|&c| ser_node(doc, c)).collect();
                json!({ "type": "document", "nodeId": nid, "children": children })
            }
            NodeData::Element(el) => {
                let children: Vec<Value> = node.children.iter().map(|&c| ser_node(doc, c)).collect();
                let mut obj = json!({
                    "type": "element",
                    "tag": el.tag_name,
                    "nodeId": nid,
                    "children": children,
                });
                if !el.attributes.is_empty() {
                    obj["attributes"] = json!(el.attributes);
                }
                obj
            }
            NodeData::Text(t) => {
                let trimmed = t.content.trim();
                if trimmed.is_empty() {
                    json!({ "type": "text", "nodeId": nid, "text": "" })
                } else {
                    json!({ "type": "text", "nodeId": nid, "text": t.content })
                }
            }
            NodeData::Comment(c) => {
                json!({ "type": "comment", "nodeId": nid, "text": c })
            }
        }
    }
    let tree = ser_node(doc, doc.root());
    serde_json::to_string(&tree).unwrap_or_else(|_| "null".into())
}

fn serialize_layout(layout_box: &LayoutBox) -> String {
    fn ser_box(b: &LayoutBox) -> Value {
        let bt = match b.box_type {
            BoxType::Block => "block",
            BoxType::InlineBlock => "inline-block",
            BoxType::Inline => "inline",
            BoxType::Flex => "flex",
            BoxType::Grid => "grid",
            BoxType::Text => "text",
            BoxType::Image => "image",
            BoxType::Contents => "contents",
            BoxType::None => "none",
        };
        let children: Vec<Value> = b.children.iter().map(|c| ser_box(c)).collect();
        let mut obj = json!({
            "nodeId": b.node_id,
            "boxType": bt,
            "x": b.x, "y": b.y,
            "width": b.width, "height": b.height,
        });
        if let Some(ref text) = b.text {
            obj["text"] = json!(text);
        }
        if let Some(ref src) = b.image_src {
            obj["imageSrc"] = json!(src);
        }
        if let Some(ref href) = b.link_href {
            obj["linkHref"] = json!(href);
        }
        if !children.is_empty() {
            obj["children"] = json!(children);
        }
        obj
    }
    let tree = ser_box(layout_box);
    serde_json::to_string(&tree).unwrap_or_else(|_| "null".into())
}

fn serialize_styles(doc: &Document, styles: &StyleMap) -> String {
    let mut map = serde_json::Map::new();
    for node in &doc.nodes {
        if let Some(style) = styles.get(&node.id) {
            map.insert(node.id.to_string(), serialize_one_style(style));
        }
    }
    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

fn serialize_one_style(s: &ComputedStyle) -> Value {
    json!({
        "display": format!("{:?}", s.display),
        "color": format!("{:?}", s.color),
        "background_color": format!("{:?}", s.background_color),
        "font_size": s.font_size,
        "font_weight": format!("{:?}", s.font_weight),
        "font_style": format!("{:?}", s.font_style),
        "text_decoration": format!("{:?}", s.text_decoration),
        "text_align": format!("{:?}", s.text_align),
        "line_height": s.line_height,
        "width": format!("{:?}", s.width),
        "height": format!("{:?}", s.height),
        "margin_top": s.margin_top,
        "margin_right": s.margin_right,
        "margin_bottom": s.margin_bottom,
        "margin_left": s.margin_left,
        "padding_top": s.padding_top,
        "padding_right": s.padding_right,
        "padding_bottom": s.padding_bottom,
        "padding_left": s.padding_left,
        "border_top_width": s.border_top_width,
        "border_right_width": s.border_right_width,
        "border_bottom_width": s.border_bottom_width,
        "border_left_width": s.border_left_width,
        "border_color": format!("{:?}", s.border_color),
        "visibility": format!("{:?}", s.visibility),
        "flex_direction": format!("{:?}", s.flex_direction),
        "flex_grow": s.flex_grow,
        "justify_content": format!("{:?}", s.justify_content),
        "align_items": format!("{:?}", s.align_items),
        "gap": s.gap,
    })
}

// ── Page text extraction ──────────────────────────────────────

/// Extract visible text from flat boxes.
pub fn extract_page_text(flat_boxes: &[FlatBox]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut last_y: f32 = -999.0;

    for fb in flat_boxes {
        if let Some(ref text) = fb.text {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            // New line if y changed significantly
            if (fb.y - last_y).abs() > 4.0 && !current_line.is_empty() {
                lines.push(std::mem::take(&mut current_line));
            }
            if !current_line.is_empty() {
                current_line.push(' ');
            }
            current_line.push_str(trimmed);
            last_y = fb.y;
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    lines.join("\n")
}

/// Extract links from flat boxes.
pub fn extract_links(flat_boxes: &[FlatBox]) -> Vec<LinkInfo> {
    let mut seen = HashMap::new();
    for fb in flat_boxes {
        if let Some(ref href) = fb.link_href {
            let text = fb.text.as_deref().unwrap_or("").trim().to_string();
            let key = href.clone();
            let entry = seen.entry(key).or_insert_with(|| LinkInfo {
                href: href.clone(),
                text: String::new(),
                x: fb.x,
                y: fb.y,
                width: fb.width,
                height: fb.height,
            });
            if !text.is_empty() {
                if !entry.text.is_empty() {
                    entry.text.push(' ');
                }
                entry.text.push_str(&text);
            }
        }
    }
    seen.into_values().collect()
}

/// Extract page title from the DOM.
pub fn extract_title(doc: &Document) -> String {
    for node in &doc.nodes {
        if let NodeData::Element(ref el) = node.data {
            if el.tag_name == "title" {
                for &child_id in &node.children {
                    if let NodeData::Text(ref t) = doc.nodes[child_id].data {
                        return t.content.trim().to_string();
                    }
                }
            }
        }
    }
    String::new()
}
