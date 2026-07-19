//! DOM bindings for the V8 JavaScript engine (via the `v8` crate).
//!
//! V8 is ~100x faster than Boa and can actually execute modern framework
//! bundles (React, Vue, etc.) in reasonable time.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Once};

use base64::Engine;
use incognidium_dom::*;
use incognidium_html::parse_html;

/// Shared DOM state accessible from native JS functions via thread-local.
pub struct DomState {
    pub document: Document,
}

type SharedDom = Arc<Mutex<DomState>>;

thread_local! {
    static DOM: RefCell<Option<SharedDom>> = const { RefCell::new(None) };
    static WRAPPER_CACHE: RefCell<HashMap<NodeId, v8::Global<v8::Object>>> = RefCell::new(HashMap::new());
    static DOCUMENT_OBJ: RefCell<Option<v8::Global<v8::Object>>> = const { RefCell::new(None) };
    static TIMEOUT_QUEUE: RefCell<Vec<TimeoutEntry>> = const { RefCell::new(Vec::new()) };
    /// Total callbacks executed during the current page execution.
    static TIMEOUT_COUNT: RefCell<usize> = const { RefCell::new(0) };
    /// Re-entrancy guard so a callback cannot recursively drain the queue.
    static DRAIN_GUARD: RefCell<bool> = const { RefCell::new(false) };
    /// Base URL of the current page, used to resolve dynamically inserted scripts.
    static BASE_URL: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Track URLs for dynamically loaded scripts so we don't execute the same file twice.
    static LOADED_SCRIPTS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Depth guard to prevent runaway recursion from scripts that insert scripts.
    static DYNAMIC_SCRIPT_DEPTH: RefCell<usize> = const { RefCell::new(0) };
    /// Window / document event listeners registered through addEventListener.
    /// Fired at lifecycle milestones (DOMContentLoaded / load) so pages that
    /// defer work until those events actually run their bootstrap code.
    static WINDOW_EVENT_LISTENERS: RefCell<
        HashMap<String, Vec<v8::Global<v8::Function>>>,
    > = RefCell::new(HashMap::new());
}

/// Deferred setTimeout/setInterval callback.
struct TimeoutEntry {
    func: v8::Global<v8::Function>,
    args: Vec<v8::Global<v8::Value>>,
}

fn clear_timeout_queue() {
    TIMEOUT_QUEUE.with(|q| q.borrow_mut().clear());
    TIMEOUT_COUNT.with(|c| *c.borrow_mut() = 0);
    DRAIN_GUARD.with(|g| *g.borrow_mut() = false);
}

fn clear_window_event_listeners() {
    WINDOW_EVENT_LISTENERS.with(|m| m.borrow_mut().clear());
}

fn add_window_event_listener(
    scope: &mut v8::HandleScope,
    event_type: &str,
    func: v8::Local<v8::Function>,
) {
    WINDOW_EVENT_LISTENERS.with(|m| {
        m.borrow_mut()
            .entry(event_type.to_string())
            .or_default()
            .push(v8::Global::new(scope, func))
    });
}

fn dispatch_window_event(scope: &mut v8::HandleScope, event_type: &str) {
    let listeners: Vec<v8::Global<v8::Function>> =
        WINDOW_EVENT_LISTENERS.with(|m| m.borrow().get(event_type).cloned().unwrap_or_default());
    if listeners.is_empty() {
        return;
    }
    let global = scope.get_current_context().global(scope);
    let event = v8::Object::new(scope);
    set_str(scope, event, "type", event_type);
    set_bool(scope, event, "bubbles", true);
    set_bool(scope, event, "cancelable", true);
    set_bool(scope, event, "composed", false);
    let target_key = v8_str(scope, "target");
    let _ = event.set(scope, target_key.into(), global.into());
    let current_target_key = v8_str(scope, "currentTarget");
    let _ = event.set(scope, current_target_key.into(), global.into());
    let default_prevented_key = v8_str(scope, "defaultPrevented");
    let false_val: v8::Local<v8::Value> = v8::Boolean::new(scope, false).into();
    let _ = event.set(scope, default_prevented_key.into(), false_val);
    let ev_val: v8::Local<v8::Value> = event.into();
    let undef = v8::undefined(scope).into();
    for listener in listeners {
        let func = v8::Local::new(scope, listener);
        let tc = &mut v8::TryCatch::new(scope);
        let _ = func.call(tc, undef, &[ev_val]);
    }
}

fn queue_timeout(
    scope: &mut v8::HandleScope,
    func: v8::Local<v8::Function>,
    args: Vec<v8::Local<v8::Value>>,
) {
    let global_func = v8::Global::new(scope, func);
    let global_args: Vec<_> = args
        .into_iter()
        .map(|v| v8::Global::new(scope, v))
        .collect();
    TIMEOUT_QUEUE.with(|q| {
        q.borrow_mut().push(TimeoutEntry {
            func: global_func,
            args: global_args,
        })
    });
}

/// Drain pending setTimeout callbacks. Snapshots the queue first so callbacks
/// enqueued during this drain are deferred to the next drain. A re-entrancy
/// guard and a per-page budget prevent infinite recursion from chained
/// setTimeout(0) loops on WordPress/React bundles.
fn drain_timeout_queue(scope: &mut v8::HandleScope, max: usize) {
    let already_draining = DRAIN_GUARD.with(|g| *g.borrow());
    if already_draining {
        return;
    }
    DRAIN_GUARD.with(|g| *g.borrow_mut() = true);

    let remaining_budget = TIMEOUT_COUNT.with(|c| {
        let count = *c.borrow();
        if count >= MAX_TIMEOUT_CALLBACKS {
            return 0usize;
        }
        (MAX_TIMEOUT_CALLBACKS - count).min(max)
    });

    if remaining_budget == 0 {
        // Drop any queued callbacks; we've hit the page budget.
        let dropped = TIMEOUT_QUEUE.with(|q| {
            let len = q.borrow().len();
            q.borrow_mut().clear();
            len
        });
        if dropped > 0 {
            eprintln!(
                "setTimeout budget exhausted: dropped {} queued callbacks (total {})",
                dropped, MAX_TIMEOUT_CALLBACKS
            );
        }
        DRAIN_GUARD.with(|g| *g.borrow_mut() = false);
        return;
    }

    // Snapshot current queue; leave newly-enqueued callbacks for the next drain.
    let snapshot = TIMEOUT_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
    let to_run = snapshot.len().min(remaining_budget);
    let dropped = snapshot.len().saturating_sub(to_run);
    if dropped > 0 {
        eprintln!(
            "setTimeout queue capped: {} callbacks dropped, {} processed",
            dropped, to_run
        );
    }

    for entry in snapshot.into_iter().take(to_run) {
        TIMEOUT_COUNT.with(|c| *c.borrow_mut() += 1);
        let func = v8::Local::new(scope, entry.func);
        let args: Vec<v8::Local<v8::Value>> = entry
            .args
            .iter()
            .map(|g| v8::Local::new(scope, g))
            .collect();
        let undef = v8::undefined(scope).into();
        let tc = &mut v8::TryCatch::new(scope);
        let _ = func.call(tc, undef, &args);
        if tc.has_caught() {
            let err = tc
                .exception()
                .and_then(|e| e.to_string(tc))
                .map(|s| s.to_rust_string_lossy(tc))
                .unwrap_or_default();
            eprintln!("[setTimeout callback error] {}", err);
        }
    }

    DRAIN_GUARD.with(|g| *g.borrow_mut() = false);
}

const MAX_DYNAMIC_SCRIPT_SIZE: usize = 8 * 1024 * 1024; // 8 MB per dynamically loaded script
const MAX_DYNAMIC_SCRIPT_DEPTH: usize = 12;

/// If `child` is a <script> element with a `src`, fetch and execute it synchronously.
/// This lets webpack/next.js chunk loaders inject their dependencies at runtime.
fn execute_appended_script_if_needed(
    scope: &mut v8::HandleScope,
    child_val: v8::Local<v8::Value>,
    child_id: NodeId,
) {
    let (is_script, src) = with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[child_id].data {
            if el.tag_name == "script" {
                return (true, el.attributes.get("src").cloned());
            }
        }
        (false, None)
    });
    if !is_script {
        return;
    }
    let src = match src {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };
    let base = match page_base_url() {
        Some(u) => u,
        None => return,
    };
    let resolved = match incognidium_net::resolve_url(&base, &src) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Dynamic script URL resolve error for {}: {}", src, e);
            return;
        }
    };

    // Dedup: some loaders append the same chunk many times.
    let already_loaded = LOADED_SCRIPTS.with(|s| s.borrow().contains(&resolved));
    if already_loaded {
        return;
    }
    LOADED_SCRIPTS.with(|s| {
        s.borrow_mut().insert(resolved.clone());
    });

    // Avoid runaway recursion from scripts that keep injecting more scripts.
    let depth_ok = DYNAMIC_SCRIPT_DEPTH.with(|d| {
        let cur = *d.borrow();
        if cur >= MAX_DYNAMIC_SCRIPT_DEPTH {
            false
        } else {
            *d.borrow_mut() = cur + 1;
            true
        }
    });
    if !depth_ok {
        eprintln!(
            "Dynamic script recursion limit reached, skipping {}",
            resolved
        );
        return;
    }

    let body = match incognidium_net::fetch_url(&resolved) {
        Ok(resp) => {
            if resp.body.len() > MAX_DYNAMIC_SCRIPT_SIZE {
                eprintln!(
                    "Dynamic script too large ({} KB), skipping {}",
                    resp.body.len() / 1024,
                    resolved
                );
                call_script_event_handler(scope, child_val, "onerror");
                DYNAMIC_SCRIPT_DEPTH.with(|d| *d.borrow_mut() -= 1);
                return;
            }
            resp.body
        }
        Err(e) => {
            eprintln!("Dynamic script fetch error for {}: {}", resolved, e);
            call_script_event_handler(scope, child_val, "onerror");
            DYNAMIC_SCRIPT_DEPTH.with(|d| *d.borrow_mut() -= 1);
            return;
        }
    };

    if let Some(doc) = document_obj(scope) {
        let cs_key = v8_str(scope, "currentScript");
        doc.set(scope, cs_key.into(), child_val);
    }

    let success = {
        let tc = &mut v8::TryCatch::new(scope);
        let source_v8 = v8_str(tc, &body);
        let ok = match v8::Script::compile(tc, source_v8, None) {
            Some(script_obj) => match script_obj.run(tc) {
                Some(_) => true,
                None => false,
            },
            None => false,
        };
        if let Some(err) = tc
            .exception()
            .and_then(|e| e.to_string(tc))
            .map(|s| s.to_rust_string_lossy(tc))
        {
            eprintln!("Dynamic script error in {}: {}", resolved, err);
        } else if ok {
            eprintln!("[dynamic script OK] {} ({} bytes)", resolved, body.len());
        }
        ok
    };

    if let Some(doc) = document_obj(scope) {
        let cs_key = v8_str(scope, "currentScript");
        let null_val = v8::null(scope).into();
        doc.set(scope, cs_key.into(), null_val);
    }

    drain_timeout_queue(scope, 10);
    scope.perform_microtask_checkpoint();

    let handler_name = if success { "onload" } else { "onerror" };
    call_script_event_handler(scope, child_val, handler_name);

    DYNAMIC_SCRIPT_DEPTH.with(|d| {
        let cur = *d.borrow();
        if cur > 0 {
            *d.borrow_mut() = cur - 1;
        }
    });
}

/// Call `element.onload` / `element.onerror` if the page set them as functions.
fn call_script_event_handler(
    scope: &mut v8::HandleScope,
    child_val: v8::Local<v8::Value>,
    name: &str,
) {
    let obj = match v8::Local::<v8::Object>::try_from(child_val) {
        Ok(o) => o,
        Err(_) => return,
    };
    let key = v8_str(scope, name);
    let val = match obj.get(scope, key.into()) {
        Some(v) => v,
        None => return,
    };
    if !val.is_function() {
        return;
    }
    let func = match v8::Local::<v8::Function>::try_from(val) {
        Ok(f) => f,
        Err(_) => return,
    };
    let undef = v8::undefined(scope).into();
    let ev = v8::Object::new(scope);
    let type_key = v8_str(scope, "type");
    let type_val = v8_str(scope, if name == "onload" { "load" } else { "error" });
    let _ = obj.set(scope, type_key.into(), type_val.into());
    let args = [ev.into()];
    let tc = &mut v8::TryCatch::new(scope);
    let _ = func.call(tc, undef, &args);
    if tc.has_caught() {
        let err = tc
            .exception()
            .and_then(|e| e.to_string(tc))
            .map(|s| s.to_rust_string_lossy(tc))
            .unwrap_or_default();
        eprintln!("[script {} handler error] {}", name, err);
    }
}

fn document_obj<'s>(scope: &mut v8::HandleScope<'s>) -> Option<v8::Local<'s, v8::Object>> {
    DOCUMENT_OBJ.with(|d| d.borrow().as_ref().map(|g| v8::Local::new(scope, g)))
}

fn set_document_obj(scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>) {
    let g = v8::Global::new(scope, obj);
    DOCUMENT_OBJ.with(|d| *d.borrow_mut() = Some(g));
}

fn cache_get<'s>(
    scope: &mut v8::HandleScope<'s>,
    node_id: NodeId,
) -> Option<v8::Local<'s, v8::Object>> {
    WRAPPER_CACHE.with(|c| c.borrow().get(&node_id).map(|g| v8::Local::new(scope, g)))
}

fn cache_put(scope: &mut v8::HandleScope, node_id: NodeId, obj: v8::Local<v8::Object>) {
    let global = v8::Global::new(scope, obj);
    WRAPPER_CACHE.with(|c| {
        c.borrow_mut().insert(node_id, global);
    });
}

fn cache_clear() {
    WRAPPER_CACHE.with(|c| c.borrow_mut().clear());
}

fn set_page_base_url(url: String) {
    BASE_URL.with(|b| *b.borrow_mut() = Some(url));
}

fn page_base_url() -> Option<String> {
    BASE_URL.with(|b| b.borrow().clone())
}

fn clear_loaded_scripts() {
    LOADED_SCRIPTS.with(|s| s.borrow_mut().clear());
    DYNAMIC_SCRIPT_DEPTH.with(|d| *d.borrow_mut() = 0);
}

fn with_dom<F, R>(f: F) -> R
where
    F: FnOnce(&mut DomState) -> R,
{
    DOM.with(|cell| {
        let borrow = cell.borrow();
        let dom = borrow.as_ref().expect("DOM not installed");
        let mut state = dom.lock().unwrap();
        f(&mut state)
    })
}

fn set_dom(dom: SharedDom) {
    DOM.with(|cell| {
        *cell.borrow_mut() = Some(dom);
    });
}

fn take_dom() -> Option<SharedDom> {
    DOM.with(|cell| cell.borrow_mut().take())
}

static V8_INIT: Once = Once::new();

fn init_v8() {
    V8_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

// ── helpers ──────────────────────────────────────────────────────────────

fn v8_str<'s>(scope: &mut v8::HandleScope<'s>, s: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, s).unwrap()
}

/// Iterate over the DOM starting at `start`, visiting each node at most once.
/// Uses an explicit stack so broken parent/child pointers (e.g. cycles created
/// by JS DOM manipulation) cannot blow the native Rust stack.
fn walk_dom_nodes<F>(doc: &incognidium_dom::Document, start: NodeId, mut visit: F)
where
    F: FnMut(NodeId),
{
    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![start];
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(node) = doc.nodes.get(id) {
            // Push children in reverse so they are visited in original order.
            for &child in node.children.iter().rev() {
                stack.push(child);
            }
        }
        visit(id);
    }
}

fn set_fn(
    scope: &mut v8::HandleScope,
    obj: v8::Local<v8::Object>,
    name: &str,
    f: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let key = v8_str(scope, name);
    let tmpl = v8::FunctionTemplate::new(scope, f);
    let func = tmpl.get_function(scope).unwrap();
    obj.set(scope, key.into(), func.into());
}

fn set_str(scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>, name: &str, val: &str) {
    let key = v8_str(scope, name);
    let v = v8_str(scope, val);
    obj.set(scope, key.into(), v.into());
}

fn set_int(scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>, name: &str, val: i32) {
    let key = v8_str(scope, name);
    let v = v8::Integer::new(scope, val);
    obj.set(scope, key.into(), v.into());
}

fn set_bool(scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>, name: &str, val: bool) {
    let key = v8_str(scope, name);
    let v = v8::Boolean::new(scope, val);
    obj.set(scope, key.into(), v.into());
}

fn set_num(scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>, name: &str, val: f64) {
    let key = v8_str(scope, name);
    let v = v8::Number::new(scope, val);
    obj.set(scope, key.into(), v.into());
}

fn set_null(scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>, name: &str) {
    let key = v8_str(scope, name);
    let v: v8::Local<v8::Value> = v8::null(scope).into();
    obj.set(scope, key.into(), v);
}

fn get_prop<'s>(
    scope: &mut v8::HandleScope<'s>,
    obj: v8::Local<v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let key = v8_str(scope, name);
    obj.get(scope, key.into())
}

fn extract_node_id(scope: &mut v8::HandleScope, val: v8::Local<v8::Value>) -> Option<NodeId> {
    let obj = val.to_object(scope)?;
    let nid = get_prop(scope, obj, "__node_id__")?;
    nid.int32_value(scope).map(|n| n as NodeId)
}

// ── console ──────────────────────────────────────────────────────────────

fn console_log_impl(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
    prefix: &str,
) {
    let mut out = String::new();
    for i in 0..args.length() {
        if i > 0 {
            out.push(' ');
        }
        let arg = args.get(i);
        if let Some(s) = arg.to_string(scope) {
            out.push_str(&s.to_rust_string_lossy(scope));
        }
    }
    eprintln!("[console.{}] {}", prefix, out);
}

fn console_log(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    console_log_impl(scope, args, rv, "log");
}

fn console_warn(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    console_log_impl(scope, args, rv, "warn");
}

fn console_error(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    console_log_impl(scope, args, rv, "error");
}

// Console timer storage
thread_local! {
    static CONSOLE_TIMERS: RefCell<HashMap<String, std::time::Instant>> = RefCell::new(HashMap::new());
}

fn console_time(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let label = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "default".to_string());
    CONSOLE_TIMERS.with(|timers| {
        timers.borrow_mut().insert(label, std::time::Instant::now());
    });
}

fn console_time_end(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let label = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "default".to_string());
    CONSOLE_TIMERS.with(|timers| {
        if let Some(start) = timers.borrow_mut().remove(&label) {
            let elapsed = start.elapsed();
            eprintln!(
                "[console.time] {}: {}ms",
                label,
                elapsed.as_secs_f64() * 1000.0
            );
        }
    });
}

fn console_time_log(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let label = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "default".to_string());
    CONSOLE_TIMERS.with(|timers| {
        if let Some(start) = timers.borrow().get(&label) {
            let elapsed = start.elapsed();
            eprintln!(
                "[console.timeLog] {}: {}ms",
                label,
                elapsed.as_secs_f64() * 1000.0
            );
        }
    });
}

// Console count storage
thread_local! {
    static CONSOLE_COUNTS: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

fn console_info(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    console_log_impl(scope, args, rv, "info");
}

fn console_debug(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    console_log_impl(scope, args, rv, "debug");
}

fn console_trace(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let mut out = String::new();
    for i in 0..args.length() {
        if i > 0 {
            out.push(' ');
        }
        let arg = args.get(i);
        if let Some(s) = arg.to_string(scope) {
            out.push_str(&s.to_rust_string_lossy(scope));
        }
    }
    eprintln!("[console.trace] {}", out);
    eprintln!("    at (stack trace not available)");
}

fn console_dir(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // console.dir displays an interactive list of properties
    // For now, just log the object
    if args.length() > 0 {
        let arg = args.get(0);
        if let Some(s) = arg.to_string(scope) {
            let str = s.to_rust_string_lossy(scope);
            eprintln!("[console.dir] {}", str);
        }
    }
}

fn console_table(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // console.table displays tabular data
    // For now, just log the data
    if args.length() > 0 {
        let arg = args.get(0);
        if let Some(s) = arg.to_string(scope) {
            let str = s.to_rust_string_lossy(scope);
            eprintln!("[console.table] {}", str);
        }
    }
}

fn console_group(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // console.group - starts a new group (visual indentation)
    // For now, just log a marker
    eprintln!("[console.group]");
}

fn console_group_end(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // console.groupEnd - ends the current group
    eprintln!("[console.groupEnd]");
}

fn console_assert(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    if args.length() == 0 {
        return;
    }
    let assertion = args.get(0);
    if !assertion.is_true() {
        let mut out = String::from("Assertion failed:");
        for i in 1..args.length() {
            out.push(' ');
            let arg = args.get(i);
            if let Some(s) = arg.to_string(scope) {
                out.push_str(&s.to_rust_string_lossy(scope));
            }
        }
        eprintln!("[console.assert] {}", out);
    }
}

fn console_clear(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // console.clear - clears the console
    // In terminal, we can just print some newlines
    eprintln!("\n\n\n\n\n[console.clear]");
}

fn console_count(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let label = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "default".to_string());
    CONSOLE_COUNTS.with(|counts| {
        let mut c = counts.borrow_mut();
        let count = c.entry(label.clone()).or_insert(0);
        *count += 1;
        eprintln!("[console.count] {}: {}", label, *count);
    });
}

fn noop(_: &mut v8::HandleScope, _: v8::FunctionCallbackArguments, _: v8::ReturnValue) {}

fn noop_null(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    rv.set_null();
}

fn noop_false(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    rv.set_bool(false);
}

fn noop_true(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    rv.set_bool(true);
}

fn noop_empty_arr(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let arr = v8::Array::new(scope, 0);
    rv.set(arr.into());
}

fn noop_str(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    rv.set(v8_str(scope, "").into());
}

fn noop_obj(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let obj = v8::Object::new(scope);
    rv.set(obj.into());
}

fn noop_promise(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // Create an empty object that looks like a promise
    let obj = v8::Object::new(scope);
    set_fn(scope, obj, "then", noop);
    set_fn(scope, obj, "catch", noop);
    set_fn(scope, obj, "finally", noop);
    rv.set(obj.into());
}

fn noop_promise_arr(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // A promise-like that resolves to an empty array.
    let obj = v8::Object::new(scope);
    set_fn(scope, obj, "catch", noop);
    set_fn(scope, obj, "finally", noop);
    fn then_arr(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let cb = args.get(0);
        if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
            let arr = v8::Array::new(scope, 0);
            let undef = v8::undefined(scope).into();
            let tc = &mut v8::TryCatch::new(scope);
            let _ = func.call(tc, undef, &[arr.into()]);
        }
        rv.set(args.this().into());
    }
    set_fn(scope, obj, "then", then_arr);
    rv.set(obj.into());
}

/// .finally() implementation for our synchronous fake promises.  It runs the
/// callback and returns the same promise-like object so chaining keeps working.
fn promise_finally_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let cb = args.get(0);
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        let undef = v8::undefined(scope).into();
        let tc = &mut v8::TryCatch::new(scope);
        let _ = func.call(tc, undef, &[]);
    }
    rv.set(args.this().into());
}

/// Returns true if `val` looks like a thenable (has a callable `then`).
fn is_thenable(scope: &mut v8::HandleScope, val: v8::Local<v8::Value>) -> bool {
    if let Ok(obj) = v8::Local::<v8::Object>::try_from(val) {
        let then_key = v8_str(scope, "then");
        if let Some(then_val) = obj.get(scope, then_key.into()) {
            return then_val.is_function();
        }
    }
    false
}

/// Wrap a callback result as a fake-promise.  If the result is itself a
/// thenable, return it directly so nested `.then(r => r.json()).then(data => ...)`
/// chains unwrap correctly; otherwise return a resolved promise with the value.
fn wrap_thenable_or_value(
    scope: &mut v8::HandleScope,
    value: v8::Local<v8::Value>,
    mut rv: v8::ReturnValue,
) {
    if is_thenable(scope, value) {
        rv.set(value);
        return;
    }
    let ret = v8::Object::new(scope);
    let result_key = v8_str(scope, "__value");
    ret.set(scope, result_key.into(), value);
    set_fn(scope, ret, "then", resolved_then_cb);
    set_fn(scope, ret, "catch", noop);
    set_fn(scope, ret, "finally", promise_finally_cb);
    rv.set(ret.into());
}

/// Synchronous fetch() for JS. Executes request immediately and returns a
/// fake-promise that resolves on the next `.then()` call.
fn fetch_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url_val = args.get(0);
    let url_str = url_val.to_rust_string_lossy(scope);

    // Resolve relative URLs against window.location.href, falling back to the
    // page base URL set from the first script origin.  Without a base, path-only
    // URLs would be misinterpreted as local file paths.
    let resolved_url = {
        let context = scope.get_current_context();
        let global = context.global(scope);
        let loc_key = v8_str(scope, "location");
        let loc_url = global
            .get(scope, loc_key.into())
            .and_then(|v| v.to_object(scope))
            .and_then(|loc| {
                let href_key = v8_str(scope, "href");
                loc.get(scope, href_key.into())
            })
            .and_then(|v| v.to_string(scope))
            .map(|s| s.to_rust_string_lossy(scope));
        let base_url = loc_url.or_else(|| page_base_url()).unwrap_or_default();
        if base_url.is_empty() {
            url_str.clone()
        } else {
            incognidium_net::resolve_url(&base_url, &url_str).unwrap_or_else(|_| url_str.clone())
        }
    };

    let (ok, status, status_text, body, content_type) =
        match incognidium_net::fetch_url(&resolved_url) {
            Ok(resp) => {
                eprintln!(
                    "[fetch OK] {} -> {} ({} bytes)",
                    resolved_url,
                    resp.status,
                    resp.body.len()
                );
                let ok = resp.status >= 200 && resp.status < 300;
                let st = if resp.status == 200 {
                    "OK"
                } else if resp.status == 404 {
                    "Not Found"
                } else {
                    ""
                };
                (
                    ok,
                    resp.status as i32,
                    st.to_string(),
                    resp.body,
                    resp.content_type,
                )
            }
            Err(e) => {
                eprintln!("[fetch ERR] {} -> {}", resolved_url, e);
                (
                    false,
                    0,
                    "Network Error".to_string(),
                    String::new(),
                    String::new(),
                )
            }
        };

    // Build response object
    let resp_obj = v8::Object::new(scope);
    set_bool(scope, resp_obj, "ok", ok);
    set_int(scope, resp_obj, "status", status);
    set_str(scope, resp_obj, "statusText", &status_text);
    set_str(scope, resp_obj, "__body", &body);
    set_str(scope, resp_obj, "__content_type", &content_type);

    // .text() method
    fn resp_text_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let key = v8_str(scope, "__body");
        let body = match this.get(scope, key.into()) {
            Some(v) => match v.to_string(scope) {
                Some(s) => s.to_rust_string_lossy(scope),
                None => String::new(),
            },
            None => String::new(),
        };
        let ret = v8::Object::new(scope);
        let body_val = v8_str(scope, &body);
        let text_key = v8_str(scope, "__text");
        ret.set(scope, text_key.into(), body_val.into());
        set_fn(scope, ret, "then", resp_text_then_cb);
        set_fn(scope, ret, "catch", noop);
        set_fn(scope, ret, "finally", promise_finally_cb);
        rv.set(ret.into());
    }

    // .json() method
    fn resp_json_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let key = v8_str(scope, "__body");
        let body = match this.get(scope, key.into()) {
            Some(v) => match v.to_string(scope) {
                Some(s) => s.to_rust_string_lossy(scope),
                None => String::new(),
            },
            None => String::new(),
        };
        let ret = v8::Object::new(scope);
        let body_val = v8_str(scope, &body);
        let json_key = v8_str(scope, "__json_text");
        ret.set(scope, json_key.into(), body_val.into());
        set_fn(scope, ret, "then", resp_json_then_cb);
        set_fn(scope, ret, "catch", noop);
        set_fn(scope, ret, "finally", promise_finally_cb);
        rv.set(ret.into());
    }

    // Headers object with .get() method
    fn headers_get_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let name_key = v8_str(scope, "__name");
        let name = match this.get(scope, name_key.into()) {
            Some(v) => match v.to_string(scope) {
                Some(s) => s.to_rust_string_lossy(scope),
                None => String::new(),
            },
            None => String::new(),
        };
        let val = match name.as_str() {
            "content-type" => {
                let ct_key = v8_str(scope, "__content_type");
                match this.get(scope, ct_key.into()) {
                    Some(v) => match v.to_string(scope) {
                        Some(s) => s.to_rust_string_lossy(scope),
                        None => String::new(),
                    },
                    None => String::new(),
                }
            }
            _ => String::new(),
        };
        rv.set(v8_str(scope, &val).into());
    }
    let headers_obj = v8::Object::new(scope);
    set_str(scope, headers_obj, "__name", "");
    set_str(scope, headers_obj, "__content_type", &content_type);
    set_fn(scope, headers_obj, "get", headers_get_cb);
    let headers_key = v8_str(scope, "headers");
    resp_obj.set(scope, headers_key.into(), headers_obj.into());

    set_fn(scope, resp_obj, "text", resp_text_cb);
    set_fn(scope, resp_obj, "json", resp_json_cb);

    // Build return fake-promise
    let ret = v8::Object::new(scope);
    let resp_val: v8::Local<v8::Value> = resp_obj.into();
    let resp_key = v8_str(scope, "__resp");
    ret.set(scope, resp_key.into(), resp_val);
    set_fn(scope, ret, "then", fetch_then_cb);
    set_fn(scope, ret, "catch", fetch_catch_cb);
    set_fn(scope, ret, "finally", promise_finally_cb);
    rv.set(ret.into());
}

fn resp_text_then_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let key = v8_str(scope, "__text");
    let text = match this.get(scope, key.into()) {
        Some(v) => match v.to_string(scope) {
            Some(s) => s.to_rust_string_lossy(scope),
            None => String::new(),
        },
        None => String::new(),
    };
    let cb = args.get(0);
    let mut result = v8::undefined(scope).into();
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        let text_val = v8_str(scope, &text);
        let undef = v8::undefined(scope).into();
        let fallback = v8::undefined(scope).into();
        let tc = &mut v8::TryCatch::new(scope);
        result = func.call(tc, undef, &[text_val.into()]).unwrap_or(fallback);
        if tc.has_caught() {
            let err = tc
                .exception()
                .and_then(|e| e.to_string(tc))
                .map(|s| s.to_rust_string_lossy(tc))
                .unwrap_or_default();
            eprintln!("[fetch text then error] {}", err);
        }
    }
    wrap_thenable_or_value(scope, result, rv);
}

fn resp_json_then_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let key = v8_str(scope, "__json_text");
    let text = match this.get(scope, key.into()) {
        Some(v) => match v.to_string(scope) {
            Some(s) => s.to_rust_string_lossy(scope),
            None => String::new(),
        },
        None => String::new(),
    };
    let cb = args.get(0);
    let mut result = v8::undefined(scope).into();
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        let json_str = v8_str(scope, &text);
        let parsed = v8::json::parse(scope, json_str).unwrap_or_else(|| v8::null(scope).into());
        let undef = v8::undefined(scope).into();
        let fallback = v8::undefined(scope).into();
        let tc = &mut v8::TryCatch::new(scope);
        result = func.call(tc, undef, &[parsed]).unwrap_or(fallback);
        if tc.has_caught() {
            let err = tc
                .exception()
                .and_then(|e| e.to_string(tc))
                .map(|s| s.to_rust_string_lossy(tc))
                .unwrap_or_default();
            eprintln!("[fetch json then error] {}", err);
        }
    }
    wrap_thenable_or_value(scope, result, rv);
}

fn fetch_then_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let key = v8_str(scope, "__resp");
    let maybe_resp = this.get(scope, key.into());
    let resp = match maybe_resp {
        Some(v) => v,
        None => v8::undefined(scope).into(),
    };
    let cb = args.get(0);
    let mut result = v8::undefined(scope).into();
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        let undef = v8::undefined(scope).into();
        let fallback = v8::undefined(scope).into();
        let tc = &mut v8::TryCatch::new(scope);
        result = func.call(tc, undef, &[resp]).unwrap_or(fallback);
        if tc.has_caught() {
            let err = tc
                .exception()
                .and_then(|e| e.to_string(tc))
                .map(|s| s.to_rust_string_lossy(tc))
                .unwrap_or_default();
            eprintln!("[fetch then error] {}", err);
        }
    }
    wrap_thenable_or_value(scope, result, rv);
}

fn fetch_catch_cb(
    _scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // Synchronous fetch never rejects; return self for chaining
    rv.set(args.this().into());
}

/// Generic `.then` for a resolved fake-promise (chains indefinitely).
fn resolved_then_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let key = v8_str(scope, "__value");
    let value = this
        .get(scope, key.into())
        .unwrap_or_else(|| v8::undefined(scope).into());
    let cb = args.get(0);
    let mut result = v8::undefined(scope).into();
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        let undef = v8::undefined(scope).into();
        let fallback = v8::undefined(scope).into();
        let tc = &mut v8::TryCatch::new(scope);
        result = func.call(tc, undef, &[value]).unwrap_or(fallback);
        if tc.has_caught() {
            let err = tc
                .exception()
                .and_then(|e| e.to_string(tc))
                .map(|s| s.to_rust_string_lossy(tc))
                .unwrap_or_default();
            eprintln!("[resolved then error] {}", err);
        }
    }
    wrap_thenable_or_value(scope, result, rv);
}

// ── Web Storage API (localStorage/sessionStorage) ─────────────────────────

thread_local! {
    static LOCAL_STORAGE: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    static SESSION_STORAGE: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

fn local_storage_get_item(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let key = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    LOCAL_STORAGE.with(|s| {
        let storage = s.borrow();
        match storage.get(&key) {
            Some(value) => rv.set(v8_str(scope, value).into()),
            None => rv.set_null(),
        }
    });
}

fn local_storage_set_item(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let value = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    LOCAL_STORAGE.with(|s| {
        s.borrow_mut().insert(key, value);
    });
}

fn local_storage_remove_item(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    LOCAL_STORAGE.with(|s| {
        s.borrow_mut().remove(&key);
    });
}

fn local_storage_clear(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    LOCAL_STORAGE.with(|s| {
        s.borrow_mut().clear();
    });
}

fn local_storage_key(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let index = args.get(0).int32_value(scope).unwrap_or(0) as usize;
    LOCAL_STORAGE.with(|s| {
        let storage = s.borrow();
        let keys: Vec<&String> = storage.keys().collect();
        match keys.get(index) {
            Some(key) => rv.set(v8_str(scope, key).into()),
            None => rv.set_null(),
        }
    });
}

fn local_storage_length(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    LOCAL_STORAGE.with(|s| {
        let storage = s.borrow();
        rv.set(v8::Integer::new(scope, storage.len() as i32).into());
    });
}

fn session_storage_get_item(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let key = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    SESSION_STORAGE.with(|s| {
        let storage = s.borrow();
        match storage.get(&key) {
            Some(value) => rv.set(v8_str(scope, value).into()),
            None => rv.set_null(),
        }
    });
}

fn session_storage_set_item(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let value = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    SESSION_STORAGE.with(|s| {
        s.borrow_mut().insert(key, value);
    });
}

fn session_storage_remove_item(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let key = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    SESSION_STORAGE.with(|s| {
        s.borrow_mut().remove(&key);
    });
}

fn session_storage_clear(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    SESSION_STORAGE.with(|s| {
        s.borrow_mut().clear();
    });
}

fn session_storage_key(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let index = args.get(0).int32_value(scope).unwrap_or(0) as usize;
    SESSION_STORAGE.with(|s| {
        let storage = s.borrow();
        let keys: Vec<&String> = storage.keys().collect();
        match keys.get(index) {
            Some(key) => rv.set(v8_str(scope, key).into()),
            None => rv.set_null(),
        }
    });
}

fn session_storage_length(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    SESSION_STORAGE.with(|s| {
        let storage = s.borrow();
        rv.set(v8::Integer::new(scope, storage.len() as i32).into());
    });
}

// ── Performance API ───────────────────────────────────────────────────────

thread_local! {
    static PERFORMANCE_MARKS: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
    static PERFORMANCE_MEASURES: RefCell<HashMap<String, f64>> = RefCell::new(HashMap::new());
    static PERFORMANCE_START_TIME: std::cell::Cell<std::time::Instant> = std::cell::Cell::new(std::time::Instant::now());
}

fn performance_now_cb(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let start = PERFORMANCE_START_TIME.get();
    let elapsed = start.elapsed();
    let millis = elapsed.as_secs_f64() * 1000.0;
    rv.set(v8::Number::new(scope, millis).into());
}

fn performance_mark_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let start = PERFORMANCE_START_TIME.get();
    let elapsed = start.elapsed();
    let millis = elapsed.as_secs_f64() * 1000.0;
    PERFORMANCE_MARKS.with(|m| {
        m.borrow_mut().insert(name, millis);
    });
}

fn performance_measure_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if name.is_empty() {
        return;
    }

    let start_mark = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let end_mark = args
        .get(2)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    PERFORMANCE_MARKS.with(|m| {
        let marks = m.borrow();
        let start_time = marks.get(&start_mark).copied().unwrap_or(0.0);
        let end_time = marks.get(&end_mark).copied().unwrap_or_else(|| {
            let start = PERFORMANCE_START_TIME.get();
            start.elapsed().as_secs_f64() * 1000.0
        });
        let duration = end_time - start_time;
        PERFORMANCE_MEASURES.with(|measures| {
            measures.borrow_mut().insert(name, duration);
        });
    });
}

fn performance_clear_marks_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    PERFORMANCE_MARKS.with(|m| {
        let mut marks = m.borrow_mut();
        if name.is_empty() {
            marks.clear();
        } else {
            marks.remove(&name);
        }
    });
}

fn performance_clear_measures_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    PERFORMANCE_MEASURES.with(|m| {
        let mut measures = m.borrow_mut();
        if name.is_empty() {
            measures.clear();
        } else {
            measures.remove(&name);
        }
    });
}

fn performance_get_entries_by_name_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let entry_type = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let arr = v8::Array::new(scope, 0);
    let mut idx = 0;

    PERFORMANCE_MARKS.with(|m| {
        if entry_type.is_empty() || entry_type == "mark" {
            if let Some(&time) = m.borrow().get(&name) {
                let entry = v8::Object::new(scope);
                set_str(scope, entry, "name", &name);
                set_str(scope, entry, "entryType", "mark");
                set_num(scope, entry, "startTime", time);
                set_num(scope, entry, "duration", 0.0);
                arr.set_index(scope, idx, entry.into());
                idx += 1;
            }
        }
    });

    PERFORMANCE_MEASURES.with(|m| {
        if entry_type.is_empty() || entry_type == "measure" {
            if let Some(&duration) = m.borrow().get(&name) {
                let entry = v8::Object::new(scope);
                set_str(scope, entry, "name", &name);
                set_str(scope, entry, "entryType", "measure");
                set_num(scope, entry, "startTime", 0.0);
                set_num(scope, entry, "duration", duration);
                arr.set_index(scope, idx, entry.into());
            }
        }
    });

    rv.set(arr.into());
}

fn performance_get_entries_by_type_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let entry_type = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let arr = v8::Array::new(scope, 0);

    if entry_type == "mark" {
        PERFORMANCE_MARKS.with(|m| {
            for (idx, (name, &time)) in m.borrow().iter().enumerate() {
                let entry = v8::Object::new(scope);
                set_str(scope, entry, "name", name);
                set_str(scope, entry, "entryType", "mark");
                set_num(scope, entry, "startTime", time);
                set_num(scope, entry, "duration", 0.0);
                arr.set_index(scope, idx as u32, entry.into());
            }
        });
    } else if entry_type == "measure" {
        PERFORMANCE_MEASURES.with(|m| {
            for (idx, (name, &duration)) in m.borrow().iter().enumerate() {
                let entry = v8::Object::new(scope);
                set_str(scope, entry, "name", name);
                set_str(scope, entry, "entryType", "measure");
                set_num(scope, entry, "startTime", 0.0);
                set_num(scope, entry, "duration", duration);
                arr.set_index(scope, idx as u32, entry.into());
            }
        });
    }

    rv.set(arr.into());
}

fn performance_get_entries_cb(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let arr = v8::Array::new(scope, 0);
    let mut idx = 0u32;

    PERFORMANCE_MARKS.with(|m| {
        for (name, &time) in m.borrow().iter() {
            let entry = v8::Object::new(scope);
            set_str(scope, entry, "name", name);
            set_str(scope, entry, "entryType", "mark");
            set_num(scope, entry, "startTime", time);
            set_num(scope, entry, "duration", 0.0);
            arr.set_index(scope, idx, entry.into());
            idx += 1;
        }
    });

    PERFORMANCE_MEASURES.with(|m| {
        for (name, &duration) in m.borrow().iter() {
            let entry = v8::Object::new(scope);
            set_str(scope, entry, "name", name);
            set_str(scope, entry, "entryType", "measure");
            set_num(scope, entry, "startTime", 0.0);
            set_num(scope, entry, "duration", duration);
            arr.set_index(scope, idx, entry.into());
            idx += 1;
        }
    });

    rv.set(arr.into());
}

// ── HTML serialization helpers ─────────────────────────────────────────────

/// Serialize a DOM node to HTML string.
/// `visited` prevents infinite recursion on cyclic trees created by JS.
fn serialize_node_to_html(
    node_id: NodeId,
    doc: &Document,
    inner_only: bool,
    visited: &mut std::collections::HashSet<NodeId>,
) -> String {
    if !visited.insert(node_id) {
        return String::new();
    }
    let node = match doc.nodes.get(node_id) {
        Some(n) => n,
        None => return String::new(),
    };

    match &node.data {
        NodeData::Element(el) => {
            let tag = &el.tag_name;
            let mut html = String::new();

            if !inner_only {
                html.push_str("<");
                html.push_str(tag);

                // Serialize attributes
                for (attr_name, attr_value) in &el.attributes {
                    html.push(' ');
                    html.push_str(attr_name);
                    if !attr_value.is_empty() {
                        html.push_str("=\"");
                        // Escape quotes in attribute values
                        let escaped = attr_value.replace('"', "&quot;");
                        html.push_str(&escaped);
                        html.push('"');
                    }
                }

                // Self-closing tags
                if is_void_element(tag) {
                    html.push_str(" />");
                    return html;
                }
                html.push('>');
            }

            // Serialize children
            for child_id in &node.children {
                html.push_str(&serialize_node_to_html(*child_id, doc, false, visited));
            }

            if !inner_only {
                html.push_str("</");
                html.push_str(tag);
                html.push('>');
            }

            html
        }
        NodeData::Text(text_data) => {
            // Escape HTML entities in text content
            escape_html_entities(&text_data.content)
        }
        NodeData::Comment(comment) => {
            format!("<!--{}-->", comment)
        }
        _ => String::new(),
    }
}

/// Serialize only the children (innerHTML).
fn serialize_inner_html(node_id: NodeId, doc: &Document) -> String {
    let node = match doc.nodes.get(node_id) {
        Some(n) => n,
        None => return String::new(),
    };

    let mut html = String::new();
    let mut visited = std::collections::HashSet::new();
    for child_id in &node.children {
        html.push_str(&serialize_node_to_html(*child_id, doc, false, &mut visited));
    }
    html
}

/// Check if an element is a void element (no closing tag)
fn is_void_element(tag: &str) -> bool {
    matches!(
        tag.to_lowercase().as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Escape HTML entities in text content
fn escape_html_entities(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Parsed HTML node from a fragment (tree structure).
#[derive(Debug)]
struct HtmlFragmentNode {
    tag: String,
    attrs: HashMap<String, String>,
    children: Vec<HtmlFragmentNode>,
    text: Option<String>,
}

/// HTML parser for innerHTML/outerHTML setting — builds a proper tree so nested
/// elements keep their children.
fn parse_html_fragment(html: &str) -> Vec<HtmlFragmentNode> {
    let mut stack: Vec<HtmlFragmentNode> = Vec::new();
    let mut result: Vec<HtmlFragmentNode> = Vec::new();
    let mut chars = html.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            // Closing tag
            if chars.peek() == Some(&'/') {
                chars.next(); // skip '/'
                let mut tag = String::new();
                while let Some(c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':') {
                        tag.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                while chars.next() != Some('>') {}
                let tag_lower = tag.to_lowercase();
                // Pop until we find a matching tag (tolerate mismatches)
                if let Some(node) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    } else {
                        result.push(node);
                    }
                }
                continue;
            }

            // Comment / doctype — skip
            if chars.peek() == Some(&'!') {
                chars.next();
                if chars.peek() == Some(&'-') && chars.clone().nth(1) == Some('-') {
                    chars.next();
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '-' && chars.peek() == Some(&'-') {
                            chars.next();
                            if chars.peek() == Some(&'>') {
                                chars.next();
                                break;
                            }
                        }
                    }
                } else {
                    while chars.next() != Some('>') {}
                }
                continue;
            }

            // Opening tag
            let mut tag = String::new();
            while let Some(c) = chars.peek() {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':') {
                    tag.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if tag.is_empty() {
                continue;
            }

            // Parse attributes
            let mut attrs = HashMap::new();
            let mut self_closing = false;
            loop {
                while chars.peek() == Some(&' ') || chars.peek() == Some(&'\t') {
                    chars.next();
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    break;
                }
                if chars.peek() == Some(&'/') {
                    chars.next();
                    if chars.peek() == Some(&'>') {
                        chars.next();
                    }
                    self_closing = true;
                    break;
                }
                let mut attr_name = String::new();
                while let Some(c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '@') {
                        attr_name.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                if attr_name.is_empty() {
                    chars.next();
                    continue;
                }
                while chars.peek() == Some(&' ') || chars.peek() == Some(&'\t') {
                    chars.next();
                }
                let mut attr_value = String::new();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    while chars.peek() == Some(&' ') || chars.peek() == Some(&'\t') {
                        chars.next();
                    }
                    let quote = chars.peek().copied();
                    if quote == Some('"') || quote == Some('\'') {
                        chars.next();
                        let qc = quote.unwrap();
                        while let Some(c) = chars.next() {
                            if c == qc {
                                break;
                            }
                            attr_value.push(c);
                        }
                    } else {
                        while let Some(c) = chars.peek() {
                            if matches!(c, ' ' | '\t' | '\n' | '\r' | '>' | '/') {
                                break;
                            }
                            attr_value.push(chars.next().unwrap());
                        }
                    }
                }
                attrs.insert(attr_name.to_lowercase(), attr_value);
            }

            let node = HtmlFragmentNode {
                tag: tag.to_lowercase(),
                attrs,
                children: Vec::new(),
                text: None,
            };
            if self_closing || is_void_element(&node.tag) {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    result.push(node);
                }
            } else {
                stack.push(node);
            }
        } else if !c.is_whitespace() {
            let mut text = String::new();
            text.push(c);
            while let Some(c) = chars.peek() {
                if *c == '<' {
                    break;
                }
                text.push(chars.next().unwrap());
            }
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let node = HtmlFragmentNode {
                    tag: "text".to_string(),
                    attrs: HashMap::new(),
                    children: Vec::new(),
                    text: Some(text),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    result.push(node);
                }
            }
        }
    }

    // Drain remaining stack
    while let Some(node) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(node);
        } else {
            result.push(node);
        }
    }

    result
}

/// innerHTML getter — serializes children each time it’s read.
fn inner_html_getter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let node_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8_str(scope, "").into());
            return;
        }
    };
    let html = with_dom(|state| serialize_inner_html(node_id, &state.document));
    rv.set(v8_str(scope, &html).into());
}

/// outerHTML getter — serializes the element including itself.
fn outer_html_getter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let node_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8_str(scope, "").into());
            return;
        }
    };
    let html = with_dom(|state| {
        let mut visited = std::collections::HashSet::new();
        serialize_node_to_html(node_id, &state.document, false, &mut visited)
    });
    rv.set(v8_str(scope, &html).into());
}

/// Recursively create DOM nodes from parsed HTML fragments and wire parent/child
/// links. Returns every created NodeId so callers can wrap them.
///
/// `push` controls whether top-level nodes are appended to `parent_id.children`.
/// Callers that need to insert at a specific position can pass `false` and splice
/// the returned ids themselves.
fn build_fragment_tree(
    fragments: Vec<HtmlFragmentNode>,
    parent_id: NodeId,
    push: bool,
) -> Vec<NodeId> {
    let mut all_ids = Vec::new();
    for frag in fragments {
        let new_id = with_dom(|state| {
            let id = state.document.nodes.len();
            if frag.tag == "text" {
                state.document.nodes.push(Node {
                    id,
                    parent: Some(parent_id),
                    children: Vec::new(),
                    data: NodeData::Text(TextData {
                        content: frag.text.unwrap_or_default(),
                    }),
                });
            } else {
                let mut el = ElementData::new(&frag.tag);
                for (k, v) in frag.attrs {
                    el.attributes.insert(k, v);
                }
                state.document.nodes.push(Node {
                    id,
                    parent: Some(parent_id),
                    children: Vec::new(),
                    data: NodeData::Element(el),
                });
            }
            id
        });
        all_ids.push(new_id);

        // Add to parent's children list when requested.
        if push {
            with_dom(|state| {
                state.document.nodes[parent_id].children.push(new_id);
            });
        }

        // Recurse for nested children; they always attach to their direct parent.
        if !frag.children.is_empty() {
            let child_ids = build_fragment_tree(frag.children, new_id, true);
            all_ids.extend(child_ids);
        }
    }
    all_ids
}

/// innerHTML setter — parses HTML, clears children, and appends new nodes.
fn inner_html_setter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    value: v8::Local<v8::Value>,
    args: v8::PropertyCallbackArguments,
    _rv: v8::ReturnValue<()>,
) {
    let this = args.this();
    let node_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    let html = value
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    // Detach existing children
    with_dom(|state| {
        let children: Vec<NodeId> = state.document.nodes[node_id].children.clone();
        for child in children {
            state.document.nodes[child].parent = None;
        }
        state.document.nodes[node_id].children.clear();
    });

    let fragments = parse_html_fragment(&html);
    let all_ids = build_fragment_tree(fragments, node_id, true);

    // Wrap all created nodes (outside with_dom because wrap_element calls with_dom)
    for new_id in all_ids {
        let _ = wrap_element(scope, new_id);
    }
}

/// JS Math.random() equivalent
fn js_rand() -> f64 {
    use std::cell::RefCell;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    thread_local! {
        static COUNTER: RefCell<u64> = RefCell::new(0);
    }
    COUNTER.with(|c| {
        let mut count = c.borrow_mut();
        *count += 1;
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut hasher = DefaultHasher::new();
        now.hash(&mut hasher);
        count.hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        let hash = hasher.finish();
        // Convert to 0-1 range (not perfectly uniform but good enough)
        (hash as f64) / (u64::MAX as f64)
    })
}

/// setTimeout(fn, ms) — invoke callback synchronously (ignore delay).
/// Lets React's scheduler actually flush render work.
fn set_timeout_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let cb = args.get(0);
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        // Extra args beyond (cb, delay) are passed to the callback.
        let mut cb_args: Vec<v8::Local<v8::Value>> = Vec::new();
        for i in 2..args.length() {
            cb_args.push(args.get(i));
        }
        queue_timeout(scope, func, cb_args);
    }
    rv.set(v8::Integer::new(scope, 0).into());
}

/// requestAnimationFrame(callback) — enqueue callback for later with a timestamp.
/// Returns an ID that can be used with cancelAnimationFrame.
fn request_animation_frame_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let cb = args.get(0);
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        // Get current time in milliseconds since some epoch
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64;
        let timestamp = v8::Number::new(scope, now);
        queue_timeout(scope, func, vec![timestamp.into()]);
    }
    // Return a frame ID (using 1 as a simple ID)
    rv.set(v8::Integer::new(scope, 1).into());
}

/// cancelAnimationFrame(id) — no-op since we execute synchronously.
fn cancel_animation_frame_cb(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // No-op - we execute rAF callbacks immediately
}

/// queueMicrotask(fn) — V8 has native microtask support via EnqueueMicrotask.
fn queue_microtask_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let cb = args.get(0);
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        scope.enqueue_microtask(func);
    }
}

// ── dynamic tree-property accessor ─────────────────────────────────────────

/// Minimal `element.attributes` accessor. jQuery/Sizzle support tests read
/// `elem.attributes[name].expando`, and many sites enumerate attributes.
fn attributes_getter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let node_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8::undefined(scope).into());
            return;
        }
    };
    let attrs = with_dom(|state| {
        if let Some(NodeData::Element(el)) = state.document.nodes.get(node_id).map(|n| &n.data) {
            el.attributes.clone()
        } else {
            std::collections::HashMap::<String, String>::new()
        }
    });
    let map = v8::Object::new(scope);
    let len = attrs.len();
    for (name, value) in attrs {
        let attr_obj = v8::Object::new(scope);
        set_str(scope, attr_obj, "name", &name);
        set_str(scope, attr_obj, "value", &value);
        set_bool(scope, attr_obj, "expando", false);
        let key = v8_str(scope, &name);
        let _ = map.set(scope, key.into(), attr_obj.into());
    }
    set_int(scope, map, "length", len as i32);
    rv.set(map.into());
}

// ── document.write / writeln ─────────────────────────────────────────────

/// Clone a node (and its descendants) from a parsed fragment Document into the
/// live document, appending it under `parent`.  If the cloned node is a
/// `<script>` with a `src`, execute it synchronously so document.write-based
/// script loaders (common on Fox News and legacy ad tags) keep working.
fn clone_and_append_fragment_node(
    scope: &mut v8::HandleScope,
    frag: &incognidium_dom::Document,
    frag_id: NodeId,
    parent: NodeId,
) {
    let new_data = match &frag.nodes[frag_id].data {
        NodeData::Element(e) => NodeData::Element(e.clone()),
        NodeData::Text(t) => NodeData::Text(t.clone()),
        _ => return,
    };
    let new_id = with_dom(|state| {
        let id = state.document.nodes.len();
        state.document.nodes.push(Node {
            id,
            parent: Some(parent),
            children: Vec::new(),
            data: new_data,
        });
        state.document.nodes[parent].children.push(id);
        id
    });
    // Recurse into children in source order.
    let child_ids: Vec<NodeId> = frag.nodes[frag_id].children.iter().copied().collect();
    for child_id in child_ids {
        clone_and_append_fragment_node(scope, frag, child_id, new_id);
    }
    // Run appended scripts so document.write can bootstrap further loaders.
    if let NodeData::Element(ref el) = frag.nodes[frag_id].data {
        if el.tag_name == "script" {
            let child_val = wrap_element(scope, new_id).into();
            execute_appended_script_if_needed(scope, child_val, new_id);
        }
    }
}

fn document_write_with(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
    append_newline: bool,
) {
    let mut html = String::new();
    for i in 0..args.length() {
        if let Some(s) = args.get(i).to_string(scope) {
            html.push_str(&s.to_rust_string_lossy(scope));
        }
    }
    if append_newline {
        html.push('\n');
    }

    // Insert near the currently executing script when possible; otherwise fall
    // back to appending to <body>.  This mimics how browsers handle
    // document.write from synchronous scripts during parse.
    let target_parent = if let Some(doc) = document_obj(scope) {
        let cs_key = v8_str(scope, "currentScript");
        if let Some(cs_val) = doc.get(scope, cs_key.into()) {
            if let Ok(cs_obj) = v8::Local::<v8::Object>::try_from(cs_val) {
                if let Some(cs_id) = extract_node_id(scope, cs_obj.into()) {
                    let script_parent = with_dom(|state| state.document.nodes[cs_id].parent);
                    script_parent.or_else(|| with_dom(|state| state.document.body()))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let Some(parent) = target_parent else {
        rv.set(v8::undefined(scope).into());
        return;
    };

    let wrapped = format!("<!DOCTYPE html><html><body>{}</body></html>", html);
    let frag = parse_html(&wrapped);
    if let Some(body_id) = frag.body() {
        let child_ids: Vec<NodeId> = frag.nodes[body_id].children.iter().copied().collect();
        for child_id in child_ids {
            clone_and_append_fragment_node(scope, &frag, child_id, parent);
        }
    }
    rv.set(v8::undefined(scope).into());
}

fn document_write_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    document_write_with(scope, args, rv, false);
}

fn document_writeln_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue,
) {
    document_write_with(scope, args, rv, true);
}

fn global_add_event_listener_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let event_type = args.get(0).to_rust_string_lossy(scope);
    let cb = args.get(1);
    if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
        add_window_event_listener(scope, &event_type, func);
    }
}

fn global_remove_event_listener_cb(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // Removing specific listeners by function identity is expensive in V8
    // globals; for now removal is a no-op.  Pages rarely depend on it.
}

enum TreeVal {
    None,
    Single(Option<NodeId>),
    Many(Vec<NodeId>),
    Count(i32),
}

fn tree_property_getter_cb(
    scope: &mut v8::HandleScope,
    key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let node_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8::undefined(scope).into());
            return;
        }
    };
    let key_str = key
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let result = with_dom(|state| {
        let node = match state.document.nodes.get(node_id) {
            Some(n) => n,
            None => return TreeVal::None,
        };
        let is_element = |cid: &NodeId| {
            matches!(
                state.document.nodes.get(*cid).map(|n| &n.data),
                Some(NodeData::Element(_))
            )
        };
        match key_str.as_str() {
            "parentNode" | "parentElement" => TreeVal::Single(node.parent),
            "firstChild" => TreeVal::Single(node.children.first().copied()),
            "lastChild" => TreeVal::Single(node.children.last().copied()),
            "firstElementChild" => {
                TreeVal::Single(node.children.iter().find(|cid| is_element(cid)).copied())
            }
            "lastElementChild" => {
                TreeVal::Single(node.children.iter().rfind(|cid| is_element(cid)).copied())
            }
            "nextSibling" => {
                if let Some(pid) = node.parent {
                    let siblings = &state.document.nodes[pid].children;
                    if let Some(i) = siblings.iter().position(|&c| c == node_id) {
                        TreeVal::Single(siblings.get(i + 1).copied())
                    } else {
                        TreeVal::Single(None)
                    }
                } else {
                    TreeVal::Single(None)
                }
            }
            "previousSibling" => {
                if let Some(pid) = node.parent {
                    let siblings = &state.document.nodes[pid].children;
                    if let Some(i) = siblings.iter().position(|&c| c == node_id) {
                        TreeVal::Single(siblings.get(i.wrapping_sub(1)).copied())
                    } else {
                        TreeVal::Single(None)
                    }
                } else {
                    TreeVal::Single(None)
                }
            }
            "nextElementSibling" => {
                if let Some(pid) = node.parent {
                    let siblings = &state.document.nodes[pid].children;
                    if let Some(i) = siblings.iter().position(|&c| c == node_id) {
                        TreeVal::Single(
                            siblings
                                .iter()
                                .skip(i + 1)
                                .find(|cid| is_element(cid))
                                .copied(),
                        )
                    } else {
                        TreeVal::Single(None)
                    }
                } else {
                    TreeVal::Single(None)
                }
            }
            "previousElementSibling" => {
                if let Some(pid) = node.parent {
                    let siblings = &state.document.nodes[pid].children;
                    if let Some(i) = siblings.iter().position(|&c| c == node_id) {
                        TreeVal::Single(
                            siblings
                                .iter()
                                .take(i)
                                .rfind(|cid| is_element(cid))
                                .copied(),
                        )
                    } else {
                        TreeVal::Single(None)
                    }
                } else {
                    TreeVal::Single(None)
                }
            }
            "childNodes" => TreeVal::Many(node.children.clone()),
            "children" => TreeVal::Many(
                node.children
                    .iter()
                    .filter(|cid| is_element(cid))
                    .copied()
                    .collect(),
            ),
            "childElementCount" => {
                TreeVal::Count(node.children.iter().filter(|cid| is_element(cid)).count() as i32)
            }
            _ => TreeVal::None,
        }
    });

    match result {
        TreeVal::None => rv.set(v8::undefined(scope).into()),
        TreeVal::Single(None) => rv.set(v8::null(scope).into()),
        TreeVal::Single(Some(nid)) => rv.set(wrap_element_shallow(scope, nid).into()),
        TreeVal::Many(ids) => {
            let arr = v8::Array::new(scope, ids.len() as i32);
            for (i, &cid) in ids.iter().enumerate() {
                let el = wrap_element_shallow(scope, cid);
                arr.set_index(scope, i as u32, el.into());
            }
            rv.set(arr.into());
        }
        TreeVal::Count(n) => rv.set(v8::Integer::new(scope, n).into()),
    }
}

// ── wrap_element ─────────────────────────────────────────────────────────

fn wrap_element<'s>(scope: &mut v8::HandleScope<'s>, node_id: NodeId) -> v8::Local<'s, v8::Object> {
    if let Some(cached) = cache_get(scope, node_id) {
        return cached;
    }
    let obj = v8::Object::new(scope);
    cache_put(scope, node_id, obj);
    set_int(scope, obj, "__node_id__", node_id as i32);

    let (tag, id_attr, class_attr, node_type, text_content) = with_dom(|state| {
        if let Some(node) = state.document.nodes.get(node_id) {
            match &node.data {
                NodeData::Element(el) => (
                    el.tag_name.to_uppercase(),
                    el.attributes.get("id").cloned(),
                    el.attributes.get("class").cloned(),
                    1i32,
                    None,
                ),
                NodeData::Text(t) => (String::new(), None, None, 3i32, Some(t.content.clone())),
                _ => (String::new(), None, None, 0i32, None),
            }
        } else {
            (String::new(), None, None, 0i32, None)
        }
    });

    set_int(scope, obj, "nodeType", node_type);
    set_int(scope, obj, "ELEMENT_NODE", 1);
    set_int(scope, obj, "TEXT_NODE", 3);
    set_int(scope, obj, "COMMENT_NODE", 8);
    set_int(scope, obj, "DOCUMENT_NODE", 9);
    set_int(scope, obj, "DOCUMENT_FRAGMENT_NODE", 11);
    set_bool(scope, obj, "isConnected", true);
    set_str(scope, obj, "namespaceURI", "http://www.w3.org/1999/xhtml");
    if let Some(doc) = document_obj(scope) {
        let k = v8_str(scope, "ownerDocument");
        obj.set(scope, k.into(), doc.into());
    }
    if node_type == 1 {
        set_str(scope, obj, "tagName", &tag);
        set_str(scope, obj, "nodeName", &tag);
        let lc = tag.to_lowercase();
        set_str(scope, obj, "localName", &lc);
        // `<style>` and `<link>` elements expose a `sheet` with `cssRules`; many
        // sites read it to decide whether styles have loaded.
        if lc == "style" || lc == "link" {
            let sheet = v8::Object::new(scope);
            let rules = v8::Array::new(scope, 0);
            let css_rules_key = v8_str(scope, "cssRules");
            let _ = sheet.set(scope, css_rules_key.into(), rules.into());
            let sheet_key = v8_str(scope, "sheet");
            let _ = obj.set(scope, sheet_key.into(), sheet.into());
        }
        set_str(scope, obj, "id", &id_attr.unwrap_or_default());
        set_str(scope, obj, "className", &class_attr.unwrap_or_default());
        // textContent accessor (getter + setter)
        let tc_key = v8_str(scope, "textContent");
        let _ = obj.set_accessor_with_setter(
            scope,
            tc_key.into(),
            text_content_getter_cb,
            text_content_setter_cb,
        );

        // innerHTML / outerHTML accessors (getter + setter) so JS can mutate DOM
        let inner_key = v8_str(scope, "innerHTML");
        let _ = obj.set_accessor_with_setter(
            scope,
            inner_key.into(),
            inner_html_getter_cb,
            inner_html_setter_cb,
        );
        let outer_key = v8_str(scope, "outerHTML");
        let _ = obj.set_accessor(scope, outer_key.into(), outer_html_getter_cb);

        // Live tree accessors so wrapped nodes report current DOM state after
        // innerHTML/appendChild/insertBefore mutations.
        for &prop in &[
            "parentNode",
            "parentElement",
            "firstChild",
            "lastChild",
            "firstElementChild",
            "lastElementChild",
            "nextSibling",
            "previousSibling",
            "nextElementSibling",
            "previousElementSibling",
            "childNodes",
            "children",
            "childElementCount",
        ] {
            let k = v8_str(scope, prop);
            let _ = obj.set_accessor(scope, k.into(), tree_property_getter_cb);
        }
        let attr_key = v8_str(scope, "attributes");
        let _ = obj.set_accessor(scope, attr_key.into(), attributes_getter_cb);

        // Reflect element.src (used by script loaders) to the src attribute.
        let src_key = v8_str(scope, "src");
        let _ = obj.set_accessor_with_setter(
            scope,
            src_key.into(),
            element_src_getter_cb,
            element_src_setter_cb,
        );
        // Reflect common script/link attributes set as properties.
        let href_key = v8_str(scope, "href");
        let _ = obj.set_accessor_with_setter(
            scope,
            href_key.into(),
            element_href_getter_cb,
            element_href_setter_cb,
        );
        let charset_key = v8_str(scope, "charset");
        let _ = obj.set_accessor_with_setter(
            scope,
            charset_key.into(),
            element_attr_getter_cb,
            element_attr_setter_cb,
        );
        let async_key = v8_str(scope, "async");
        let _ = obj.set_accessor_with_setter(
            scope,
            async_key.into(),
            element_attr_getter_cb,
            element_attr_setter_cb,
        );
        let defer_key = v8_str(scope, "defer");
        let _ = obj.set_accessor_with_setter(
            scope,
            defer_key.into(),
            element_attr_getter_cb,
            element_attr_setter_cb,
        );

        // Layout properties (stubs - would come from actual layout engine)
        // clientWidth/Height: visible area including padding but not border/scrollbar
        set_int(scope, obj, "clientWidth", 0);
        set_int(scope, obj, "clientHeight", 0);
        set_int(scope, obj, "clientTop", 0);
        set_int(scope, obj, "clientLeft", 0);

        // offsetWidth/Height: layout width including padding, border, scrollbar
        set_int(scope, obj, "offsetWidth", 0);
        set_int(scope, obj, "offsetHeight", 0);
        set_int(scope, obj, "offsetTop", 0);
        set_int(scope, obj, "offsetLeft", 0);
        set_int(scope, obj, "offsetParent", 0); // null (0 cast to pointer)

        // scrollWidth/Height: total scrollable area
        set_int(scope, obj, "scrollWidth", 0);
        set_int(scope, obj, "scrollHeight", 0);
        set_int(scope, obj, "scrollTop", 0);
        set_int(scope, obj, "scrollLeft", 0);

        // getBoundingClientRect() is a method above, but we also set initial values
        // These are relative to the viewport
        set_int(scope, obj, "boundingClientTop", 0);
        set_int(scope, obj, "boundingClientLeft", 0);
        set_int(scope, obj, "boundingClientWidth", 0);
        set_int(scope, obj, "boundingClientHeight", 0);
    } else if node_type == 3 {
        if let Some(t) = text_content {
            set_str(scope, obj, "textContent", &t);
            set_str(scope, obj, "nodeValue", &t);
            set_str(scope, obj, "data", &t);
        }
    }

    // methods
    set_fn(scope, obj, "appendChild", append_child_cb);
    set_fn(scope, obj, "append", append_cb);
    set_fn(scope, obj, "prepend", prepend_cb);
    set_fn(scope, obj, "removeChild", remove_child_cb);
    set_fn(scope, obj, "insertBefore", insert_before_cb);
    set_fn(scope, obj, "replaceChild", replace_child_cb);
    set_fn(scope, obj, "cloneNode", clone_node_cb);
    set_fn(scope, obj, "remove", remove_cb);
    set_fn(scope, obj, "setAttribute", set_attribute_cb);
    set_fn(scope, obj, "getAttribute", get_attribute_cb);
    set_fn(scope, obj, "hasAttribute", has_attribute_cb);
    set_fn(scope, obj, "removeAttribute", remove_attribute_cb);
    set_fn(scope, obj, "addEventListener", add_event_listener_cb);
    set_fn(scope, obj, "removeEventListener", remove_event_listener_cb);
    set_fn(scope, obj, "dispatchEvent", dispatch_event_cb);
    set_fn(scope, obj, "querySelector", element_query_selector_cb);
    set_fn(
        scope,
        obj,
        "querySelectorAll",
        element_query_selector_all_cb,
    );
    set_fn(
        scope,
        obj,
        "getElementsByTagName",
        element_get_elements_by_tag_name_cb,
    );
    set_fn(
        scope,
        obj,
        "getElementsByClassName",
        element_get_elements_by_class_name_cb,
    );
    set_fn(
        scope,
        obj,
        "getBoundingClientRect",
        get_bounding_client_rect_cb,
    );
    set_fn(scope, obj, "focus", focus_cb);
    set_fn(scope, obj, "blur", blur_cb);
    set_fn(scope, obj, "click", click_cb);
    set_fn(scope, obj, "scrollIntoView", scroll_into_view_cb);
    set_fn(scope, obj, "contains", contains_cb);
    set_fn(scope, obj, "matches", matches_cb);
    set_fn(scope, obj, "closest", closest_cb);
    set_fn(scope, obj, "insertAdjacentHTML", insert_adjacent_html_cb);
    set_fn(
        scope,
        obj,
        "insertAdjacentElement",
        insert_adjacent_element_cb,
    );
    set_fn(scope, obj, "normalize", normalize_cb);
    set_fn(scope, obj, "before", before_cb);
    set_fn(scope, obj, "after", after_cb);
    set_fn(scope, obj, "replaceWith", replace_with_cb);
    set_fn(scope, obj, "replaceChildren", replace_children_cb);

    // style - CSSStyleDeclaration with inline style manipulation
    let style = v8::Object::new(scope);
    // Store reference to owner element
    set_int(scope, style, "__element__", node_id as i32);
    set_fn(scope, style, "setProperty", style_set_property_cb);
    set_fn(
        scope,
        style,
        "getPropertyValue",
        style_get_property_value_cb,
    );
    set_fn(scope, style, "removeProperty", style_remove_property_cb);
    // CSS text property
    set_str(scope, style, "cssText", "");
    let style_key = v8_str(scope, "style");
    obj.set(scope, style_key.into(), style.into());

    // classList
    let classlist = v8::Object::new(scope);
    // Store reference to owner element for classList methods
    set_int(scope, classlist, "__element__", node_id as i32);
    set_fn(scope, classlist, "add", classlist_add_cb);
    set_fn(scope, classlist, "remove", classlist_remove_cb);
    set_fn(scope, classlist, "toggle", classlist_toggle_cb);
    set_fn(scope, classlist, "contains", classlist_contains_cb);
    set_fn(scope, classlist, "replace", classlist_replace_cb);
    let cl_key = v8_str(scope, "classList");
    obj.set(scope, cl_key.into(), classlist.into());

    // canvas.getContext() - returns a Canvas 2D rendering context
    fn canvas_get_context_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        // Check context type - only support "2d" for now
        let context_type = args
            .get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default()
            .to_lowercase();

        if context_type != "2d" {
            // Return null for unsupported context types (webgl, etc.)
            rv.set_null();
            return;
        }

        // Get the canvas element this context belongs to
        let this = args.this();
        let canvas_id = get_prop(scope, this, "__node_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId);

        // Create the CanvasRenderingContext2D object
        let ctx = v8::Object::new(scope);

        // Store reference to canvas element
        if let Some(cid) = canvas_id {
            set_int(scope, ctx, "__canvas_id__", cid as i32);
        }

        // Canvas state properties (default values)
        set_str(scope, ctx, "fillStyle", "#000000");
        set_str(scope, ctx, "strokeStyle", "#000000");
        set_str(scope, ctx, "lineWidth", "1");
        set_str(scope, ctx, "lineCap", "butt");
        set_str(scope, ctx, "lineJoin", "miter");
        set_str(scope, ctx, "miterLimit", "10");
        set_str(scope, ctx, "globalAlpha", "1");
        set_str(scope, ctx, "globalCompositeOperation", "source-over");
        set_str(scope, ctx, "font", "10px sans-serif");
        set_str(scope, ctx, "textAlign", "start");
        set_str(scope, ctx, "textBaseline", "alphabetic");
        set_str(scope, ctx, "direction", "inherit");
        set_str(scope, ctx, "shadowColor", "rgba(0, 0, 0, 0)");
        set_str(scope, ctx, "shadowBlur", "0");
        set_str(scope, ctx, "shadowOffsetX", "0");
        set_str(scope, ctx, "shadowOffsetY", "0");

        // Drawing methods
        set_fn(scope, ctx, "fillRect", canvas_fill_rect_cb);
        set_fn(scope, ctx, "strokeRect", canvas_stroke_rect_cb);
        set_fn(scope, ctx, "clearRect", canvas_clear_rect_cb);
        set_fn(scope, ctx, "fillText", canvas_fill_text_cb);
        set_fn(scope, ctx, "strokeText", canvas_stroke_text_cb);
        set_fn(scope, ctx, "measureText", canvas_measure_text_cb);

        // Path methods
        set_fn(scope, ctx, "beginPath", canvas_begin_path_cb);
        set_fn(scope, ctx, "closePath", canvas_close_path_cb);
        set_fn(scope, ctx, "moveTo", canvas_move_to_cb);
        set_fn(scope, ctx, "lineTo", canvas_line_to_cb);
        set_fn(scope, ctx, "bezierCurveTo", canvas_bezier_curve_to_cb);
        set_fn(scope, ctx, "quadraticCurveTo", canvas_quadratic_curve_to_cb);
        set_fn(scope, ctx, "arc", canvas_arc_cb);
        set_fn(scope, ctx, "rect", canvas_rect_cb);
        set_fn(scope, ctx, "fill", canvas_fill_cb);
        set_fn(scope, ctx, "stroke", canvas_stroke_cb);
        set_fn(scope, ctx, "clip", canvas_clip_cb);

        // State methods
        set_fn(scope, ctx, "save", canvas_save_cb);
        set_fn(scope, ctx, "restore", canvas_restore_cb);

        // Transform methods
        set_fn(scope, ctx, "scale", canvas_scale_cb);
        set_fn(scope, ctx, "rotate", canvas_rotate_cb);
        set_fn(scope, ctx, "translate", canvas_translate_cb);
        set_fn(scope, ctx, "transform", canvas_transform_cb);
        set_fn(scope, ctx, "setTransform", canvas_set_transform_cb);
        set_fn(scope, ctx, "resetTransform", canvas_reset_transform_cb);

        // Image methods
        set_fn(scope, ctx, "drawImage", canvas_draw_image_cb);
        set_fn(scope, ctx, "createImageData", canvas_create_image_data_cb);
        set_fn(scope, ctx, "getImageData", canvas_get_image_data_cb);
        set_fn(scope, ctx, "putImageData", canvas_put_image_data_cb);

        // Gradient/Pattern methods
        set_fn(
            scope,
            ctx,
            "createLinearGradient",
            canvas_create_linear_gradient_cb,
        );
        set_fn(
            scope,
            ctx,
            "createRadialGradient",
            canvas_create_radial_gradient_cb,
        );
        set_fn(scope, ctx, "createPattern", canvas_create_pattern_cb);

        // Getter methods
        set_fn(scope, ctx, "getTransform", canvas_get_transform_cb);
        set_fn(scope, ctx, "isPointInPath", canvas_is_point_in_path_cb);
        set_fn(scope, ctx, "isPointInStroke", canvas_is_point_in_stroke_cb);

        rv.set(ctx.into());
    }

    // Canvas 2D Context State Storage
    thread_local! {
        static CANVAS_STATE: RefCell<HashMap<NodeId, CanvasContextState>> = RefCell::new(HashMap::new());
    }

    #[derive(Debug, Clone)]
    struct CanvasContextState {
        // Transformation matrix (a, b, c, d, e, f for 2x3 matrix)
        transform: [f64; 6],
        // Path operations stack
        path: Vec<PathCommand>,
        // Style properties
        fill_style: String,
        stroke_style: String,
        line_width: f64,
        global_alpha: f64,
    }

    #[derive(Debug, Clone)]
    enum PathCommand {
        MoveTo(f64, f64),
        LineTo(f64, f64),
        Arc(f64, f64, f64, f64, f64, bool),
        Rect(f64, f64, f64, f64),
        ClosePath,
    }

    impl Default for CanvasContextState {
        fn default() -> Self {
            CanvasContextState {
                transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], // Identity matrix
                path: Vec::new(),
                fill_style: "#000000".to_string(),
                stroke_style: "#000000".to_string(),
                line_width: 1.0,
                global_alpha: 1.0,
            }
        }
    }

    fn get_canvas_state(canvas_id: NodeId) -> CanvasContextState {
        CANVAS_STATE.with(|state| state.borrow().get(&canvas_id).cloned().unwrap_or_default())
    }

    fn set_canvas_state(canvas_id: NodeId, ctx_state: CanvasContextState) {
        CANVAS_STATE.with(|state| {
            state.borrow_mut().insert(canvas_id, ctx_state);
        })
    }

    // ── Canvas 2D Context Drawing Callbacks ───────────────────────────────

    fn canvas_fill_rect_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let _x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _width = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _height = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let state = get_canvas_state(canvas_id);
            eprintln!(
                "Canvas fillRect: ({}, {}, {}, {}) with fillStyle={}",
                _x, _y, _width, _height, state.fill_style
            );
            // In a real implementation, this would draw to an actual canvas surface
        }
    }

    fn canvas_stroke_rect_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let _x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _width = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _height = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let state = get_canvas_state(canvas_id);
            eprintln!(
                "Canvas strokeRect: ({}, {}, {}, {}) with strokeStyle={}",
                _x, _y, _width, _height, state.stroke_style
            );
        }
    }

    fn canvas_clear_rect_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let _x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _width = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _height = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        eprintln!(
            "Canvas clearRect: ({}, {}, {}, {})",
            _x, _y, _width, _height
        );
    }

    fn canvas_fill_text_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let text = args
            .get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();
        let _x = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        eprintln!("Canvas fillText: \"{}\" at ({}, {})", text, _x, _y);
    }

    fn canvas_stroke_text_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let text = args
            .get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();
        let _x = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        eprintln!("Canvas strokeText: \"{}\" at ({}, {})", text, _x, _y);
    }

    fn canvas_measure_text_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let text = args
            .get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();

        // Create a TextMetrics object with estimated width
        // In a real implementation, this would measure actual text
        let metrics = v8::Object::new(scope);
        let estimated_width = text.len() as f64 * 8.0; // Rough estimate: ~8px per char
        let width_num = v8::Number::new(scope, estimated_width);
        let width_key = v8_str(scope, "width");
        metrics.set(scope, width_key.into(), width_num.into());

        rv.set(metrics.into());
    }

    // ── Canvas 2D Context Path Callbacks ──────────────────────────────────

    fn canvas_begin_path_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.path.clear();
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas beginPath");
        }
    }

    fn canvas_close_path_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.path.push(PathCommand::ClosePath);
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas closePath");
        }
    }

    fn canvas_move_to_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.path.push(PathCommand::MoveTo(x, y));
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas moveTo: ({}, {})", x, y);
        }
    }

    fn canvas_line_to_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.path.push(PathCommand::LineTo(x, y));
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas lineTo: ({}, {})", x, y);
        }
    }

    fn canvas_bezier_curve_to_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let _cp1x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _cp1y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _cp2x = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _cp2y = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _x = args
            .get(4)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(5)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        eprintln!(
            "Canvas bezierCurveTo: cp=({}, {}), ({}, {}), end=({}, {})",
            _cp1x, _cp1y, _cp2x, _cp2y, _x, _y
        );
    }

    fn canvas_quadratic_curve_to_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let _cpx = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _cpy = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _x = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        eprintln!(
            "Canvas quadraticCurveTo: cp=({}, {}), end=({}, {})",
            _cpx, _cpy, _x, _y
        );
    }

    fn canvas_arc_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let radius = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let start_angle = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let end_angle = args
            .get(4)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let anticlockwise = args.get(5).is_true();

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.path.push(PathCommand::Arc(
                x,
                y,
                radius,
                start_angle,
                end_angle,
                anticlockwise,
            ));
            set_canvas_state(canvas_id, state);
            eprintln!(
                "Canvas arc: center=({}, {}), r={}, angles=({}, {}), ccw={}",
                x, y, radius, start_angle, end_angle, anticlockwise
            );
        }
    }

    fn canvas_rect_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let width = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let height = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.path.push(PathCommand::Rect(x, y, width, height));
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas rect: ({}, {}, {}, {})", x, y, width, height);
        }
    }

    fn canvas_fill_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let state = get_canvas_state(canvas_id);
            eprintln!("Canvas fill: {} path commands", state.path.len());
        }
    }

    fn canvas_stroke_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let state = get_canvas_state(canvas_id);
            eprintln!("Canvas stroke: {} path commands", state.path.len());
        }
    }

    fn canvas_clip_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        eprintln!("Canvas clip");
    }

    // ── Canvas 2D Context State Callbacks ─────────────────────────────────

    fn canvas_save_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let state = get_canvas_state(canvas_id);
            // In a full implementation, this would push state onto a stack
            eprintln!("Canvas save: current transform={:?}", state.transform);
        }
    }

    fn canvas_restore_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            // In a full implementation, this would pop state from a stack
            eprintln!("Canvas restore");
        }
    }

    // ── Canvas 2D Context Transform Callbacks ───────────────────────────────

    fn canvas_scale_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(1.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(1.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.transform[0] *= x;
            state.transform[3] *= y;
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas scale: ({}, {})", x, y);
        }
    }

    fn canvas_rotate_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let angle = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            let cos = angle.cos();
            let sin = angle.sin();
            // Apply rotation matrix multiplication
            let a = state.transform[0];
            let b = state.transform[1];
            let c = state.transform[2];
            let d = state.transform[3];
            state.transform[0] = a * cos + c * sin;
            state.transform[1] = b * cos + d * sin;
            state.transform[2] = c * cos - a * sin;
            state.transform[3] = d * cos - b * sin;
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas rotate: {} radians", angle);
        }
    }

    fn canvas_translate_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.transform[4] += x;
            state.transform[5] += y;
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas translate: ({}, {})", x, y);
        }
    }

    fn canvas_transform_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let a = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(1.0);
        let b = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let c = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let d = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(1.0);
        let e = args
            .get(4)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let f = args
            .get(5)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            // Matrix multiplication
            let old = state.transform;
            state.transform = [
                old[0] * a + old[2] * b,
                old[1] * a + old[3] * b,
                old[0] * c + old[2] * d,
                old[1] * c + old[3] * d,
                old[0] * e + old[2] * f + old[4],
                old[1] * e + old[3] * f + old[5],
            ];
            set_canvas_state(canvas_id, state);
            eprintln!(
                "Canvas transform: matrix ({}, {}, {}, {}, {}, {})",
                a, b, c, d, e, f
            );
        }
    }

    fn canvas_set_transform_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let a = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(1.0);
        let b = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let c = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let d = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(1.0);
        let e = args
            .get(4)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let f = args
            .get(5)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.transform = [a, b, c, d, e, f];
            set_canvas_state(canvas_id, state);
            eprintln!(
                "Canvas setTransform: matrix ({}, {}, {}, {}, {}, {})",
                a, b, c, d, e, f
            );
        }
    }

    fn canvas_reset_transform_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let mut state = get_canvas_state(canvas_id);
            state.transform = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
            set_canvas_state(canvas_id, state);
            eprintln!("Canvas resetTransform");
        }
    }

    fn canvas_get_transform_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Some(canvas_id) = get_prop(scope, this, "__canvas_id__")
            .and_then(|v| v.int32_value(scope))
            .map(|n| n as NodeId)
        {
            let state = get_canvas_state(canvas_id);
            // Return DOMMatrix or object with transform values
            let obj = v8::Object::new(scope);
            let arr = v8::Array::new(scope, 6);
            for (i, &val) in state.transform.iter().enumerate() {
                let num = v8::Number::new(scope, val);
                arr.set_index(scope, i as u32, num.into());
            }
            let m_key = v8_str(scope, "m");
            obj.set(scope, m_key.into(), arr.into());
            rv.set(obj.into());
        }
    }

    // ── Canvas 2D Context Image Callbacks ──────────────────────────────────

    fn canvas_draw_image_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        // drawImage has 3 variants:
        // drawImage(image, dx, dy)
        // drawImage(image, dx, dy, dWidth, dHeight)
        // drawImage(image, sx, sy, sWidth, sHeight, dx, dy, dWidth, dHeight)
        eprintln!("Canvas drawImage");
    }

    fn canvas_create_image_data_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let width = args.get(0).int32_value(scope).unwrap_or(0);
        let height = args.get(1).int32_value(scope).unwrap_or(0);

        // Create ImageData object
        let obj = v8::Object::new(scope);
        set_int(scope, obj, "width", width);
        set_int(scope, obj, "height", height);

        // Create Uint8ClampedArray for pixel data (RGBA per pixel)
        let data_len = (width * height * 4) as usize;
        let data_arr = v8::Array::new(scope, data_len as i32);
        let zero = v8::Integer::new(scope, 0);
        for i in 0..data_len {
            data_arr.set_index(scope, i as u32, zero.into());
        }

        let data_key = v8_str(scope, "data");
        obj.set(scope, data_key.into(), data_arr.into());
        rv.set(obj.into());
    }

    fn canvas_get_image_data_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let _sx = args.get(0).int32_value(scope).unwrap_or(0);
        let _sy = args.get(1).int32_value(scope).unwrap_or(0);
        let width = args.get(2).int32_value(scope).unwrap_or(0);
        let height = args.get(3).int32_value(scope).unwrap_or(0);

        // Create ImageData object (returns transparent black pixels for now)
        let obj = v8::Object::new(scope);
        set_int(scope, obj, "width", width);
        set_int(scope, obj, "height", height);

        let data_len = (width * height * 4) as usize;
        let data_arr = v8::Array::new(scope, data_len as i32);
        let zero = v8::Integer::new(scope, 0);
        for i in 0..data_len {
            data_arr.set_index(scope, i as u32, zero.into());
        }

        let data_key = v8_str(scope, "data");
        obj.set(scope, data_key.into(), data_arr.into());
        rv.set(obj.into());
    }

    fn canvas_put_image_data_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        eprintln!("Canvas putImageData");
    }

    // ── Canvas 2D Context Gradient/Pattern Callbacks ─────────────────────────

    fn canvas_create_linear_gradient_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let _x0 = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y0 = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _x1 = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y1 = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        // Create CanvasGradient object
        let obj = v8::Object::new(scope);
        set_str(scope, obj, "__type__", "linear");
        set_fn(
            scope,
            obj,
            "addColorStop",
            canvas_gradient_add_color_stop_cb,
        );
        rv.set(obj.into());
    }

    fn canvas_create_radial_gradient_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let _x0 = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y0 = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _r0 = args
            .get(2)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _x1 = args
            .get(3)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y1 = args
            .get(4)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _r1 = args
            .get(5)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        // Create CanvasGradient object
        let obj = v8::Object::new(scope);
        set_str(scope, obj, "__type__", "radial");
        set_fn(
            scope,
            obj,
            "addColorStop",
            canvas_gradient_add_color_stop_cb,
        );
        rv.set(obj.into());
    }

    fn canvas_create_pattern_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        // Create CanvasPattern object (stub)
        let obj = v8::Object::new(scope);
        set_str(scope, obj, "__type__", "pattern");
        rv.set(obj.into());
    }

    fn canvas_gradient_add_color_stop_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let _offset = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _color = args
            .get(1)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();
        eprintln!(
            "CanvasGradient addColorStop: offset={}, color={}",
            _offset, _color
        );
    }

    // ── Canvas 2D Context Hit Testing Callbacks ─────────────────────────────

    fn canvas_is_point_in_path_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let _x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        // Stub: always return false
        rv.set_bool(false);
    }

    fn canvas_is_point_in_stroke_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let _x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let _y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);

        // Stub: always return false
        rv.set_bool(false);
    }
    // Add getContext only for canvas elements
    with_dom(|state| {
        if let Some(node) = state.document.nodes.get(node_id) {
            if let NodeData::Element(ref e) = node.data {
                if e.tag_name == "canvas" {
                    set_fn(scope, obj, "getContext", canvas_get_context_cb);
                }
            }
        }
    });

    // dataset - data-* attributes as camelCase properties
    let ds = v8::Object::new(scope);
    set_int(scope, ds, "__element__", node_id as i32);

    // Populate with existing data-* attributes
    with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[node_id].data {
            for (attr_name, attr_value) in &el.attributes {
                if attr_name.starts_with("data-") {
                    // Convert data-foo-bar to fooBar (camelCase)
                    let camel_name = attr_name[5..] // Remove "data-" prefix
                        .split('-')
                        .enumerate()
                        .map(|(i, part)| {
                            if i == 0 {
                                part.to_string()
                            } else {
                                let mut chars = part.chars();
                                match chars.next() {
                                    Some(first) => {
                                        first.to_uppercase().collect::<String>()
                                            + &chars.as_str().to_lowercase()
                                    }
                                    None => String::new(),
                                }
                            }
                        })
                        .collect::<String>();
                    if !camel_name.is_empty() {
                        set_str(scope, ds, &camel_name, attr_value);
                    }
                }
            }
        }
    });

    let ds_key = v8_str(scope, "dataset");
    obj.set(scope, ds_key.into(), ds.into());

    // Tree properties are live accessors so that mutations (innerHTML,
    // appendChild, etc.) are visible to JS without re-wrapping the parent node.
    for &prop in &[
        "parentNode",
        "parentElement",
        "firstChild",
        "lastChild",
        "firstElementChild",
        "lastElementChild",
        "nextSibling",
        "previousSibling",
        "nextElementSibling",
        "previousElementSibling",
        "childNodes",
        "children",
        "childElementCount",
    ] {
        let k = v8_str(scope, prop);
        let _ = obj.set_accessor(scope, k.into(), tree_property_getter_cb);
    }

    obj
}

/// Shallow wrap: identity + methods, but no tree references (to avoid recursion).
/// When JS reads `elem.parentNode.appendChild(...)`, we'd like appendChild to
/// work. So we include the mutation methods but NOT parentNode/childNodes/etc.
fn wrap_element_shallow<'s>(
    scope: &mut v8::HandleScope<'s>,
    node_id: NodeId,
) -> v8::Local<'s, v8::Object> {
    if let Some(cached) = cache_get(scope, node_id) {
        return cached;
    }
    let obj = v8::Object::new(scope);
    cache_put(scope, node_id, obj);
    set_int(scope, obj, "__node_id__", node_id as i32);

    let (tag, id_attr, class_attr, node_type, text_content) = with_dom(|state| {
        if let Some(node) = state.document.nodes.get(node_id) {
            match &node.data {
                NodeData::Element(el) => (
                    el.tag_name.to_uppercase(),
                    el.attributes.get("id").cloned(),
                    el.attributes.get("class").cloned(),
                    1i32,
                    None,
                ),
                NodeData::Text(t) => (String::new(), None, None, 3i32, Some(t.content.clone())),
                _ => (String::new(), None, None, 0i32, None),
            }
        } else {
            (String::new(), None, None, 0i32, None)
        }
    });
    set_int(scope, obj, "nodeType", node_type);
    if node_type == 1 {
        set_str(scope, obj, "tagName", &tag);
        set_str(scope, obj, "nodeName", &tag);
        set_str(scope, obj, "id", &id_attr.unwrap_or_default());
        set_str(scope, obj, "className", &class_attr.unwrap_or_default());
        // textContent accessor (getter + setter)
        let tc_key = v8_str(scope, "textContent");
        let _ = obj.set_accessor_with_setter(
            scope,
            tc_key.into(),
            text_content_getter_cb,
            text_content_setter_cb,
        );

        // innerHTML / outerHTML accessors (getter + setter) so JS can mutate DOM
        let inner_key = v8_str(scope, "innerHTML");
        let _ = obj.set_accessor_with_setter(
            scope,
            inner_key.into(),
            inner_html_getter_cb,
            inner_html_setter_cb,
        );
        let outer_key = v8_str(scope, "outerHTML");
        let _ = obj.set_accessor(scope, outer_key.into(), outer_html_getter_cb);

        // Live tree accessors so wrapped nodes report current DOM state after
        // innerHTML/appendChild/insertBefore mutations.
        for &prop in &[
            "parentNode",
            "parentElement",
            "firstChild",
            "lastChild",
            "firstElementChild",
            "lastElementChild",
            "nextSibling",
            "previousSibling",
            "nextElementSibling",
            "previousElementSibling",
            "childNodes",
            "children",
            "childElementCount",
        ] {
            let k = v8_str(scope, prop);
            let _ = obj.set_accessor(scope, k.into(), tree_property_getter_cb);
        }
        let attr_key = v8_str(scope, "attributes");
        let _ = obj.set_accessor(scope, attr_key.into(), attributes_getter_cb);

        // `<style>` and `<link>` elements expose a `sheet` with `cssRules`.
        let lc = tag.to_lowercase();
        if lc == "style" || lc == "link" {
            let sheet = v8::Object::new(scope);
            let rules = v8::Array::new(scope, 0);
            let css_rules_key = v8_str(scope, "cssRules");
            let _ = sheet.set(scope, css_rules_key.into(), rules.into());
            let sheet_key = v8_str(scope, "sheet");
            let _ = obj.set(scope, sheet_key.into(), sheet.into());
        }

        // Reflect element.src (used by script loaders) to the src attribute.
        let src_key = v8_str(scope, "src");
        let _ = obj.set_accessor_with_setter(
            scope,
            src_key.into(),
            element_src_getter_cb,
            element_src_setter_cb,
        );
        // Reflect common script/link attributes set as properties.
        let href_key = v8_str(scope, "href");
        let _ = obj.set_accessor_with_setter(
            scope,
            href_key.into(),
            element_href_getter_cb,
            element_href_setter_cb,
        );
        let charset_key = v8_str(scope, "charset");
        let _ = obj.set_accessor_with_setter(
            scope,
            charset_key.into(),
            element_attr_getter_cb,
            element_attr_setter_cb,
        );
        let async_key = v8_str(scope, "async");
        let _ = obj.set_accessor_with_setter(
            scope,
            async_key.into(),
            element_attr_getter_cb,
            element_attr_setter_cb,
        );
        let defer_key = v8_str(scope, "defer");
        let _ = obj.set_accessor_with_setter(
            scope,
            defer_key.into(),
            element_attr_getter_cb,
            element_attr_setter_cb,
        );

        // Layout properties (stubs - would come from actual layout engine)
        // clientWidth/Height: visible area including padding but not border/scrollbar
        set_int(scope, obj, "clientWidth", 0);
        set_int(scope, obj, "clientHeight", 0);
        set_int(scope, obj, "clientTop", 0);
        set_int(scope, obj, "clientLeft", 0);

        // offsetWidth/Height: layout width including padding, border, scrollbar
        set_int(scope, obj, "offsetWidth", 0);
        set_int(scope, obj, "offsetHeight", 0);
        set_int(scope, obj, "offsetTop", 0);
        set_int(scope, obj, "offsetLeft", 0);
        set_int(scope, obj, "offsetParent", 0); // null (0 cast to pointer)

        // scrollWidth/Height: total scrollable area
        set_int(scope, obj, "scrollWidth", 0);
        set_int(scope, obj, "scrollHeight", 0);
        set_int(scope, obj, "scrollTop", 0);
        set_int(scope, obj, "scrollLeft", 0);

        // getBoundingClientRect() is a method above, but we also set initial values
        // These are relative to the viewport
        set_int(scope, obj, "boundingClientTop", 0);
        set_int(scope, obj, "boundingClientLeft", 0);
        set_int(scope, obj, "boundingClientWidth", 0);
        set_int(scope, obj, "boundingClientHeight", 0);
    } else if node_type == 3 {
        if let Some(t) = text_content {
            set_str(scope, obj, "textContent", &t);
            set_str(scope, obj, "nodeValue", &t);
        }
    }
    // methods
    set_fn(scope, obj, "appendChild", append_child_cb);
    set_fn(scope, obj, "append", append_cb);
    set_fn(scope, obj, "prepend", prepend_cb);
    set_fn(scope, obj, "removeChild", remove_child_cb);
    set_fn(scope, obj, "insertBefore", insert_before_cb);
    set_fn(scope, obj, "replaceChild", replace_child_cb);
    set_fn(scope, obj, "cloneNode", clone_node_cb);
    set_fn(scope, obj, "remove", remove_cb);
    set_fn(scope, obj, "setAttribute", set_attribute_cb);
    set_fn(scope, obj, "getAttribute", get_attribute_cb);
    set_fn(scope, obj, "hasAttribute", has_attribute_cb);
    set_fn(scope, obj, "removeAttribute", remove_attribute_cb);
    set_fn(scope, obj, "addEventListener", add_event_listener_cb);
    set_fn(scope, obj, "removeEventListener", remove_event_listener_cb);
    set_fn(scope, obj, "dispatchEvent", dispatch_event_cb);
    set_fn(scope, obj, "querySelector", element_query_selector_cb);
    set_fn(
        scope,
        obj,
        "querySelectorAll",
        element_query_selector_all_cb,
    );
    set_fn(
        scope,
        obj,
        "getElementsByTagName",
        element_get_elements_by_tag_name_cb,
    );
    set_fn(
        scope,
        obj,
        "getElementsByClassName",
        element_get_elements_by_class_name_cb,
    );
    set_fn(
        scope,
        obj,
        "getBoundingClientRect",
        get_bounding_client_rect_cb,
    );
    set_fn(scope, obj, "focus", focus_cb);
    set_fn(scope, obj, "blur", blur_cb);
    set_fn(scope, obj, "click", click_cb);
    set_fn(scope, obj, "scrollIntoView", scroll_into_view_cb);
    set_fn(scope, obj, "contains", contains_cb);
    set_fn(scope, obj, "matches", matches_cb);
    set_fn(scope, obj, "closest", closest_cb);
    set_fn(scope, obj, "insertAdjacentHTML", insert_adjacent_html_cb);
    set_fn(
        scope,
        obj,
        "insertAdjacentElement",
        insert_adjacent_element_cb,
    );
    set_fn(scope, obj, "normalize", normalize_cb);
    set_fn(scope, obj, "before", before_cb);
    set_fn(scope, obj, "after", after_cb);
    set_fn(scope, obj, "replaceWith", replace_with_cb);
    set_fn(scope, obj, "replaceChildren", replace_children_cb);

    // style - CSSStyleDeclaration with inline style manipulation
    let style = v8::Object::new(scope);
    set_int(scope, style, "__element__", node_id as i32);
    set_fn(scope, style, "setProperty", style_set_property_cb);
    set_fn(
        scope,
        style,
        "getPropertyValue",
        style_get_property_value_cb,
    );
    set_fn(scope, style, "removeProperty", style_remove_property_cb);
    set_str(scope, style, "cssText", "");
    let style_key = v8_str(scope, "style");
    obj.set(scope, style_key.into(), style.into());

    // classList
    let classlist = v8::Object::new(scope);
    set_int(scope, classlist, "__element__", node_id as i32);
    set_fn(scope, classlist, "add", classlist_add_cb);
    set_fn(scope, classlist, "remove", classlist_remove_cb);
    set_fn(scope, classlist, "toggle", classlist_toggle_cb);
    set_fn(scope, classlist, "contains", classlist_contains_cb);
    set_fn(scope, classlist, "replace", classlist_replace_cb);
    let cl_key = v8_str(scope, "classList");
    obj.set(scope, cl_key.into(), classlist.into());

    // dataset - data-* attributes as camelCase properties
    let ds = v8::Object::new(scope);
    set_int(scope, ds, "__element__", node_id as i32);
    with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[node_id].data {
            for (attr_name, attr_value) in &el.attributes {
                if attr_name.starts_with("data-") {
                    let camel_name = attr_name[5..]
                        .split('-')
                        .enumerate()
                        .map(|(i, part)| {
                            if i == 0 {
                                part.to_string()
                            } else {
                                let mut chars = part.chars();
                                match chars.next() {
                                    Some(first) => {
                                        first.to_uppercase().collect::<String>()
                                            + &chars.as_str().to_lowercase()
                                    }
                                    None => String::new(),
                                }
                            }
                        })
                        .collect::<String>();
                    if !camel_name.is_empty() {
                        set_str(scope, ds, &camel_name, attr_value);
                    }
                }
            }
        }
    });
    let ds_key = v8_str(scope, "dataset");
    obj.set(scope, ds_key.into(), ds.into());

    obj
}

// ── mutation callbacks ───────────────────────────────────────────────────

fn append_child_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let parent = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let child_val = args.get(0);
    let child = match extract_node_id(scope, child_val) {
        Some(n) => n,
        None => {
            rv.set(child_val);
            return;
        }
    };
    with_dom(|state| {
        if let Some(node) = state.document.nodes.get(child) {
            if let Some(old_parent) = node.parent {
                state.document.nodes[old_parent]
                    .children
                    .retain(|&c| c != child);
            }
        }
        state.document.nodes[child].parent = Some(parent);
        state.document.nodes[parent].children.push(child);
    });
    execute_appended_script_if_needed(scope, child_val, child);
    rv.set(child_val);
}

/// Element.append(...nodesOrStrings) — like appendChild but accepts text strings.
fn append_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let parent = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    for i in 0..args.length() {
        let arg = args.get(i);
        if arg.is_string() {
            let text = arg.to_rust_string_lossy(scope);
            let text_id = with_dom(|state| {
                let id = state.document.nodes.len();
                state.document.nodes.push(Node {
                    id,
                    parent: Some(parent),
                    children: Vec::new(),
                    data: NodeData::Text(TextData { content: text }),
                });
                id
            });
            with_dom(|state| {
                state.document.nodes[parent].children.push(text_id);
            });
            let _ = wrap_element(scope, text_id);
        } else if let Ok(node) = v8::Local::<v8::Object>::try_from(arg) {
            if let Some(child) = extract_node_id(scope, node.into()) {
                with_dom(|state| {
                    if let Some(old_parent) = state.document.nodes[child].parent {
                        state.document.nodes[old_parent]
                            .children
                            .retain(|&c| c != child);
                    }
                    state.document.nodes[child].parent = Some(parent);
                    state.document.nodes[parent].children.push(child);
                });
                execute_appended_script_if_needed(scope, arg, child);
            }
        }
    }
}

/// Element.prepend(...nodesOrStrings) — inserts nodes/strings before the first child.
fn prepend_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let parent = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    // Collect new node ids in order, then insert them before existing children.
    let mut new_children: Vec<NodeId> = Vec::new();
    for i in 0..args.length() {
        let arg = args.get(i);
        if arg.is_string() {
            let text = arg.to_rust_string_lossy(scope);
            let text_id = with_dom(|state| {
                let id = state.document.nodes.len();
                state.document.nodes.push(Node {
                    id,
                    parent: Some(parent),
                    children: Vec::new(),
                    data: NodeData::Text(TextData { content: text }),
                });
                id
            });
            new_children.push(text_id);
            let _ = wrap_element(scope, text_id);
        } else if let Ok(node) = v8::Local::<v8::Object>::try_from(arg) {
            if let Some(child) = extract_node_id(scope, node.into()) {
                with_dom(|state| {
                    if let Some(old_parent) = state.document.nodes[child].parent {
                        state.document.nodes[old_parent]
                            .children
                            .retain(|&c| c != child);
                    }
                    state.document.nodes[child].parent = Some(parent);
                });
                new_children.push(child);
                execute_appended_script_if_needed(scope, arg, child);
            }
        }
    }
    if !new_children.is_empty() {
        with_dom(|state| {
            let mut old = std::mem::take(&mut state.document.nodes[parent].children);
            state.document.nodes[parent]
                .children
                .reserve(new_children.len() + old.len());
            state.document.nodes[parent]
                .children
                .extend_from_slice(&new_children);
            state.document.nodes[parent]
                .children
                .extend_from_slice(&old);
            // Existing children already have the correct parent pointer; newly created
            // text nodes and reparented nodes were set above.
        });
    }
}

/// textContent getter — concatenates text of all descendants.
fn text_content_getter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let node_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8_str(scope, "").into());
            return;
        }
    };
    let text = with_dom(|state| {
        let mut visited = std::collections::HashSet::new();
        get_text_content(node_id, &state.document, &mut visited)
    });
    rv.set(v8_str(scope, &text).into());
}

/// textContent setter — removes all children and inserts a single text node.
fn text_content_setter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    value: v8::Local<v8::Value>,
    args: v8::PropertyCallbackArguments,
    _rv: v8::ReturnValue<()>,
) {
    let this = args.this();
    let node_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let text = value
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    // Detach existing children
    with_dom(|state| {
        let children: Vec<NodeId> = state.document.nodes[node_id].children.clone();
        for child in children {
            state.document.nodes[child].parent = None;
        }
        state.document.nodes[node_id].children.clear();
    });

    if !text.is_empty() {
        let text_id = with_dom(|state| {
            let id = state.document.nodes.len();
            state.document.nodes.push(Node {
                id,
                parent: Some(node_id),
                children: Vec::new(),
                data: NodeData::Text(TextData { content: text }),
            });
            id
        });
        with_dom(|state| {
            state.document.nodes[node_id].children.push(text_id);
        });
        let _ = wrap_element(scope, text_id);
    }
}

/// Recursively collect text content from a DOM subtree.
fn get_text_content(
    node_id: NodeId,
    doc: &incognidium_dom::Document,
    visited: &mut std::collections::HashSet<NodeId>,
) -> String {
    if !visited.insert(node_id) {
        return String::new();
    }
    let mut result = String::new();
    if let Some(node) = doc.nodes.get(node_id) {
        match &node.data {
            incognidium_dom::NodeData::Text(t) => result.push_str(&t.content),
            incognidium_dom::NodeData::Element(_) => {
                for child_id in &node.children {
                    result.push_str(&get_text_content(*child_id, doc, visited));
                }
            }
            _ => {}
        }
    }
    result
}

// ── event callbacks ──────────────────────────────────────────────────────

fn add_event_listener_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    let event_type = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let handler = args.get(1);

    // Try to convert handler to string representation
    let handler_str = if handler.is_function() {
        "[function]".to_string()
    } else {
        handler
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default()
    };

    let capture = args.get(2).is_true();

    if event_type.is_empty() {
        return;
    }

    with_dom(|state| {
        if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
            el.event_listeners.push(incognidium_dom::EventListener {
                event_type,
                handler: handler_str,
                capture,
            });
        }
    });
}

fn remove_event_listener_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    let event_type = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if event_type.is_empty() {
        return;
    }

    with_dom(|state| {
        if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
            el.event_listeners.retain(|l| l.event_type != event_type);
        }
    });
}

fn dispatch_event_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_bool(false);
            return;
        }
    };

    let event_val = args.get(0);
    let event_type = event_val
        .to_object(scope)
        .and_then(|obj| {
            let type_key = v8_str(scope, "type");
            obj.get(scope, type_key.into())
                .and_then(|v| v.to_string(scope))
                .map(|s| s.to_rust_string_lossy(scope))
        })
        .unwrap_or_else(|| {
            event_val
                .to_string(scope)
                .map(|s| s.to_rust_string_lossy(scope))
                .unwrap_or_default()
        });

    // Check if there are listeners for this event type
    let has_listener = with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[nid].data {
            el.event_listeners
                .iter()
                .any(|l| l.event_type == event_type)
        } else {
            false
        }
    });

    rv.set_bool(has_listener);
}

// ── element interaction callbacks ──────────────────────────────────────────

fn focus_cb(
    _scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let _nid = match extract_node_id(_scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    // For now, just a stub - in a real implementation this would:
    // 1. Set focus state on the element
    // 2. Trigger focus event
    // 3. Update visual focus indicator
}

fn blur_cb(
    _scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let _nid = match extract_node_id(_scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    // For now, just a stub - in a real implementation this would:
    // 1. Remove focus state from the element
    // 2. Trigger blur event
}

fn click_cb(
    _scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(_scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    // Trigger click behavior - check for click event listeners
    with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[nid].data {
            // Check if there are click event listeners
            let has_click_listener = el.event_listeners.iter().any(|l| l.event_type == "click");
            if has_click_listener {
                // In a full implementation, this would execute the handler
                eprintln!("Click event triggered on element {}", el.tag_name);
            }

            // Handle special click behavior for certain elements
            match el.tag_name.as_str() {
                "input" | "button" => {
                    // Trigger input/button click behavior
                    eprintln!("Click on {} element", el.tag_name);
                }
                "a" => {
                    // Handle anchor click - could navigate
                    if let Some(href) = el.attributes.get("href") {
                        eprintln!("Anchor click to: {}", href);
                    }
                }
                _ => {}
            }
        }
    });
}

fn scroll_into_view_cb(
    _scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(_scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    // Parse options argument
    let _align_to_top = args.get(0);
    // Could be a boolean (alignToTop) or an options object with behavior/block/inline

    // For now, just log - in a real implementation this would:
    // 1. Calculate the element's position
    // 2. Scroll the viewport to make the element visible
    // 3. Respect the scroll options (smooth vs auto, block/inline alignment)
    with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[nid].data {
            eprintln!("scrollIntoView called on <{}>", el.tag_name);
        }
    });
}

// ── mutation callbacks ───────────────────────────────────────────────────

fn remove_child_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let parent = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let child_val = args.get(0);
    let child = match extract_node_id(scope, child_val) {
        Some(n) => n,
        None => {
            rv.set(child_val);
            return;
        }
    };
    with_dom(|state| {
        state.document.nodes[parent]
            .children
            .retain(|&c| c != child);
        state.document.nodes[child].parent = None;
    });
    rv.set(child_val);
}

fn insert_before_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let parent = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let new_val = args.get(0);
    let new_id = match extract_node_id(scope, new_val) {
        Some(n) => n,
        None => {
            rv.set(new_val);
            return;
        }
    };
    let ref_val = args.get(1);
    let ref_id = extract_node_id(scope, ref_val);
    with_dom(|state| {
        if let Some(op) = state.document.nodes[new_id].parent {
            state.document.nodes[op].children.retain(|&c| c != new_id);
        }
        state.document.nodes[new_id].parent = Some(parent);
        let idx = match ref_id {
            Some(r) => state.document.nodes[parent]
                .children
                .iter()
                .position(|&c| c == r)
                .unwrap_or(state.document.nodes[parent].children.len()),
            None => state.document.nodes[parent].children.len(),
        };
        state.document.nodes[parent].children.insert(idx, new_id);
    });
    execute_appended_script_if_needed(scope, new_val, new_id);
    rv.set(new_val);
}

fn set_attribute_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let value = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    with_dom(|state| {
        if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
            el.attributes.insert(name, value);
        }
    });
}

fn get_attribute_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let result = with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[nid].data {
            el.attributes.get(&name).cloned()
        } else {
            None
        }
    });
    match result {
        Some(v) => {
            let s = v8_str(scope, &v);
            rv.set(s.into());
        }
        None => rv.set_null(),
    }
}

fn has_attribute_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_bool(false);
            return;
        }
    };
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let result = with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[nid].data {
            el.attributes.contains_key(&name)
        } else {
            false
        }
    });
    rv.set_bool(result);
}

fn remove_attribute_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    with_dom(|state| {
        if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
            el.attributes.remove(&name);
        }
    });
}

fn element_src_getter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    element_attr_getter(scope, args, &mut rv, "src");
}

fn element_src_setter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    value: v8::Local<v8::Value>,
    args: v8::PropertyCallbackArguments,
    _rv: v8::ReturnValue<()>,
) {
    element_attr_setter(scope, args, value, "src");
}

fn element_href_getter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    element_attr_getter(scope, args, &mut rv, "href");
}

fn element_href_setter_cb(
    scope: &mut v8::HandleScope,
    _key: v8::Local<v8::Name>,
    value: v8::Local<v8::Value>,
    args: v8::PropertyCallbackArguments,
    _rv: v8::ReturnValue<()>,
) {
    element_attr_setter(scope, args, value, "href");
}

fn element_attr_getter_cb(
    scope: &mut v8::HandleScope,
    key: v8::Local<v8::Name>,
    args: v8::PropertyCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let attr = key
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    element_attr_getter(scope, args, &mut rv, &attr);
}

fn element_attr_setter_cb(
    scope: &mut v8::HandleScope,
    key: v8::Local<v8::Name>,
    value: v8::Local<v8::Value>,
    args: v8::PropertyCallbackArguments,
    _rv: v8::ReturnValue<()>,
) {
    let attr = key
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    element_attr_setter(scope, args, value, &attr);
}

fn element_attr_getter(
    scope: &mut v8::HandleScope,
    args: v8::PropertyCallbackArguments,
    rv: &mut v8::ReturnValue,
    attr: &str,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };
    let result = with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[nid].data {
            el.attributes.get(attr).cloned()
        } else {
            None
        }
    });
    match result {
        Some(v) => rv.set(v8_str(scope, &v).into()),
        None => rv.set_null(),
    }
}

fn element_attr_setter(
    scope: &mut v8::HandleScope,
    args: v8::PropertyCallbackArguments,
    value: v8::Local<v8::Value>,
    attr: &str,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let val = value
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    with_dom(|state| {
        if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
            el.attributes.insert(attr.to_string(), val);
        }
    });
}

// ── getBoundingClientRect ────────────────────────────────────────────────

fn get_bounding_client_rect_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let _nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };
    // Return a DOMRect-like object with default/placeholder values
    // In a full implementation, this would require layout information
    let rect = v8::Object::new(scope);
    set_int(scope, rect, "x", 0);
    set_int(scope, rect, "y", 0);
    set_int(scope, rect, "width", 0);
    set_int(scope, rect, "height", 0);
    set_int(scope, rect, "top", 0);
    set_int(scope, rect, "right", 0);
    set_int(scope, rect, "bottom", 0);
    set_int(scope, rect, "left", 0);
    rv.set(rect.into());
}

// ── classList callbacks ─────────────────────────────────────────────────

fn classlist_add_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    // classList is on the element, so we need to get the element from the classList object
    // The classList object should have a reference to its owner element
    let class_name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if class_name.is_empty() {
        return;
    }

    // Get the owner element from the __element__ property
    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
                let current = el.attributes.get("class").cloned().unwrap_or_default();
                let classes: Vec<&str> = current.split_whitespace().collect();
                if !classes.contains(&class_name.as_str()) {
                    let new_class = if current.is_empty() {
                        class_name
                    } else {
                        format!("{} {}", current, class_name)
                    };
                    el.attributes.insert("class".to_string(), new_class);
                }
            }
        });
    }
}

fn classlist_remove_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let class_name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if class_name.is_empty() {
        return;
    }

    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
                if let Some(current) = el.attributes.get("class") {
                    let classes: Vec<&str> = current.split_whitespace().collect();
                    let filtered: Vec<&str> =
                        classes.into_iter().filter(|&c| c != class_name).collect();
                    let new_class = filtered.join(" ");
                    if new_class.is_empty() {
                        el.attributes.remove("class");
                    } else {
                        el.attributes.insert("class".to_string(), new_class);
                    }
                }
            }
        });
    }
}

fn classlist_contains_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let class_name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    let result = if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref el) = state.document.nodes[nid].data {
                if let Some(current) = el.attributes.get("class") {
                    let classes: Vec<&str> = current.split_whitespace().collect();
                    classes.contains(&class_name.as_str())
                } else {
                    false
                }
            } else {
                false
            }
        })
    } else {
        false
    };
    rv.set_bool(result);
}

fn classlist_toggle_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let class_name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if class_name.is_empty() {
        rv.set_bool(false);
        return;
    }

    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    let (added, should_add) = if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref el) = state.document.nodes[nid].data {
                if let Some(current) = el.attributes.get("class") {
                    let classes: Vec<&str> = current.split_whitespace().collect();
                    let contains = classes.contains(&class_name.as_str());
                    (!contains, !contains) // (added_result, should_add)
                } else {
                    (true, true) // No classes, will add
                }
            } else {
                (false, false)
            }
        })
    } else {
        (false, false)
    };

    if should_add {
        // Add the class
        if let Some(nid) = owner_id {
            with_dom(|state| {
                if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
                    let current = el.attributes.get("class").cloned().unwrap_or_default();
                    let new_class = if current.is_empty() {
                        class_name.clone()
                    } else {
                        format!("{} {}", current, class_name)
                    };
                    el.attributes.insert("class".to_string(), new_class);
                }
            });
        }
    } else if let Some(nid) = owner_id {
        // Remove the class
        with_dom(|state| {
            if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
                if let Some(current) = el.attributes.get("class") {
                    let classes: Vec<&str> = current.split_whitespace().collect();
                    let filtered: Vec<&str> =
                        classes.into_iter().filter(|&c| c != class_name).collect();
                    let new_class = filtered.join(" ");
                    if new_class.is_empty() {
                        el.attributes.remove("class");
                    } else {
                        el.attributes.insert("class".to_string(), new_class);
                    }
                }
            }
        });
    }

    rv.set_bool(added);
}

fn classlist_replace_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let old_class = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let new_class = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if old_class.is_empty() || new_class.is_empty() {
        rv.set_bool(false);
        return;
    }

    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    let replaced = if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
                if let Some(current) = el.attributes.get("class") {
                    let classes: Vec<&str> = current.split_whitespace().collect();
                    if classes.contains(&old_class.as_str()) {
                        let replaced: Vec<&str> = classes
                            .into_iter()
                            .map(|c| {
                                if c == old_class {
                                    new_class.as_str()
                                } else {
                                    c
                                }
                            })
                            .collect();
                        el.attributes
                            .insert("class".to_string(), replaced.join(" "));
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        })
    } else {
        false
    };
    rv.set_bool(replaced);
}

// ── additional element callbacks ───────────────────────────────────────────

fn replace_child_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let parent = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let new_val = args.get(0);
    let new_id = match extract_node_id(scope, new_val) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };
    let old_val = args.get(1);
    let old_id = match extract_node_id(scope, old_val) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };

    with_dom(|state| {
        // Remove old child from parent's children list
        state.document.nodes[parent]
            .children
            .retain(|c| *c != old_id);
        // Remove old child's parent reference
        state.document.nodes[old_id].parent = None;

        // Remove new node from its current parent if any
        if let Some(op) = state.document.nodes[new_id].parent {
            state.document.nodes[op].children.retain(|c| *c != new_id);
        }

        // Find position where old child was and insert new child there
        // Since we already removed old_id, we just append new_id
        state.document.nodes[parent].children.push(new_id);
        state.document.nodes[new_id].parent = Some(parent);
    });

    rv.set(old_val);
}

fn clone_node_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };

    let deep = args.get(0).is_true();

    let cloned_id = with_dom(|state| {
        fn clone_node_recursive(
            doc: &mut Document,
            source_id: NodeId,
            parent: Option<NodeId>,
            deep: bool,
        ) -> NodeId {
            let new_id = doc.nodes.len();

            // Clone children first (before we push and invalidate the source reference)
            let children_to_clone: Vec<NodeId> = if deep {
                doc.nodes[source_id].children.clone()
            } else {
                Vec::new()
            };

            // Get the source data
            let source = &doc.nodes[source_id];

            // Clone the node data
            let cloned_data = match &source.data {
                NodeData::Element(el) => {
                    let mut new_el = ElementData::new(&el.tag_name);
                    new_el.attributes = el.attributes.clone();
                    NodeData::Element(new_el)
                }
                NodeData::Text(t) => NodeData::Text(TextData {
                    content: t.content.clone(),
                }),
                NodeData::Comment(c) => NodeData::Comment(c.clone()),
                NodeData::Document => NodeData::Document,
            };

            doc.nodes.push(Node {
                id: new_id,
                parent,
                children: Vec::new(),
                data: cloned_data,
            });

            // Clone children if deep clone
            if deep {
                for child_id in children_to_clone {
                    let cloned_child = clone_node_recursive(doc, child_id, Some(new_id), true);
                    doc.nodes[new_id].children.push(cloned_child);
                }
            }

            new_id
        }

        clone_node_recursive(
            &mut state.document,
            nid,
            None, // Cloned node has no parent initially
            deep,
        )
    });

    let obj = wrap_element(scope, cloned_id);
    rv.set(obj.into());
}

fn remove_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    with_dom(|state| {
        if let Some(parent_id) = state.document.nodes[nid].parent {
            // Remove from parent's children list
            state.document.nodes[parent_id]
                .children
                .retain(|c| *c != nid);
            // Clear parent reference
            state.document.nodes[nid].parent = None;
        }
    });
}

fn contains_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let container_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_bool(false);
            return;
        }
    };
    let child_val = args.get(0);
    let child_id = match extract_node_id(scope, child_val) {
        Some(n) => n,
        None => {
            rv.set_bool(false);
            return;
        }
    };

    // Check if child is the same as container (element.contains(itself) returns true)
    if container_id == child_id {
        rv.set_bool(true);
        return;
    }

    let result = with_dom(|state| {
        // Walk up the tree from child to see if we reach container
        let mut current = state.document.nodes[child_id].parent;
        while let Some(parent) = current {
            if parent == container_id {
                return true;
            }
            current = state.document.nodes[parent].parent;
        }
        false
    });

    rv.set_bool(result);
}

/// normalize() merges adjacent text nodes and removes empty text nodes
fn normalize_cb(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = _args.this();
    let node_id = match extract_node_id(_scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    with_dom(|state| {
        let mut i = 0;
        let children = &state.document.nodes[node_id].children.clone();
        let mut new_children: Vec<NodeId> = Vec::new();
        let mut pending_text: Option<(String, NodeId)> = None; // (accumulated_text, first_node_id)

        for &child_id in children {
            match &state.document.nodes.get(child_id).map(|n| &n.data) {
                Some(NodeData::Text(text_data)) => {
                    if text_data.content.is_empty() {
                        // Skip empty text nodes
                        continue;
                    }
                    match &mut pending_text {
                        Some((acc_text, first_id)) => {
                            // Merge with previous text node
                            *acc_text += &text_data.content;
                            // Mark this node for removal by setting it to a removed state
                            // In a real implementation we would delete the node
                        }
                        None => {
                            pending_text = Some((text_data.content.clone(), child_id));
                            new_children.push(child_id);
                        }
                    }
                }
                _ => {
                    // Non-text node: flush any pending text and add this child
                    if let Some((ref text, id)) = pending_text {
                        // Update the accumulated text in the first text node
                        if let Some(node) = state.document.nodes.get_mut(id) {
                            if let NodeData::Text(ref mut t) = node.data {
                                t.content = text.clone();
                            }
                        }
                        pending_text = None;
                    }
                    new_children.push(child_id);
                }
            }
        }

        // Flush final pending text
        if let Some((ref text, id)) = pending_text {
            if let Some(node) = state.document.nodes.get_mut(id) {
                if let NodeData::Text(ref mut t) = node.data {
                    t.content = text.clone();
                }
            }
        }

        // Update children list
        state.document.nodes[node_id].children = new_children;
    });
}

fn matches_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_bool(false);
            return;
        }
    };

    let selector = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if selector.is_empty() {
        rv.set_bool(false);
        return;
    }

    let result = with_dom(|state| {
        if let NodeData::Element(ref el) = state.document.nodes[nid].data {
            let sel = selector.trim();

            // Handle class selector (.classname)
            if sel.starts_with('.') {
                let class_name = &sel[1..];
                if let Some(class_attr) = el.attributes.get("class") {
                    let classes: Vec<&str> = class_attr.split_whitespace().collect();
                    return classes.contains(&class_name);
                }
                return false;
            }

            // Handle ID selector (#id)
            if sel.starts_with('#') {
                let id = &sel[1..];
                return el.attributes.get("id").map(|v| v == id).unwrap_or(false);
            }

            // Handle tag name selector (case-insensitive)
            let sel_lower = sel.to_lowercase();
            return el.tag_name.to_lowercase() == sel_lower;
        }
        false
    });

    rv.set_bool(result);
}

// ── document callbacks ───────────────────────────────────────────────────

fn get_element_by_id_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let id = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let nid = with_dom(|state| state.document.get_element_by_id(&id));
    match nid {
        Some(n) => {
            let obj = wrap_element(scope, n);
            rv.set(obj.into());
        }
        None => rv.set_null(),
    }
}

fn create_element_raw(tag: &str) -> NodeId {
    with_dom(|state| {
        let id = state.document.nodes.len();
        state.document.nodes.push(Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Element(ElementData::new(tag)),
        });
        id
    })
}

fn create_element<'s>(scope: &mut v8::HandleScope<'s>, tag: &str) -> v8::Local<'s, v8::Object> {
    let node_id = create_element_raw(tag);
    wrap_element(scope, node_id)
}

fn create_element_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let tag = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "div".into());
    let obj = create_element(scope, &tag);
    rv.set(obj.into());
}

fn create_text_node_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let text = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let node_id = with_dom(|state| {
        let id = state.document.nodes.len();
        state.document.nodes.push(Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Text(TextData { content: text }),
        });
        id
    });
    let obj = wrap_element(scope, node_id);
    rv.set(obj.into());
}

fn create_document_fragment_cb(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // DocumentFragment is a special node type that acts as a container
    // It can hold children but isn't part of the main document tree
    let node_id = with_dom(|state| {
        let id = state.document.nodes.len();
        // Create as an element with a special fragment tag
        let mut frag_data = ElementData::new("fragment");
        frag_data
            .attributes
            .insert("__document_fragment__".to_string(), "true".to_string());
        state.document.nodes.push(Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Element(frag_data),
        });
        id
    });

    let obj = wrap_element(scope, node_id);

    // Mark it as a DocumentFragment by adding a special property
    set_str(scope, obj, "nodeName", "#document-fragment");
    set_int(scope, obj, "nodeType", 11); // DOCUMENT_FRAGMENT_NODE

    rv.set(obj.into());
}

fn create_comment_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let comment = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let node_id = with_dom(|state| {
        let id = state.document.nodes.len();
        state.document.nodes.push(Node {
            id,
            parent: None,
            children: Vec::new(),
            data: NodeData::Comment(comment),
        });
        id
    });
    let obj = wrap_element(scope, node_id);
    set_str(scope, obj, "nodeName", "#comment");
    set_int(scope, obj, "nodeType", 8); // COMMENT_NODE
    rv.set(obj.into());
}

fn create_html_document_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // jQuery uses document.implementation.createHTMLDocument('') to build a
    // sandboxed document for HTML parsing. Return a minimal document object.
    let title = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let doc = v8::Object::new(scope);
    set_int(scope, doc, "nodeType", 9);
    set_str(scope, doc, "nodeName", "#document");
    set_str(scope, doc, "title", &title);
    set_str(scope, doc, "readyState", "complete");
    set_str(scope, doc, "URL", "about:blank");
    set_str(scope, doc, "documentURI", "about:blank");
    // body / documentElement are needed by jQuery's support tests.
    let body = create_element(scope, "body");
    let html = create_element(scope, "html");
    let bk = v8_str(scope, "body");
    let hk = v8_str(scope, "documentElement");
    doc.set(scope, bk.into(), body.into());
    doc.set(scope, hk.into(), html.into());
    // Minimal createElement used by jQuery for feature detection.
    set_fn(scope, doc, "createElement", create_element_cb);
    set_fn(scope, doc, "createTextNode", create_text_node_cb);
    set_fn(
        scope,
        doc,
        "createDocumentFragment",
        create_document_fragment_cb,
    );
    rv.set(doc.into());
}

fn create_range_cb(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // Minimal Range object used by some scripts for text selection / detection.
    let range = v8::Object::new(scope);
    set_fn(scope, range, "setStart", noop);
    set_fn(scope, range, "setEnd", noop);
    set_fn(scope, range, "selectNode", noop);
    set_fn(scope, range, "selectNodeContents", noop);
    set_fn(scope, range, "collapse", noop);
    set_fn(scope, range, "cloneContents", noop_obj);
    set_fn(scope, range, "deleteContents", noop);
    set_fn(scope, range, "extractContents", noop_obj);
    set_fn(scope, range, "insertNode", noop);
    set_fn(scope, range, "surroundContents", noop);
    set_fn(scope, range, "getBoundingClientRect", noop_obj);
    set_fn(scope, range, "getClientRects", noop_obj);
    set_fn(scope, range, "toString", noop_str);
    rv.set(range.into());
}

/// Recursively clone a node and its children, returning the new node ID
fn clone_node_recursive(source_id: NodeId, doc: &mut Document, deep: bool) -> NodeId {
    let new_id = doc.nodes.len();

    let source_data = match doc.nodes.get(source_id) {
        Some(node) => node.data.clone(),
        None => return new_id,
    };

    let new_node = Node {
        id: new_id,
        parent: None,
        children: Vec::new(),
        data: source_data.clone(),
    };
    doc.nodes.push(new_node);

    // Deep clone: also clone children
    if deep {
        if let Some(source_node) = doc.nodes.get(source_id) {
            let child_ids: Vec<NodeId> = source_node.children.clone();
            for child_id in child_ids {
                let cloned_child = clone_node_recursive(child_id, doc, true);
                doc.nodes[new_id].children.push(cloned_child);
                if let Some(child) = doc.nodes.get_mut(cloned_child) {
                    child.parent = Some(new_id);
                }
            }
        }
    }

    new_id
}

fn import_node_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let node_val = args.get(0);
    let deep = args.get(1).is_true();

    let source_id = match extract_node_id(scope, node_val) {
        Some(id) => id,
        None => {
            rv.set_null();
            return;
        }
    };

    let new_id = with_dom(|state| clone_node_recursive(source_id, &mut state.document, deep));

    let obj = wrap_element(scope, new_id);
    rv.set(obj.into());
}

fn adopt_node_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // adoptNode removes the node from its current parent and returns it
    // The node is effectively moved to this document
    let node_val = args.get(0);

    let node_id = match extract_node_id(scope, node_val) {
        Some(id) => id,
        None => {
            rv.set_null();
            return;
        }
    };

    // Remove from current parent
    with_dom(|state| {
        if let Some(old_parent) = state.document.nodes[node_id].parent {
            state.document.nodes[old_parent]
                .children
                .retain(|c| *c != node_id);
        }
        state.document.nodes[node_id].parent = None;
    });

    rv.set(node_val);
}

fn query_selector_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let sel = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let nid = with_dom(|state| {
        let bridge = murkiu_bindings::DomBridge::new(state.document.clone());
        bridge.query_selector(&sel)
    });
    match nid {
        Some(n) => {
            let obj = wrap_element(scope, n);
            rv.set(obj.into());
        }
        None => rv.set_null(),
    }
}

/// Add an `item(n)` method to a plain Array so it behaves like an
/// HTMLCollection / NodeList. Scripts such as NASA's analytics call
/// `document.getElementsByTagName(...).item(0)`.
fn add_item_method(scope: &mut v8::HandleScope, arr: v8::Local<v8::Array>) {
    fn item_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this = args.this();
        if let Ok(arr) = v8::Local::<v8::Array>::try_from(this) {
            let idx = if args.length() > 0 {
                args.get(0).to_int32(scope).map(|i| i.value()).unwrap_or(0) as u32
            } else {
                0
            };
            rv.set(
                arr.get_index(scope, idx)
                    .unwrap_or_else(|| v8::null(scope).into()),
            );
        } else {
            rv.set(v8::null(scope).into());
        }
    }
    let arr_obj = v8::Local::<v8::Object>::from(arr);
    set_fn(scope, arr_obj, "item", item_cb);
}

// ── querySelectorAll ───────────────────────────────────────────────────────

fn query_selector_all_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let sel = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let nids = with_dom(|state| {
        // Simple selector matching without using DomBridge
        let mut results = Vec::new();
        let sel_trim = sel.trim();

        fn matches_selector(doc: &Document, node_id: NodeId, sel: &str) -> bool {
            if let NodeData::Element(ref el) = doc.nodes[node_id].data {
                let sel_trim = sel.trim();

                // Class selector
                if sel_trim.starts_with('.') {
                    let class_name = &sel_trim[1..];
                    if let Some(class_attr) = el.attributes.get("class") {
                        let classes: Vec<&str> = class_attr.split_whitespace().collect();
                        return classes.contains(&class_name);
                    }
                    return false;
                }

                // ID selector
                if sel_trim.starts_with('#') {
                    let id = &sel_trim[1..];
                    return el.attributes.get("id").map(|v| v == id).unwrap_or(false);
                }

                // Attribute selector: [attr], [attr="val"], [attr*="val"], [attr^="val"], [attr$="val"]
                if sel_trim.starts_with('[') && sel_trim.ends_with(']') {
                    let inner = &sel_trim[1..sel_trim.len() - 1];
                    // Find operator
                    let ops = ["*=", "^=", "$=", "~="];
                    for op in ops {
                        if let Some(pos) = inner.find(op) {
                            let attr = inner[..pos].trim();
                            let val = inner[pos + op.len()..].trim();
                            // Remove quotes
                            let val_clean = val
                                .strip_prefix('"')
                                .and_then(|v| v.strip_suffix('"'))
                                .or_else(|| val.strip_prefix("'").and_then(|v| v.strip_suffix("'")))
                                .unwrap_or(val);
                            let attr_val = el.attributes.get(attr);
                            return match op {
                                "*=" => attr_val.map(|v| v.contains(val_clean)).unwrap_or(false),
                                "^=" => attr_val.map(|v| v.starts_with(val_clean)).unwrap_or(false),
                                "$=" => attr_val.map(|v| v.ends_with(val_clean)).unwrap_or(false),
                                "~=" => attr_val
                                    .map(|v| v.split_whitespace().any(|w| w == val_clean))
                                    .unwrap_or(false),
                                _ => false,
                            };
                        }
                    }
                    // No operator - just check attribute exists
                    let attr = inner.trim();
                    return el.attributes.contains_key(attr);
                }

                // Tag selector
                return el.tag_name.to_lowercase() == sel_trim.to_lowercase();
            }
            false
        }

        // Start from document element (html) if it exists
        if let Some(html_id) = state.document.document_element() {
            walk_dom_nodes(&state.document, html_id, |id| {
                if matches_selector(&state.document, id, sel_trim) {
                    results.push(id);
                }
            });
        }

        results
    });

    // Create array of elements
    let arr = v8::Array::new(scope, nids.len() as i32);
    for (i, nid) in nids.iter().enumerate() {
        let obj = wrap_element(scope, *nid);
        arr.set_index(scope, i as u32, obj.into());
    }
    add_item_method(scope, arr);
    rv.set(arr.into());
}

// ── getElementsByTagName ──────────────────────────────────────────────────

fn get_elements_by_tag_name_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let tag = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default()
        .to_lowercase();

    let nids = with_dom(|state| {
        let mut results = Vec::new();

        // Start from document element
        if let Some(html_id) = state.document.document_element() {
            walk_dom_nodes(&state.document, html_id, |id| {
                if let NodeData::Element(ref el) = state.document.nodes[id].data {
                    if el.tag_name.to_lowercase() == tag || tag == "*" {
                        results.push(id);
                    }
                }
            });
        }

        results
    });

    // Create HTMLCollection-like array
    let arr = v8::Array::new(scope, nids.len() as i32);
    for (i, nid) in nids.iter().enumerate() {
        let obj = wrap_element(scope, *nid);
        arr.set_index(scope, i as u32, obj.into());
    }
    add_item_method(scope, arr);
    rv.set(arr.into());
}

// ── getElementsByClassName ────────────────────────────────────────────────

fn get_elements_by_class_name_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let class_name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if class_name.is_empty() {
        rv.set(v8::Array::new(scope, 0).into());
        return;
    }

    let nids = with_dom(|state| {
        let mut results = Vec::new();
        if let Some(html_id) = state.document.document_element() {
            walk_dom_nodes(&state.document, html_id, |id| {
                if let NodeData::Element(ref el) = state.document.nodes[id].data {
                    if let Some(class_attr) = el.attributes.get("class") {
                        let classes: Vec<&str> = class_attr.split_whitespace().collect();
                        if classes.contains(&class_name.as_str()) {
                            results.push(id);
                        }
                    }
                }
            });
        }
        results
    });

    // Create HTMLCollection-like array
    let arr = v8::Array::new(scope, nids.len() as i32);
    for (i, nid) in nids.iter().enumerate() {
        let obj = wrap_element(scope, *nid);
        arr.set_index(scope, i as u32, obj.into());
    }
    add_item_method(scope, arr);
    rv.set(arr.into());
}

fn element_query_selector_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let element_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };

    let sel = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let nid = with_dom(|state| {
        let sel_trim = sel.trim();

        fn matches_selector(doc: &Document, node_id: NodeId, sel: &str) -> bool {
            if let NodeData::Element(ref el) = doc.nodes[node_id].data {
                let sel_trim = sel.trim();

                if sel_trim.starts_with('.') {
                    let class_name = &sel_trim[1..];
                    if let Some(class_attr) = el.attributes.get("class") {
                        let classes: Vec<&str> = class_attr.split_whitespace().collect();
                        return classes.contains(&class_name);
                    }
                    return false;
                }

                if sel_trim.starts_with('#') {
                    let id = &sel_trim[1..];
                    return el.attributes.get("id").map(|v| v == id).unwrap_or(false);
                }

                // Attribute selector: [attr], [attr="val"], [attr*="val"], [attr^="val"], [attr$="val"]
                if sel_trim.starts_with('[') && sel_trim.ends_with(']') {
                    let inner = &sel_trim[1..sel_trim.len() - 1];
                    let ops = ["*=", "^=", "$=", "~="];
                    for op in ops {
                        if let Some(pos) = inner.find(op) {
                            let attr = inner[..pos].trim();
                            let val = inner[pos + op.len()..].trim();
                            let val_clean = val
                                .strip_prefix('"')
                                .and_then(|v| v.strip_suffix('"'))
                                .or_else(|| val.strip_prefix("'").and_then(|v| v.strip_suffix("'")))
                                .unwrap_or(val);
                            let attr_val = el.attributes.get(attr);
                            return match op {
                                "*=" => attr_val.map(|v| v.contains(val_clean)).unwrap_or(false),
                                "^=" => attr_val.map(|v| v.starts_with(val_clean)).unwrap_or(false),
                                "$=" => attr_val.map(|v| v.ends_with(val_clean)).unwrap_or(false),
                                "~=" => attr_val
                                    .map(|v| v.split_whitespace().any(|w| w == val_clean))
                                    .unwrap_or(false),
                                _ => false,
                            };
                        }
                    }
                    let attr = inner.trim();
                    return el.attributes.contains_key(attr);
                }

                return el.tag_name.to_lowercase() == sel_trim.to_lowercase();
            }
            false
        }

        // Search starting from element's children (not the element itself)
        let mut found = None;
        walk_dom_nodes(&state.document, element_id, |id| {
            if id == element_id {
                return;
            }
            if found.is_none() && matches_selector(&state.document, id, sel_trim) {
                found = Some(id);
            }
        });
        found
    });

    match nid {
        Some(n) => {
            let obj = wrap_element(scope, n);
            rv.set(obj.into());
        }
        None => rv.set_null(),
    }
}

// ── Element querySelectorAll (scoped to element) ────────────────────────────

fn element_query_selector_all_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let element_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8::Array::new(scope, 0).into());
            return;
        }
    };

    let sel = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let nids = with_dom(|state| {
        let mut results = Vec::new();
        let sel_trim = sel.trim();
        if let NodeData::Element(_) = state.document.nodes[element_id].data {
            walk_dom_nodes(&state.document, element_id, |id| {
                if id == element_id {
                    return;
                }
                if let NodeData::Element(ref el) = state.document.nodes[id].data {
                    let sel_trim = sel.trim();
                    let matched = if sel_trim.starts_with('.') {
                        let class_name = &sel_trim[1..];
                        el.attributes
                            .get("class")
                            .map(|c| c.split_whitespace().any(|w| w == class_name))
                            .unwrap_or(false)
                    } else if sel_trim.starts_with('#') {
                        let id_name = &sel_trim[1..];
                        el.attributes
                            .get("id")
                            .map(|v| v == id_name)
                            .unwrap_or(false)
                    } else if sel_trim.starts_with('[') && sel_trim.ends_with(']') {
                        let inner = &sel_trim[1..sel_trim.len() - 1];
                        let ops = ["*=", "^=", "$=", "~="];
                        let mut attr_match = false;
                        for op in ops {
                            if let Some(pos) = inner.find(op) {
                                let attr = inner[..pos].trim();
                                let val = inner[pos + op.len()..].trim();
                                let val_clean = val
                                    .strip_prefix('"')
                                    .and_then(|v| v.strip_suffix('"'))
                                    .or_else(|| {
                                        val.strip_prefix("'").and_then(|v| v.strip_suffix("'"))
                                    })
                                    .unwrap_or(val);
                                let attr_val = el.attributes.get(attr);
                                attr_match = match op {
                                    "*=" => {
                                        attr_val.map(|v| v.contains(val_clean)).unwrap_or(false)
                                    }
                                    "^=" => {
                                        attr_val.map(|v| v.starts_with(val_clean)).unwrap_or(false)
                                    }
                                    "$=" => {
                                        attr_val.map(|v| v.ends_with(val_clean)).unwrap_or(false)
                                    }
                                    "~=" => attr_val
                                        .map(|v| v.split_whitespace().any(|w| w == val_clean))
                                        .unwrap_or(false),
                                    _ => false,
                                };
                                break;
                            }
                        }
                        if !attr_match && inner.trim() == inner {
                            attr_match = el.attributes.contains_key(inner.trim());
                        }
                        attr_match
                    } else {
                        el.tag_name.to_lowercase() == sel_trim.to_lowercase()
                    };
                    if matched {
                        results.push(id);
                    }
                }
            });
        }
        results
    });

    let arr = v8::Array::new(scope, nids.len() as i32);
    for (i, nid) in nids.iter().enumerate() {
        let obj = wrap_element(scope, *nid);
        arr.set_index(scope, i as u32, obj.into());
    }
    rv.set(arr.into());
}

// ── Element getElementsByTagName (scoped to element) ───────────────────────

fn element_get_elements_by_tag_name_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let element_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8::Array::new(scope, 0).into());
            return;
        }
    };

    let tag = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default()
        .to_lowercase();

    let nids = with_dom(|state| {
        let mut results = Vec::new();
        walk_dom_nodes(&state.document, element_id, |id| {
            if id == element_id {
                return;
            }
            if let NodeData::Element(ref el) = state.document.nodes[id].data {
                if el.tag_name.to_lowercase() == tag || tag == "*" {
                    results.push(id);
                }
            }
        });
        results
    });

    let arr = v8::Array::new(scope, nids.len() as i32);
    for (i, nid) in nids.iter().enumerate() {
        let obj = wrap_element(scope, *nid);
        arr.set_index(scope, i as u32, obj.into());
    }
    add_item_method(scope, arr);
    rv.set(arr.into());
}

// ── Element getElementsByClassName (scoped to element) ────────────────────

fn element_get_elements_by_class_name_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let element_id = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set(v8::Array::new(scope, 0).into());
            return;
        }
    };

    let class_name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if class_name.is_empty() {
        rv.set(v8::Array::new(scope, 0).into());
        return;
    }

    let nids = with_dom(|state| {
        let mut results = Vec::new();
        walk_dom_nodes(&state.document, element_id, |id| {
            if id == element_id {
                return;
            }
            if let NodeData::Element(ref el) = state.document.nodes[id].data {
                if let Some(class_attr) = el.attributes.get("class") {
                    let classes: Vec<&str> = class_attr.split_whitespace().collect();
                    if classes.contains(&class_name.as_str()) {
                        results.push(id);
                    }
                }
            }
        });
        results
    });

    let arr = v8::Array::new(scope, nids.len() as i32);
    for (i, nid) in nids.iter().enumerate() {
        let obj = wrap_element(scope, *nid);
        arr.set_index(scope, i as u32, obj.into());
    }
    add_item_method(scope, arr);
    rv.set(arr.into());
}

// ── getElementsByName ─────────────────────────────────────────────────────

fn get_elements_by_name_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let name = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if name.is_empty() {
        rv.set(v8::Array::new(scope, 0).into());
        return;
    }

    let nids = with_dom(|state| {
        let mut results = Vec::new();
        if let Some(html_id) = state.document.document_element() {
            walk_dom_nodes(&state.document, html_id, |id| {
                if let NodeData::Element(ref el) = state.document.nodes[id].data {
                    if el
                        .attributes
                        .get("name")
                        .map(|v| v.as_str() == name)
                        .unwrap_or(false)
                    {
                        results.push(id);
                    }
                }
            });
        }
        results
    });

    // Create NodeList-like array
    let arr = v8::Array::new(scope, nids.len() as i32);
    for (i, nid) in nids.iter().enumerate() {
        let obj = wrap_element(scope, *nid);
        arr.set_index(scope, i as u32, obj.into());
    }
    rv.set(arr.into());
}

// ── style setProperty/getPropertyValue/removeProperty ──────────────────────

fn style_set_property_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let property = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let value = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if property.is_empty() {
        return;
    }

    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
                // Convert camelCase to kebab-case for CSS properties
                let css_property = property
                    .replace("backgroundColor", "background-color")
                    .replace("borderColor", "border-color")
                    .replace("borderWidth", "border-width")
                    .replace("fontSize", "font-size")
                    .replace("fontFamily", "font-family")
                    .replace("fontWeight", "font-weight")
                    .replace("lineHeight", "line-height")
                    .replace("textAlign", "text-align")
                    .replace("marginTop", "margin-top")
                    .replace("marginRight", "margin-right")
                    .replace("marginBottom", "margin-bottom")
                    .replace("marginLeft", "margin-left")
                    .replace("paddingTop", "padding-top")
                    .replace("paddingRight", "padding-right")
                    .replace("paddingBottom", "padding-bottom")
                    .replace("paddingLeft", "padding-left")
                    .replace("width", "width")
                    .replace("height", "height")
                    .replace("display", "display")
                    .replace("position", "position")
                    .replace("top", "top")
                    .replace("left", "left")
                    .replace("right", "right")
                    .replace("bottom", "bottom")
                    .replace("color", "color")
                    .replace("background", "background")
                    .replace("border", "border")
                    .replace("margin", "margin")
                    .replace("padding", "padding");

                // Get or create style attribute
                let style_attr = el.attributes.get("style").cloned().unwrap_or_default();
                let mut styles: HashMap<String, String> = style_attr
                    .split(';')
                    .filter_map(|s| {
                        let mut parts = s.trim().splitn(2, ':');
                        let prop = parts.next()?.trim().to_string();
                        let val = parts.next()?.trim().to_string();
                        if !prop.is_empty() {
                            Some((prop, val))
                        } else {
                            None
                        }
                    })
                    .collect();

                if value.is_empty() {
                    styles.remove(&css_property);
                } else {
                    styles.insert(css_property, value);
                }

                // Rebuild style string
                let new_style: String = styles
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect::<Vec<_>>()
                    .join("; ");

                if new_style.is_empty() {
                    el.attributes.remove("style");
                } else {
                    el.attributes.insert("style".to_string(), new_style);
                }
            }
        });
    }
}

fn style_get_property_value_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let property = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    let result = if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref el) = state.document.nodes[nid].data {
                if let Some(style_attr) = el.attributes.get("style") {
                    // Parse style attribute and find property
                    for part in style_attr.split(';') {
                        let mut kv = part.trim().splitn(2, ':');
                        if let (Some(prop), Some(val)) = (kv.next(), kv.next()) {
                            if prop.trim().to_lowercase() == property.to_lowercase() {
                                return Some(val.trim().to_string());
                            }
                        }
                    }
                }
            }
            None
        })
    } else {
        None
    };

    match result {
        Some(v) => rv.set(v8_str(scope, &v).into()),
        None => rv.set_null(),
    }
}

fn style_remove_property_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let property = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if property.is_empty() {
        return;
    }

    let owner_id = get_prop(scope, this, "__element__").and_then(|v| extract_node_id(scope, v));

    if let Some(nid) = owner_id {
        with_dom(|state| {
            if let NodeData::Element(ref mut el) = state.document.nodes[nid].data {
                if let Some(style_attr) = el.attributes.get("style") {
                    let styles: Vec<&str> = style_attr.split(';').collect();
                    let mut new_styles = Vec::new();

                    for part in styles {
                        let trimmed = part.trim();
                        if !trimmed.is_empty() {
                            let mut kv = trimmed.splitn(2, ':');
                            if let Some(prop) = kv.next() {
                                if prop.trim().to_lowercase() != property.to_lowercase() {
                                    new_styles.push(trimmed);
                                }
                            }
                        }
                    }

                    let new_style = new_styles.join("; ");
                    if new_style.is_empty() {
                        el.attributes.remove("style");
                    } else {
                        el.attributes.insert("style".to_string(), new_style);
                    }
                }
            }
        });
    }
}

// ── closest ───────────────────────────────────────────────────────────────

fn closest_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };

    let selector = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if selector.is_empty() {
        rv.set_null();
        return;
    }

    let result = with_dom(|state| {
        let sel = selector.trim();

        fn matches_selector(doc: &Document, node_id: NodeId, sel: &str) -> bool {
            if let NodeData::Element(ref el) = doc.nodes[node_id].data {
                let sel_trim = sel.trim();

                if sel_trim.starts_with('.') {
                    let class_name = &sel_trim[1..];
                    if let Some(class_attr) = el.attributes.get("class") {
                        let classes: Vec<&str> = class_attr.split_whitespace().collect();
                        return classes.contains(&class_name);
                    }
                    return false;
                }

                if sel_trim.starts_with('#') {
                    let id = &sel_trim[1..];
                    return el.attributes.get("id").map(|v| v == id).unwrap_or(false);
                }

                // Attribute selector: [attr], [attr="val"], [attr*="val"], [attr^="val"], [attr$="val"]
                if sel_trim.starts_with('[') && sel_trim.ends_with(']') {
                    let inner = &sel_trim[1..sel_trim.len() - 1];
                    let ops = ["*=", "^=", "$=", "~="];
                    for op in ops {
                        if let Some(pos) = inner.find(op) {
                            let attr = inner[..pos].trim();
                            let val = inner[pos + op.len()..].trim();
                            let val_clean = val
                                .strip_prefix('"')
                                .and_then(|v| v.strip_suffix('"'))
                                .or_else(|| val.strip_prefix("'").and_then(|v| v.strip_suffix("'")))
                                .unwrap_or(val);
                            let attr_val = el.attributes.get(attr);
                            return match op {
                                "*=" => attr_val.map(|v| v.contains(val_clean)).unwrap_or(false),
                                "^=" => attr_val.map(|v| v.starts_with(val_clean)).unwrap_or(false),
                                "$=" => attr_val.map(|v| v.ends_with(val_clean)).unwrap_or(false),
                                "~=" => attr_val
                                    .map(|v| v.split_whitespace().any(|w| w == val_clean))
                                    .unwrap_or(false),
                                _ => false,
                            };
                        }
                    }
                    let attr = inner.trim();
                    return el.attributes.contains_key(attr);
                }

                return el.tag_name.to_lowercase() == sel_trim.to_lowercase();
            }
            false
        }

        // Walk up the tree from current element
        let mut current = state.document.nodes[nid].parent;
        while let Some(parent_id) = current {
            if matches_selector(&state.document, parent_id, sel) {
                return Some(parent_id);
            }
            current = state.document.nodes[parent_id].parent;
        }
        None
    });

    match result {
        Some(n) => {
            let obj = wrap_element(scope, n);
            rv.set(obj.into());
        }
        None => rv.set_null(),
    }
}

// ── insertAdjacentHTML ──────────────────────────────────────────────────────

/// Insert a slice of already-created node ids at a position relative to `target`.
/// `position` is one of beforebegin/afterbegin/beforeend/afterend.
fn insert_nodes_at_position(
    state: &mut DomState,
    position: &str,
    target: NodeId,
    new_ids: &[NodeId],
) {
    match position {
        "beforebegin" => {
            let parent_id = match state.document.nodes[target].parent {
                Some(p) => p,
                None => return,
            };
            let pos = state.document.nodes[parent_id]
                .children
                .iter()
                .position(|&c| c == target)
                .unwrap_or(state.document.nodes[parent_id].children.len());
            for (offset, &id) in new_ids.iter().enumerate() {
                state.document.nodes[id].parent = Some(parent_id);
                state.document.nodes[parent_id]
                    .children
                    .insert(pos + offset, id);
            }
        }
        "afterbegin" => {
            for (offset, &id) in new_ids.iter().enumerate() {
                state.document.nodes[id].parent = Some(target);
                state.document.nodes[target].children.insert(offset, id);
            }
        }
        "beforeend" => {
            for &id in new_ids {
                state.document.nodes[id].parent = Some(target);
                state.document.nodes[target].children.push(id);
            }
        }
        "afterend" => {
            let parent_id = match state.document.nodes[target].parent {
                Some(p) => p,
                None => return,
            };
            let pos = state.document.nodes[parent_id]
                .children
                .iter()
                .position(|&c| c == target)
                .unwrap_or(state.document.nodes[parent_id].children.len());
            for (offset, &id) in new_ids.iter().enumerate() {
                state.document.nodes[id].parent = Some(parent_id);
                state.document.nodes[parent_id]
                    .children
                    .insert(pos + 1 + offset, id);
            }
        }
        _ => {}
    }
}

fn insert_adjacent_html_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };

    let position = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default()
        .to_lowercase();
    let html = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    if html.is_empty()
        || !matches!(
            position.as_str(),
            "beforebegin" | "afterbegin" | "beforeend" | "afterend"
        )
    {
        return;
    }

    // Determine the parent that the parsed fragment nodes should belong to.
    let fragment_parent = match position.as_str() {
        "beforebegin" | "afterend" => with_dom(|state| state.document.nodes[nid].parent),
        "afterbegin" | "beforeend" => Some(nid),
        _ => None,
    };

    let Some(parent_id) = fragment_parent else {
        return;
    };

    let fragments = parse_html_fragment(&html);
    let all_ids = build_fragment_tree(fragments, parent_id, false);

    with_dom(|state| {
        insert_nodes_at_position(state, &position, nid, &all_ids);
    });

    for &new_id in &all_ids {
        let obj = wrap_element(scope, new_id);
        execute_appended_script_if_needed(scope, obj.into(), new_id);
    }
}

// ── insertAdjacentElement ──────────────────────────────────────────────────

fn insert_adjacent_element_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };

    let position = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default()
        .to_lowercase();
    let element_val = args.get(1);
    let element_id = match extract_node_id(scope, element_val) {
        Some(n) => n,
        None => {
            rv.set_null();
            return;
        }
    };

    with_dom(|state| {
        // Remove element from its current parent
        if let Some(old_parent) = state.document.nodes[element_id].parent {
            state.document.nodes[old_parent]
                .children
                .retain(|c| *c != element_id);
        }

        match position.as_str() {
            "beforebegin" => {
                if let Some(parent_id) = state.document.nodes[nid].parent {
                    state.document.nodes[element_id].parent = Some(parent_id);
                    if let Some(pos) = state.document.nodes[parent_id]
                        .children
                        .iter()
                        .position(|c| *c == nid)
                    {
                        state.document.nodes[parent_id]
                            .children
                            .insert(pos, element_id);
                    }
                }
            }
            "afterbegin" => {
                state.document.nodes[element_id].parent = Some(nid);
                state.document.nodes[nid].children.insert(0, element_id);
            }
            "beforeend" => {
                state.document.nodes[element_id].parent = Some(nid);
                state.document.nodes[nid].children.push(element_id);
            }
            "afterend" => {
                if let Some(parent_id) = state.document.nodes[nid].parent {
                    state.document.nodes[element_id].parent = Some(parent_id);
                    if let Some(pos) = state.document.nodes[parent_id]
                        .children
                        .iter()
                        .position(|c| *c == nid)
                    {
                        state.document.nodes[parent_id]
                            .children
                            .insert(pos + 1, element_id);
                    }
                }
            }
            _ => {
                // Restore parent reference if position is invalid
                return;
            }
        }
    });

    rv.set(element_val);
}

// ── before / after / replaceWith / replaceChildren ─────────────────────────

/// Collect node ids for a variadic list of nodes or strings, creating text nodes
/// for strings and reparenting existing nodes. Used by Element.before/after/etc.
fn collect_insertion_nodes(
    scope: &mut v8::HandleScope,
    args: &v8::FunctionCallbackArguments,
) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for i in 0..args.length() {
        let arg = args.get(i);
        if arg.is_string() {
            let text = arg.to_rust_string_lossy(scope);
            let text_id = with_dom(|state| {
                let id = state.document.nodes.len();
                state.document.nodes.push(Node {
                    id,
                    parent: None,
                    children: Vec::new(),
                    data: NodeData::Text(TextData { content: text }),
                });
                id
            });
            let _ = wrap_element(scope, text_id);
            ids.push(text_id);
        } else if let Ok(node) = v8::Local::<v8::Object>::try_from(arg) {
            if let Some(child) = extract_node_id(scope, node.into()) {
                with_dom(|state| {
                    if let Some(old_parent) = state.document.nodes[child].parent {
                        state.document.nodes[old_parent]
                            .children
                            .retain(|&c| c != child);
                    }
                    state.document.nodes[child].parent = None;
                });
                ids.push(child);
            }
        }
    }
    ids
}

fn before_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let new_ids = collect_insertion_nodes(scope, &args);
    let parent_id = match with_dom(|state| state.document.nodes[nid].parent) {
        Some(p) => p,
        None => return,
    };
    with_dom(|state| {
        let pos = state.document.nodes[parent_id]
            .children
            .iter()
            .position(|&c| c == nid)
            .unwrap_or(state.document.nodes[parent_id].children.len());
        for (offset, &id) in new_ids.iter().enumerate() {
            state.document.nodes[id].parent = Some(parent_id);
            state.document.nodes[parent_id]
                .children
                .insert(pos + offset, id);
        }
    });
    for &id in &new_ids {
        let obj = wrap_element(scope, id);
        execute_appended_script_if_needed(scope, obj.into(), id);
    }
}

fn after_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let new_ids = collect_insertion_nodes(scope, &args);
    let parent_id = match with_dom(|state| state.document.nodes[nid].parent) {
        Some(p) => p,
        None => return,
    };
    with_dom(|state| {
        let pos = state.document.nodes[parent_id]
            .children
            .iter()
            .position(|&c| c == nid)
            .unwrap_or(state.document.nodes[parent_id].children.len());
        for (offset, &id) in new_ids.iter().enumerate() {
            state.document.nodes[id].parent = Some(parent_id);
            state.document.nodes[parent_id]
                .children
                .insert(pos + 1 + offset, id);
        }
    });
    for &id in &new_ids {
        let obj = wrap_element(scope, id);
        execute_appended_script_if_needed(scope, obj.into(), id);
    }
}

fn replace_with_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let new_ids = collect_insertion_nodes(scope, &args);
    let parent_id = match with_dom(|state| state.document.nodes[nid].parent) {
        Some(p) => p,
        None => return,
    };
    with_dom(|state| {
        let pos = state.document.nodes[parent_id]
            .children
            .iter()
            .position(|&c| c == nid)
            .unwrap_or(state.document.nodes[parent_id].children.len());
        // Remove the element being replaced.
        state.document.nodes[parent_id]
            .children
            .retain(|&c| c != nid);
        state.document.nodes[nid].parent = None;
        // Insert replacements in its place.
        for (offset, &id) in new_ids.iter().enumerate() {
            state.document.nodes[id].parent = Some(parent_id);
            state.document.nodes[parent_id]
                .children
                .insert(pos + offset, id);
        }
    });
    for &id in &new_ids {
        let obj = wrap_element(scope, id);
        execute_appended_script_if_needed(scope, obj.into(), id);
    }
}

fn replace_children_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let this = args.this();
    let nid = match extract_node_id(scope, this.into()) {
        Some(n) => n,
        None => return,
    };
    let new_ids = collect_insertion_nodes(scope, &args);
    with_dom(|state| {
        let old: Vec<NodeId> = state.document.nodes[nid].children.clone();
        for child in old {
            state.document.nodes[child].parent = None;
        }
        state.document.nodes[nid].children.clear();
        for &id in &new_ids {
            state.document.nodes[id].parent = Some(nid);
            state.document.nodes[nid].children.push(id);
        }
    });
    for &id in &new_ids {
        let obj = wrap_element(scope, id);
        execute_appended_script_if_needed(scope, obj.into(), id);
    }
}

// ── base64 encoding/decoding ───────────────────────────────────────────────

fn get_btoa_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let input = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let encoded = base64::engine::general_purpose::STANDARD.encode(input);
    rv.set(v8_str(scope, &encoded).into());
}

fn get_atob_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let input = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    match base64::engine::general_purpose::STANDARD.decode(&input) {
        Ok(bytes) => {
            if let Ok(decoded) = String::from_utf8(bytes) {
                rv.set(v8_str(scope, &decoded).into());
            } else {
                rv.set_null();
            }
        }
        Err(_) => rv.set_null(),
    }
}

// ── window scroll callbacks ────────────────────────────────────────────────

fn window_scroll_to_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // scrollTo(x, y) or scrollTo({ top: y, left: x, behavior: 'smooth' })
    if args.length() >= 2 {
        // (x, y) form
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        eprintln!("window.scrollTo({}, {})", x, y);
    } else if args.length() == 1 {
        // Options object form
        let opts = args.get(0);
        if let Some(obj) = opts.to_object(scope) {
            let top = get_prop(scope, obj, "top")
                .and_then(|v| v.to_number(scope))
                .map(|n| n.value())
                .unwrap_or(0.0);
            let left = get_prop(scope, obj, "left")
                .and_then(|v| v.to_number(scope))
                .map(|n| n.value())
                .unwrap_or(0.0);
            let behavior = get_prop(scope, obj, "behavior")
                .and_then(|v| v.to_string(scope))
                .map(|s| s.to_rust_string_lossy(scope))
                .unwrap_or_else(|| "auto".to_string());
            eprintln!(
                "window.scrollTo({{ top: {}, left: {}, behavior: {} }})",
                top, left, behavior
            );
        }
    }
}

fn window_scroll_by_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // scrollBy(x, y) or scrollBy({ top: dy, left: dx, behavior: 'smooth' })
    if args.length() >= 2 {
        // (x, y) form
        let x = args
            .get(0)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        let y = args
            .get(1)
            .to_number(scope)
            .map(|n| n.value())
            .unwrap_or(0.0);
        eprintln!("window.scrollBy({}, {})", x, y);
    } else if args.length() == 1 {
        // Options object form
        let opts = args.get(0);
        if let Some(obj) = opts.to_object(scope) {
            let top = get_prop(scope, obj, "top")
                .and_then(|v| v.to_number(scope))
                .map(|n| n.value())
                .unwrap_or(0.0);
            let left = get_prop(scope, obj, "left")
                .and_then(|v| v.to_number(scope))
                .map(|n| n.value())
                .unwrap_or(0.0);
            let behavior = get_prop(scope, obj, "behavior")
                .and_then(|v| v.to_string(scope))
                .map(|s| s.to_rust_string_lossy(scope))
                .unwrap_or_else(|| "auto".to_string());
            eprintln!(
                "window.scrollBy({{ top: {}, left: {}, behavior: {} }})",
                top, left, behavior
            );
        }
    }
}

// ── matchMedia callback ──────────────────────────────────────────────────

fn match_media_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let query = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();

    // Create MediaQueryList object
    let mql = v8::Object::new(scope);

    // Evaluate common media queries
    let matches = evaluate_media_query(&query);
    set_bool(scope, mql, "matches", matches);
    set_str(scope, mql, "media", &query);

    // addListener/removeListener stubs
    set_fn(scope, mql, "addListener", noop);
    set_fn(scope, mql, "removeListener", noop);
    set_fn(scope, mql, "addEventListener", noop);
    set_fn(scope, mql, "removeEventListener", noop);
    set_fn(scope, mql, "dispatchEvent", dispatch_event_cb);

    rv.set(mql.into());
}

fn evaluate_media_query(query: &str) -> bool {
    let query = query.trim();

    // Handle common media queries with sensible defaults
    match query {
        // Common mobile/desktop breakpoints
        "(min-width: 0px)" | "(min-width:0px)" => true,
        "(max-width: 99999px)" | "(max-width:99999px)" => true,

        // Standard breakpoints - return sensible defaults
        "screen" | "all" | "print" | "speech" => true,
        "(prefers-color-scheme: light)" => true,
        "(prefers-color-scheme: dark)" => false,
        "(prefers-reduced-motion: reduce)" => false,
        "(prefers-reduced-motion: no-preference)" => true,
        "(hover: hover)" => true,
        "(hover: none)" => false,
        "(pointer: coarse)" => false,
        "(pointer: fine)" => true,
        "(any-pointer: coarse)" => false,
        "(any-pointer: fine)" => true,
        "(orientation: landscape)" => true, // Default to landscape/desktop
        "(orientation: portrait)" => false,

        // Resolution queries
        "(min-resolution: 1dppx)" | "(min-resolution: 96dpi)" => true,
        "(-webkit-min-device-pixel-ratio: 1)" => true,
        "(-webkit-min-device-pixel-ratio: 2)" => false, // Not retina by default

        // Default: be permissive and return true for unknown queries
        // This helps frameworks that test feature support
        _ => {
            // Check for common patterns
            if query.contains("min-width") {
                // Default to desktop viewport (1024px)
                true
            } else if query.contains("max-width") {
                // Assume we're not constrained by max-width
                false
            } else if query.contains("prefers-color-scheme") {
                // Default to light mode
                query.contains("light")
            } else {
                // Default permissive
                true
            }
        }
    }
}

// ── location callbacks ───────────────────────────────────────────────────

fn location_reload_cb(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // No-op reload - in a real browser this would reload the page
    eprintln!("location.reload() called");
}

fn location_replace_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // No-op replace - in a real browser this would navigate to new URL
    if args.length() > 0 {
        let url = args
            .get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();
        eprintln!("location.replace({})", url);
    }
}

fn location_assign_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // No-op assign - in a real browser this would navigate to new URL
    if args.length() > 0 {
        let url = args
            .get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();
        eprintln!("location.assign({})", url);
    }
}

// ── history callbacks ────────────────────────────────────────────────────

fn history_push_state_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // pushState(state, title, url) - adds history entry
    if args.length() >= 3 {
        let url = args
            .get(2)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();
        eprintln!("history.pushState(..., \"{}\")", url);
    } else {
        eprintln!("history.pushState()");
    }
}

fn history_replace_state_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    // replaceState(state, title, url) - replaces current history entry
    if args.length() >= 3 {
        let url = args
            .get(2)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default();
        eprintln!("history.replaceState(..., \"{}\")", url);
    } else {
        eprintln!("history.replaceState()");
    }
}

fn history_back_cb(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    eprintln!("history.back()");
}

fn history_forward_cb(
    _scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    eprintln!("history.forward()");
}

fn history_go_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let delta = args.get(0).int32_value(scope).unwrap_or(0);
    eprintln!("history.go({})", delta);
}

// ── getComputedStyle ──────────────────────────────────────────────────────

fn get_computed_style_cb(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let element_val = args.get(0);
    let nid = match extract_node_id(scope, element_val) {
        Some(n) => n,
        None => {
            // Return empty CSSStyleDeclaration object
            let style = v8::Object::new(scope);
            rv.set(style.into());
            return;
        }
    };

    // Create a CSSStyleDeclaration-like object
    let style = v8::Object::new(scope);

    // Get computed styles from the document
    let (computed, inline_style) = with_dom(|state| {
        if let Some(node) = state.document.nodes.get(nid) {
            match &node.data {
                NodeData::Element(el) => {
                    let inline = el.attributes.get("style").cloned().unwrap_or_default();
                    let computed = (
                        el.tag_name.clone(),
                        el.attributes.get("class").cloned().unwrap_or_default(),
                        el.attributes.get("id").cloned().unwrap_or_default(),
                    );
                    (Some(computed), inline)
                }
                _ => (None, String::new()),
            }
        } else {
            (None, String::new())
        }
    });

    // Parse inline styles into a HashMap
    let mut style_map: HashMap<String, String> = HashMap::new();
    if !inline_style.is_empty() {
        for decl in inline_style.split(';') {
            let decl = decl.trim();
            if decl.is_empty() {
                continue;
            }
            if let Some(colon_pos) = decl.find(':') {
                let prop = decl[..colon_pos].trim().to_lowercase();
                let val = decl[colon_pos + 1..].trim().to_string();
                style_map.insert(prop, val);
            }
        }
    }

    if let Some((tag, _class_attr, _id)) = computed {
        // Helper to get style value from inline styles or default
        let get_style = |prop: &str, default: &str| -> String {
            style_map
                .get(prop)
                .cloned()
                .unwrap_or_else(|| default.to_string())
        };

        // Set common CSS properties from inline styles or defaults
        set_str(scope, style, "display", &get_style("display", ""));
        set_str(
            scope,
            style,
            "visibility",
            &get_style("visibility", "visible"),
        );
        set_str(scope, style, "position", &get_style("position", "static"));
        set_str(scope, style, "width", &get_style("width", "auto"));
        set_str(scope, style, "height", &get_style("height", "auto"));
        set_str(scope, style, "margin", &get_style("margin", ""));
        set_str(scope, style, "marginTop", &get_style("margin-top", "0px"));
        set_str(
            scope,
            style,
            "marginRight",
            &get_style("margin-right", "0px"),
        );
        set_str(
            scope,
            style,
            "marginBottom",
            &get_style("margin-bottom", "0px"),
        );
        set_str(scope, style, "marginLeft", &get_style("margin-left", "0px"));
        set_str(scope, style, "padding", &get_style("padding", ""));
        set_str(scope, style, "paddingTop", &get_style("padding-top", "0px"));
        set_str(
            scope,
            style,
            "paddingRight",
            &get_style("padding-right", "0px"),
        );
        set_str(
            scope,
            style,
            "paddingBottom",
            &get_style("padding-bottom", "0px"),
        );
        set_str(
            scope,
            style,
            "paddingLeft",
            &get_style("padding-left", "0px"),
        );
        set_str(scope, style, "border", &get_style("border", ""));
        set_str(
            scope,
            style,
            "borderWidth",
            &get_style("border-width", "0px"),
        );
        set_str(
            scope,
            style,
            "borderStyle",
            &get_style("border-style", "none"),
        );
        set_str(scope, style, "borderColor", &get_style("border-color", ""));
        set_str(scope, style, "background", &get_style("background", ""));
        set_str(
            scope,
            style,
            "backgroundColor",
            &get_style("background-color", "transparent"),
        );
        set_str(
            scope,
            style,
            "backgroundImage",
            &get_style("background-image", "none"),
        );
        set_str(scope, style, "color", &get_style("color", "black"));
        set_str(scope, style, "font", &get_style("font", ""));
        set_str(
            scope,
            style,
            "fontFamily",
            &get_style("font-family", "serif"),
        );
        set_str(scope, style, "fontSize", &get_style("font-size", "16px"));
        set_str(
            scope,
            style,
            "fontWeight",
            &get_style("font-weight", "normal"),
        );
        set_str(
            scope,
            style,
            "fontStyle",
            &get_style("font-style", "normal"),
        );
        set_str(
            scope,
            style,
            "lineHeight",
            &get_style("line-height", "normal"),
        );
        set_str(scope, style, "textAlign", &get_style("text-align", "left"));
        set_str(
            scope,
            style,
            "textDecoration",
            &get_style("text-decoration", "none"),
        );
        set_str(
            scope,
            style,
            "whiteSpace",
            &get_style("white-space", "normal"),
        );
        set_str(scope, style, "overflow", &get_style("overflow", "visible"));
        set_str(scope, style, "float", &get_style("float", "none"));
        set_str(scope, style, "clear", &get_style("clear", "none"));
        set_str(scope, style, "zIndex", &get_style("z-index", "auto"));
        set_str(scope, style, "opacity", &get_style("opacity", "1"));
        set_str(scope, style, "top", &get_style("top", "auto"));
        set_str(scope, style, "right", &get_style("right", "auto"));
        set_str(scope, style, "bottom", &get_style("bottom", "auto"));
        set_str(scope, style, "left", &get_style("left", "auto"));
        set_str(scope, style, "cssText", &inline_style);
    }

    // Add getPropertyValue method
    fn get_property_value_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let prop_name = args
            .get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default()
            .to_lowercase();

        // Get the style object and look up the property
        // For now, return empty string - in a full implementation,
        // we'd need access to the resolved styles
        let this = args.this();
        if let Some(val) = get_prop(scope, this, &prop_name) {
            if let Some(s) = val.to_string(scope) {
                let str_val = s.to_rust_string_lossy(scope);
                rv.set(v8_str(scope, &str_val).into());
                return;
            }
        }
        rv.set(v8_str(scope, "").into());
    }
    set_fn(scope, style, "getPropertyValue", get_property_value_cb);

    // Add setProperty method (no-op for computed styles)
    set_fn(scope, style, "setProperty", noop);

    // Add removeProperty method (no-op for computed styles)
    set_fn(scope, style, "removeProperty", noop);

    // Add item method for array-like access
    fn item_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let _index = args.get(0).int32_value(scope).unwrap_or(0);
        // Return empty string for any index
        // Full implementation would return property name at index
        rv.set(v8_str(scope, "").into());
    }
    set_fn(scope, style, "item", item_cb);

    // Set length property
    set_int(scope, style, "length", 0);

    rv.set(style.into());
}

// ── install globals ──────────────────────────────────────────────────────

fn install_globals(scope: &mut v8::HandleScope, global: v8::Local<v8::Object>) {
    // document object (create first so element wrappers can reference it)
    let doc_obj = v8::Object::new(scope);
    set_document_obj(scope, doc_obj);

    // console
    let console = v8::Object::new(scope);
    set_fn(scope, console, "log", console_log);
    set_fn(scope, console, "warn", console_warn);
    set_fn(scope, console, "error", console_error);
    set_fn(scope, console, "info", console_info);
    set_fn(scope, console, "debug", console_debug);
    set_fn(scope, console, "trace", console_trace);
    set_fn(scope, console, "dir", console_dir);
    set_fn(scope, console, "table", console_table);
    set_fn(scope, console, "group", console_group);
    set_fn(scope, console, "groupEnd", console_group_end);
    set_fn(scope, console, "time", console_time);
    set_fn(scope, console, "timeEnd", console_time_end);
    set_fn(scope, console, "timeLog", console_time_log);
    set_fn(scope, console, "assert", console_assert);
    set_fn(scope, console, "clear", console_clear);
    set_fn(scope, console, "count", console_count);
    let ck = v8_str(scope, "console");
    global.set(scope, ck.into(), console.into());

    // document — populated now (obj created earlier and stored in thread-local)
    set_int(scope, doc_obj, "nodeType", 9);
    set_str(scope, doc_obj, "nodeName", "#document");
    set_fn(scope, doc_obj, "getElementById", get_element_by_id_cb);
    set_fn(scope, doc_obj, "createElement", create_element_cb);
    set_fn(scope, doc_obj, "createElementNS", create_element_cb);
    set_fn(scope, doc_obj, "createTextNode", create_text_node_cb);
    set_fn(scope, doc_obj, "querySelector", query_selector_cb);
    set_fn(scope, doc_obj, "querySelectorAll", query_selector_all_cb);
    set_fn(
        scope,
        doc_obj,
        "getElementsByTagName",
        get_elements_by_tag_name_cb,
    );
    set_fn(
        scope,
        doc_obj,
        "getElementsByClassName",
        get_elements_by_class_name_cb,
    );
    set_fn(scope, doc_obj, "getElementsByName", get_elements_by_name_cb);
    set_fn(
        scope,
        doc_obj,
        "addEventListener",
        global_add_event_listener_cb,
    );
    set_fn(
        scope,
        doc_obj,
        "removeEventListener",
        global_remove_event_listener_cb,
    );
    set_fn(scope, doc_obj, "createEvent", noop);
    set_fn(
        scope,
        doc_obj,
        "createDocumentFragment",
        create_document_fragment_cb,
    );
    set_fn(scope, doc_obj, "createComment", create_comment_cb);
    set_fn(scope, doc_obj, "importNode", import_node_cb);
    set_fn(scope, doc_obj, "adoptNode", adopt_node_cb);
    set_fn(scope, doc_obj, "createRange", create_range_cb);
    set_fn(scope, doc_obj, "dispatchEvent", dispatch_event_cb);
    set_fn(scope, doc_obj, "write", document_write_cb);
    set_fn(scope, doc_obj, "writeln", document_writeln_cb);

    // document.implementation — required by jQuery and some anti-bot scripts.
    let impl_obj = v8::Object::new(scope);
    set_fn(
        scope,
        impl_obj,
        "createHTMLDocument",
        create_html_document_cb,
    );
    set_fn(scope, impl_obj, "createDocument", create_html_document_cb);
    set_fn(scope, impl_obj, "hasFeature", noop_true);
    let impl_k = v8_str(scope, "implementation");
    doc_obj.set(scope, impl_k.into(), impl_obj.into());
    set_fn(scope, doc_obj, "execCommand", noop_false);
    set_fn(scope, doc_obj, "contains", noop_true);
    set_fn(scope, doc_obj, "hasFocus", noop_true);
    set_str(scope, doc_obj, "visibilityState", "visible");
    set_bool(scope, doc_obj, "hidden", false);
    set_str(scope, doc_obj, "readyState", "loading");
    set_str(scope, doc_obj, "title", "");
    set_str(scope, doc_obj, "domain", "");
    set_str(scope, doc_obj, "URL", "");
    set_str(scope, doc_obj, "documentURI", "");
    set_str(scope, doc_obj, "cookie", "");
    set_str(scope, doc_obj, "referrer", "");
    set_str(scope, doc_obj, "compatMode", "CSS1Compat");
    set_str(scope, doc_obj, "characterSet", "UTF-8");
    set_str(scope, doc_obj, "contentType", "text/html");
    // documentElement / body / head
    if let Some(html_id) = with_dom(|s| s.document.document_element()) {
        let el = wrap_element(scope, html_id);
        let k = v8_str(scope, "documentElement");
        doc_obj.set(scope, k.into(), el.into());
    }
    if let Some(body_id) = with_dom(|s| s.document.body()) {
        let el = wrap_element(scope, body_id);
        let k = v8_str(scope, "body");
        doc_obj.set(scope, k.into(), el.into());
    }
    // document.head — find <head> under <html>
    let head_id = with_dom(|s| {
        s.document.document_element().and_then(|html| {
            s.document.nodes[html].children.iter().copied().find(|&id| {
                matches!(&s.document.nodes[id].data,
                    NodeData::Element(ref e) if e.tag_name == "head")
            })
        })
    });
    if let Some(hid) = head_id {
        let el = wrap_element(scope, hid);
        let k = v8_str(scope, "head");
        doc_obj.set(scope, k.into(), el.into());
    }
    // document.fonts — stubbed font loading API used by Google and others.
    let fonts = v8::Object::new(scope);
    set_fn(scope, fonts, "load", noop_promise);
    set_fn(scope, fonts, "add", noop_promise);
    set_fn(scope, fonts, "delete", noop_true);
    set_str(scope, fonts, "status", "loaded");
    let fonts_ready = v8::Object::new(scope);
    set_fn(scope, fonts_ready, "then", noop);
    set_fn(scope, fonts_ready, "catch", noop);
    set_fn(scope, fonts_ready, "finally", noop);
    let ready_key = v8_str(scope, "ready");
    fonts.set(scope, ready_key.into(), fonts_ready.into());
    let fonts_key = v8_str(scope, "fonts");
    doc_obj.set(scope, fonts_key.into(), fonts.into());

    let dk = v8_str(scope, "document");
    global.set(scope, dk.into(), doc_obj.into());

    // window / self / globalThis already available; set self=window=globalThis
    let wk = v8_str(scope, "window");
    global.set(scope, wk.into(), global.into());
    let sk = v8_str(scope, "self");
    global.set(scope, sk.into(), global.into());
    // Microsoft sites (Bing/MSN) use `_w` and `_d` as aliases for window/document.
    // Expose them globally so those scripts can run before their own definitions.
    let _w_key = v8_str(scope, "_w");
    global.set(scope, _w_key.into(), global.into());
    let _d_key = v8_str(scope, "_d");
    global.set(scope, _d_key.into(), doc_obj.into());
    // window.top and window.parent must exist for cross-frame scripts (e.g. CBS
    // widgets set window.top.onmessage). Since we have no real frames, point them
    // back at the current window.
    let top_key = v8_str(scope, "top");
    global.set(scope, top_key.into(), global.into());
    let parent_key = v8_str(scope, "parent");
    global.set(scope, parent_key.into(), global.into());
    // window.frames is the window itself in real browsers; named frame lookups
    // fall through to global properties, which satisfies CMP stub loops that
    // scan parent.frames["__gppLocator"], etc.
    let frames_key = v8_str(scope, "frames");
    global.set(scope, frames_key.into(), global.into());
    // window.postMessage — many trackers and CMP frames expect this to exist.
    set_fn(scope, global, "postMessage", noop);

    // Provide a global noop helper; many minified WordPress bundles assume an
    // inline utility named noop exists and crash when an earlier error prevents
    // it from being declared.
    set_fn(scope, global, "noop", noop);

    // chrome object (for anti-bot evasion)
    let chrome = v8::Object::new(scope);
    set_fn(scope, chrome, "loadTimes", noop_obj);
    set_fn(scope, chrome, "csi", noop_obj);
    let ck = v8_str(scope, "chrome");
    global.set(scope, ck.into(), chrome.into());

    // Prevent automation detection
    set_bool(scope, global, "cdc_adoQpoasnfa76pfcZLmcfl_", false);
    set_bool(scope, global, "cdc_adoQpoasnfa76pfcZLmcfl_Hash", false);

    // navigator
    let nav = v8::Object::new(scope);
    // More realistic user agent
    set_str(
        scope,
        nav,
        "userAgent",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    );
    set_str(scope, nav, "language", "en-US");
    let languages = v8::Array::new(scope, 2);
    let lang0 = v8_str(scope, "en-US");
    let lang1 = v8_str(scope, "en");
    languages.set_index(scope, 0, lang0.into());
    languages.set_index(scope, 1, lang1.into());
    let languages_key = v8_str(scope, "languages");
    nav.set(scope, languages_key.into(), languages.into());
    set_str(scope, nav, "platform", "Linux x86_64");
    set_bool(scope, nav, "cookieEnabled", true);
    set_bool(scope, nav, "onLine", true);
    set_int(scope, nav, "hardwareConcurrency", 8);
    set_str(scope, nav, "appName", "Netscape");
    set_str(scope, nav, "appVersion", "5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36");
    set_str(scope, nav, "vendor", "Google Inc.");
    set_str(scope, nav, "product", "Gecko");
    set_str(scope, nav, "productSub", "20030107");
    set_str(scope, nav, "doNotTrack", "unspecified");
    set_fn(scope, nav, "sendBeacon", noop_false);
    set_fn(scope, nav, "javaEnabled", noop_false);
    // webdriver detection evasion
    set_bool(scope, nav, "webdriver", false);
    // plugins - empty array (real Chrome has PDF plugin etc)
    let plugins = v8::Array::new(scope, 0);
    let pk = v8_str(scope, "plugins");
    nav.set(scope, pk.into(), plugins.into());
    let mime_types = v8::Array::new(scope, 0);
    let mk = v8_str(scope, "mimeTypes");
    nav.set(scope, mk.into(), mime_types.into());
    // navigator.serviceWorker stub (used by Bing/MSN progressive-web apps).
    let service_worker = v8::Object::new(scope);
    // Use a no-op controller object instead of null so scripts that call
    // navigator.serviceWorker.controller.postMessage(...) don't throw.
    let sw_controller = v8::Object::new(scope);
    set_fn(scope, sw_controller, "postMessage", noop);
    set_fn(scope, sw_controller, "addEventListener", noop);
    set_fn(scope, sw_controller, "removeEventListener", noop);
    let controller_key = v8_str(scope, "controller");
    service_worker.set(scope, controller_key.into(), sw_controller.into());
    set_fn(scope, service_worker, "register", noop_promise);
    set_fn(scope, service_worker, "getRegistration", noop_promise);
    set_fn(scope, service_worker, "getRegistrations", noop_promise_arr);
    set_fn(scope, service_worker, "addEventListener", noop);
    set_fn(scope, service_worker, "removeEventListener", noop);
    let sw_ready = v8::Object::new(scope);
    set_fn(scope, sw_ready, "then", noop);
    set_fn(scope, sw_ready, "catch", noop);
    set_fn(scope, sw_ready, "finally", noop);
    let sw_ready_key = v8_str(scope, "ready");
    service_worker.set(scope, sw_ready_key.into(), sw_ready.into());
    let sw_key = v8_str(scope, "serviceWorker");
    nav.set(scope, sw_key.into(), service_worker.into());
    let nk = v8_str(scope, "navigator");
    global.set(scope, nk.into(), nav.into());

    // location
    let loc = v8::Object::new(scope);
    set_str(scope, loc, "href", "");
    set_str(scope, loc, "hostname", "");
    set_str(scope, loc, "pathname", "/");
    set_str(scope, loc, "search", "");
    set_str(scope, loc, "hash", "");
    set_str(scope, loc, "protocol", "https:");
    set_str(scope, loc, "origin", "");
    set_str(scope, loc, "host", "");
    set_str(scope, loc, "port", "");
    set_fn(scope, loc, "reload", location_reload_cb);
    set_fn(scope, loc, "replace", location_replace_cb);
    set_fn(scope, loc, "assign", location_assign_cb);
    let lk = v8_str(scope, "location");
    global.set(scope, lk.into(), loc.into());

    // history
    let hist = v8::Object::new(scope);
    set_fn(scope, hist, "pushState", history_push_state_cb);
    set_fn(scope, hist, "replaceState", history_replace_state_cb);
    set_fn(scope, hist, "back", history_back_cb);
    set_fn(scope, hist, "forward", history_forward_cb);
    set_fn(scope, hist, "go", history_go_cb);
    set_int(scope, hist, "length", 1);
    let hk = v8_str(scope, "history");
    global.set(scope, hk.into(), hist.into());

    // screen
    let screen = v8::Object::new(scope);
    set_int(scope, screen, "width", 1920);
    set_int(scope, screen, "height", 1080);
    set_int(scope, screen, "availWidth", 1920);
    set_int(scope, screen, "availHeight", 1080);
    set_int(scope, screen, "colorDepth", 24);
    set_int(scope, screen, "pixelDepth", 24);
    let sck = v8_str(scope, "screen");
    global.set(scope, sck.into(), screen.into());

    // crypto - Web Crypto API stub
    let crypto = v8::Object::new(scope);
    fn crypto_get_random_values(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        // Fill typed array with random values
        if args.length() > 0 {
            let arr = args.get(0);
            if let Ok(ta) = v8::Local::<v8::Uint8Array>::try_from(arr) {
                let len = ta.byte_length();
                for i in 0..len {
                    let rand_val = (js_rand() * 256.0) as u8;
                    let val = v8::Integer::new_from_unsigned(scope, rand_val as u32);
                    ta.set_index(scope, i as u32, val.into());
                }
            }
            rv.set(arr);
        }
    }
    fn crypto_random_uuid(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        // Generate UUID v4
        let mut bytes = [0u8; 16];
        for i in 0..16 {
            bytes[i] = (js_rand() * 256.0) as u8;
        }
        // Set version (4) and variant bits
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let uuid = format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15]
        );
        rv.set(v8_str(scope, &uuid).into());
    }
    set_fn(scope, crypto, "getRandomValues", crypto_get_random_values);
    set_fn(scope, crypto, "randomUUID", crypto_random_uuid);
    // crypto.subtle stub
    let subtle = v8::Object::new(scope);
    set_fn(scope, subtle, "digest", noop_promise);
    set_fn(scope, subtle, "generateKey", noop_promise);
    set_fn(scope, subtle, "importKey", noop_promise);
    set_fn(scope, subtle, "exportKey", noop_promise);
    set_fn(scope, subtle, "sign", noop_promise);
    set_fn(scope, subtle, "verify", noop_promise);
    set_fn(scope, subtle, "encrypt", noop_promise);
    set_fn(scope, subtle, "decrypt", noop_promise);
    let subk = v8_str(scope, "subtle");
    crypto.set(scope, subk.into(), subtle.into());
    let crk = v8_str(scope, "crypto");
    global.set(scope, crk.into(), crypto.into());

    // innerWidth, innerHeight, scrollX/Y, devicePixelRatio
    set_int(scope, global, "innerWidth", 1024);
    set_int(scope, global, "innerHeight", 768);
    set_int(scope, global, "outerWidth", 1024);
    set_int(scope, global, "outerHeight", 768);
    set_int(scope, global, "scrollX", 0);
    set_int(scope, global, "scrollY", 0);
    set_int(scope, global, "pageXOffset", 0);
    set_int(scope, global, "pageYOffset", 0);
    {
        let k = v8_str(scope, "devicePixelRatio");
        let v = v8::Number::new(scope, 1.0);
        global.set(scope, k.into(), v.into());
    }

    // Minimal jQuery/$ stub for WordPress and other sites that assume it exists.
    // Defer callbacks via setTimeout to avoid deep synchronous recursion, and
    // expose enough of the jQuery/Sizzle surface to satisfy jquery-migrate and
    // common WordPress boot scripts. Real jQuery may overwrite this later; the
    // stub is intentionally defensive so partial failures still leave a usable $.
    let jq_stub = v8_str(
        scope,
        r#"
        (function() {
            var chain = function() { return $; };
            var arr = function() { return []; };
            var num = function() { return 0; };
            var deferred = function() {
                return {
                    done: chain, fail: chain, always: chain, then: chain, catch: chain,
                    promise: function() { return this; },
                    resolve: chain, reject: chain, notify: chain,
                    state: function() { return 'resolved'; },
                    isResolved: function() { return true; },
                    isRejected: function() { return false; }
                };
            };
            var $ = function(selector, context) {
                if (typeof selector === 'function') {
                    try { setTimeout(selector, 0); } catch(e) {}
                    return $;
                }
                if (typeof selector === 'object' && selector && selector.nodeType) {
                    // Wrapping a DOM node should still chain.
                    return $;
                }
                return $;
            };
            $.fn = {
                length: 0,
                jquery: '3.7.0',
                pseudos: {},
                expr: {},
                find: chain,
                filter: chain,
                map: arr,
                each: chain,
                addClass: chain,
                removeClass: chain,
                toggleClass: chain,
                hasClass: function() { return false; },
                attr: function(v) { return typeof v === 'undefined' ? '' : $; },
                prop: function(v) { return typeof v === 'undefined' ? undefined : $; },
                css: function(v) { return typeof v === 'undefined' ? '' : $; },
                on: chain,
                off: chain,
                click: chain,
                trigger: chain,
                parent: chain,
                parents: chain,
                children: chain,
                siblings: chain,
                append: chain,
                prepend: chain,
                remove: chain,
                empty: chain,
                html: function(v) { return typeof v === 'undefined' ? '' : $; },
                text: function(v) { return typeof v === 'undefined' ? '' : $; },
                show: chain,
                hide: chain,
                fadeIn: chain,
                fadeOut: chain,
                slideUp: chain,
                slideDown: chain,
                width: num,
                height: num,
                outerWidth: num,
                outerHeight: num,
                offset: function() { return { top:0, left:0 }; },
                position: function() { return { top:0, left:0 }; },
                scrollTop: num,
                val: function(v) { return typeof v === 'undefined' ? '' : $; },
                data: function(k, v) { return typeof v === 'undefined' ? undefined : $; },
                focus: chain,
                blur: chain,
                submit: chain,
                serialize: function() { return ''; },
                ready: function(cb) { try { setTimeout(cb, 0); } catch(e) {} }
            };
            $.expr = $.fn.expr = { ':': {}, filter: {} };
            $.expr.filters = $.expr[':'];
            $.ready = function(cb) { try { setTimeout(cb, 0); } catch(e) {} };
            $.noConflict = function() { return $; };
            $.extend = function() {
                var target = arguments[0] || {};
                for (var i = 1; i < arguments.length; i++) {
                    var src = arguments[i];
                    if (!src) continue;
                    for (var k in src) { target[k] = src[k]; }
                }
                return target;
            };
            $.ajax = function() { return { done: chain, fail: chain, always: chain, then: chain, catch: chain, abort: chain }; };
            $.get = $.post = $.load = $.getJSON = $.getScript = function() { return $.ajax(); };
            $.Deferred = deferred;
            $.when = function() { return deferred(); };
            $.ajaxPrefilter = function() {};
            $.ajaxSetup = function() {};
            $.ajaxTransport = function() {};
            $.param = function() { return ''; };
            $.isArray = Array.isArray;
            $.isFunction = function(v) { return typeof v === 'function'; };
            $.isPlainObject = function(v) { return v && typeof v === 'object'; };
            $.isEmptyObject = function(v) { return !v || Object.keys(v).length === 0; };
            $.isNumeric = function(v) { return !isNaN(parseFloat(v)) && isFinite(v); };
            $.type = function(v) {
                if (v == null) return String(v);
                return typeof v === 'object' ? 'object' : typeof v;
            };
            $.trim = function(s) { return String(s).replace(/^\s+|\s+$/g, ''); };
            $.parseHTML = function(html) {
                var d = document.implementation.createHTMLDocument('');
                var el = d.createElement('div');
                el.innerHTML = String(html || '');
                return Array.from(el.childNodes || []);
            };
            $.parseJSON = JSON.parse;
            $.holdReady = function() {};
            $.uniqueSort = function(a) { return a; };
            $.contains = function() { return false; };
            $.escapeSelector = function(s) { return String(s).replace(/[\!\"\#\$\%\&\'\(\)\*\+\,\.\/\:\;\<\=\>\?\@\[\\\]\^\`\{\|\}\~]/g, '\\$&'); };
            $.now = function() { return Date.now(); };
            $.inArray = function(v, a) { return Array.isArray(a) ? a.indexOf(v) : -1; };
            $.merge = function(a, b) { return a; };
            $.grep = function(a, fn) { return Array.isArray(a) ? a.filter(fn) : []; };
            $.map = function(a, fn) { return Array.isArray(a) ? a.map(fn) : []; };
            $.makeArray = function(a) { return Array.isArray(a) ? a : []; };
            $.queue = chain;
            $.dequeue = chain;
            $.clearQueue = chain;
            $.data = function() { return undefined; };
            $.removeData = function() {};
            $.hasData = function() { return false; };
            $.fx = { tick: noop, timer: function() {}, stop: function() {}, speeds: {}, off: false, interval: 13 };
            $.support = {};
            $.Tween = function() { return chain; };
            $.easing = {};
            $.Event = function(type) { return { type: type, preventDefault: function() {}, stopPropagation: function() {} }; };
            var staticMethods = ['addClass','removeClass','toggleClass','hasClass','attr','prop','css','on','off','click','each','map','filter','find','parent','parents','children','siblings','append','prepend','remove','empty','html','text','show','hide','fadeIn','fadeOut','slideUp','slideDown','width','height','outerWidth','outerHeight','offset','position','scrollTop','val','data','trigger','focus','blur','submit','serialize','ajax','get','post','load','getJSON','getScript','ready','onDocumentReady','noConflict','extend','Deferred','when','ajaxPrefilter','ajaxSetup','param','isArray','isFunction','isPlainObject','isEmptyObject','isNumeric','type','trim','parseHTML','parseJSON','holdReady','uniqueSort','contains','escapeSelector','now','inArray','merge','grep','map','makeArray','queue','dequeue','clearQueue','removeData','hasData','support','easing','Event'];
            for (var i = 0; i < staticMethods.length; i++) {
                var m = staticMethods[i];
                if (!$[m]) $[m] = $.fn[m] || chain;
            }
            window.$ = window.jQuery = $;
        })();
    "#,
    );
    // jQuery stub intentionally disabled on this branch: real bundled jQuery
    // (e.g. CBS via RequireJS) was being shadowed by the stub, causing Sizzle
    // to receive a broken window/document and throw null.nodeType.
    // Sites without their own jQuery can rely on the native DOM APIs instead.
    // if let Some(jq_script) = v8::Script::compile(scope, jq_stub, None) {
    //     let _ = jq_script.run(scope);
    // }

    // Minimal WordPress wp stub to satisfy scripts that call wp.data.use etc.
    let wp_stub = v8_str(
        scope,
        r#"
        (function() {
            if (typeof window.wp !== 'undefined') return;
            var nope = function() { return undefined; };
            var noop = function() {};
            var fakeStore = {
                getState: function() { return {}; },
                dispatch: noop,
                subscribe: noop,
                select: function() { return {}; }
            };
            var fakeRegistry = {
                registerStore: function() { return fakeStore; },
                registerGenericStore: noop,
                subscribe: noop,
                select: function() { return {}; },
                dispatch: noop
            };
            var dataUse = function() {
                // Return a registry-like object with a persistence property that
                // WordPress data plugins sometimes access.
                return {
                    getState: function() { return {}; },
                    dispatch: noop,
                    subscribe: noop,
                    select: function() { return {}; },
                    persistence: {}
                };
            };
            window.wp = {
                data: {
                    use: dataUse,
                    useSelect: function() { return {}; },
                    useDispatch: function() { return noop; },
                    useRegistry: function() { return fakeRegistry; },
                    registerStore: function() { return fakeStore; },
                    registerGenericStore: noop,
                    createRegistry: function() { return fakeRegistry; },
                    subscribe: noop,
                    select: function() { return {}; },
                    dispatch: noop,
                    resolveSelect: function() { return Promise.resolve({}); }
                },
                i18n: { __: function(s) { return s; }, _x: function(s) { return s; }, sprintf: function(s) { return s; }, setLocaleData: noop },
                hooks: { addAction: noop, addFilter: noop, removeAction: noop, removeFilter: noop, doAction: noop, applyFilters: function(v) { return v; }, createHooks: function() { return window.wp.hooks; } },
                domReady: noop,
                apiFetch: function() { return Promise.resolve({}); },
                url: { addQueryArgs: function(url) { return url; } }
            };
        })();
    "#,
    );
    if let Some(wp_script) = v8::Script::compile(scope, wp_stub, None) {
        let _ = wp_script.run(scope);
    }

    // React expects a global Scheduler with callback scheduling helpers. Provide a
    // minimal implementation so React DOM bundles can load without throwing.
    let scheduler_stub = v8_str(
        scope,
        r#"
        (function() {
            var noop = function() {};
            var callbacks = [];
            window.Scheduler = {
                unstable_ImmediatePriority: 1,
                unstable_UserBlockingPriority: 2,
                unstable_NormalPriority: 3,
                unstable_LowPriority: 4,
                unstable_IdlePriority: 5,
                unstable_scheduleCallback: function(priority, cb) {
                    try { setTimeout(cb, 0); } catch(e) {}
                    return { node: cb };
                },
                unstable_cancelCallback: function(task) { task.node = null; },
                unstable_runWithPriority: function(priority, fn) { if (fn) fn(); },
                unstable_shouldYield: function() { return false; },
                unstable_getCurrentPriorityLevel: function() { return 3; },
                unstable_now: function() { return performance.now(); },
                unstable_flushAll: noop,
                unstable_flushUntilNextPaint: noop,
                unstable_advanceTime: noop,
                unstable_forceFrameRate: noop,
                unstable_continueExecution: noop,
                unstable_pauseExecution: noop,
                unstable_requestPaint: noop,
                unstable_clearYields: noop,
                unstable_getFirstCallbackNode: function() { return null; },
                unstable_next: function(cb) { try { setTimeout(cb, 0); } catch(e) {} }
            };
            // Some bundles look for a UMD-style scheduler global.
            window.scheduler = window.Scheduler;
        })();
    "#,
    );
    if let Some(sch_script) = v8::Script::compile(scope, scheduler_stub, None) {
        let _ = sch_script.run(scope);
    }

    // addEventListener/removeEventListener on window
    set_fn(
        scope,
        global,
        "addEventListener",
        global_add_event_listener_cb,
    );
    set_fn(
        scope,
        global,
        "removeEventListener",
        global_remove_event_listener_cb,
    );
    set_fn(scope, global, "dispatchEvent", noop);
    set_fn(scope, global, "scrollTo", window_scroll_to_cb);
    set_fn(scope, global, "scrollBy", window_scroll_by_cb);
    set_fn(scope, global, "scroll", window_scroll_to_cb); // alias
    set_fn(scope, global, "scroll", noop);
    set_fn(scope, global, "alert", noop);
    set_fn(scope, global, "confirm", noop_false);
    set_fn(scope, global, "prompt", noop_null);
    set_fn(scope, global, "getComputedStyle", get_computed_style_cb);
    set_fn(scope, global, "matchMedia", match_media_cb);
    set_fn(
        scope,
        global,
        "requestAnimationFrame",
        request_animation_frame_cb,
    );
    set_fn(
        scope,
        global,
        "cancelAnimationFrame",
        cancel_animation_frame_cb,
    );
    set_fn(scope, global, "setTimeout", set_timeout_cb);
    set_fn(scope, global, "clearTimeout", noop);
    set_fn(scope, global, "setInterval", set_timeout_cb);
    set_fn(scope, global, "clearInterval", noop);
    set_fn(scope, global, "queueMicrotask", queue_microtask_cb);
    set_fn(scope, global, "fetch", fetch_cb);
    set_fn(scope, global, "btoa", get_btoa_cb);
    set_fn(scope, global, "atob", get_atob_cb);

    // customElements - minimal stub so custom-element registrations don't throw.
    // We don't actually upgrade elements, but define() accepts the call.
    fn custom_elements_define_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        // No-op: ignore custom element definitions.
    }
    fn custom_elements_get_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        rv.set(v8::undefined(scope).into());
    }
    fn custom_elements_when_defined_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        // Return a Promise that never resolves (undefined).
        rv.set(v8::undefined(scope).into());
    }
    let custom_elements = v8::Object::new(scope);
    set_fn(scope, custom_elements, "define", custom_elements_define_cb);
    set_fn(scope, custom_elements, "get", custom_elements_get_cb);
    set_fn(
        scope,
        custom_elements,
        "whenDefined",
        custom_elements_when_defined_cb,
    );
    let ce_key = v8_str(scope, "customElements");
    global.set(scope, ce_key.into(), custom_elements.into());

    // screen object
    let screen = v8::Object::new(scope);
    set_int(scope, screen, "width", 1024);
    set_int(scope, screen, "height", 768);
    set_int(scope, screen, "availWidth", 1024);
    set_int(scope, screen, "availHeight", 768);
    set_int(scope, screen, "colorDepth", 24);
    set_int(scope, screen, "pixelDepth", 24);
    let screen_key = v8_str(scope, "screen");
    global.set(scope, screen_key.into(), screen.into());

    // location object
    let location = v8::Object::new(scope);
    set_str(scope, location, "href", "https://example.com/");
    set_str(scope, location, "protocol", "https:");
    set_str(scope, location, "host", "example.com");
    set_str(scope, location, "hostname", "example.com");
    set_str(scope, location, "port", "");
    set_str(scope, location, "pathname", "/");
    set_str(scope, location, "search", "");
    set_str(scope, location, "hash", "");
    // location methods
    // location methods moved to module level below
    let loc_key = v8_str(scope, "location");
    global.set(scope, loc_key.into(), location.into());

    // localStorage - in-memory storage (persists for session only)
    let local_storage = v8::Object::new(scope);
    set_fn(scope, local_storage, "getItem", local_storage_get_item);
    set_fn(scope, local_storage, "setItem", local_storage_set_item);
    set_fn(
        scope,
        local_storage,
        "removeItem",
        local_storage_remove_item,
    );
    set_fn(scope, local_storage, "clear", local_storage_clear);
    set_fn(scope, local_storage, "key", local_storage_key);
    set_fn(scope, local_storage, "length", local_storage_length);
    let ls_key = v8_str(scope, "localStorage");
    global.set(scope, ls_key.into(), local_storage.into());

    // sessionStorage - in-memory storage (cleared on page close)
    let session_storage = v8::Object::new(scope);
    set_fn(scope, session_storage, "getItem", session_storage_get_item);
    set_fn(scope, session_storage, "setItem", session_storage_set_item);
    set_fn(
        scope,
        session_storage,
        "removeItem",
        session_storage_remove_item,
    );
    set_fn(scope, session_storage, "clear", session_storage_clear);
    set_fn(scope, session_storage, "key", session_storage_key);
    set_fn(scope, session_storage, "length", session_storage_length);
    let ss_key = v8_str(scope, "sessionStorage");
    global.set(scope, ss_key.into(), session_storage.into());

    /// Fire an XMLHttpRequest listener registered via `addEventListener` or the
    /// legacy `onload` / `onreadystatechange` properties.
    fn fire_xhr_event(scope: &mut v8::HandleScope, this: v8::Local<v8::Object>, event: &str) {
        let storage_key = match event {
            "load" => "__load_cb",
            "error" => "__error_cb",
            "readystatechange" => "__readystatechange_cb",
            _ => return,
        };
        let cb_key = v8_str(scope, storage_key);
        if let Some(v) = this.get(scope, cb_key.into()) {
            if let Ok(cb) = v8::Local::<v8::Function>::try_from(v) {
                let undef = v8::undefined(scope).into();
                let tc = &mut v8::TryCatch::new(scope);
                let _ = cb.call(tc, undef, &[]);
            }
        }
        let prop_key_str = format!("on{}", event);
        let prop_key = v8_str(scope, &prop_key_str);
        if let Some(v) = this.get(scope, prop_key.into()) {
            if let Ok(cb) = v8::Local::<v8::Function>::try_from(v) {
                let undef = v8::undefined(scope).into();
                let tc = &mut v8::TryCatch::new(scope);
                let _ = cb.call(tc, undef, &[]);
            }
        }
    }

    // XMLHttpRequest - real implementation for GET
    fn xhr_ctor(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        set_int(scope, obj, "readyState", 0);
        set_int(scope, obj, "status", 0);
        set_str(scope, obj, "statusText", "");
        set_str(scope, obj, "responseText", "");
        set_str(scope, obj, "response", "");
        set_str(scope, obj, "responseType", "");
        set_bool(scope, obj, "withCredentials", false);
        set_str(scope, obj, "__method", "GET");
        set_str(scope, obj, "__url", "");
        set_fn(scope, obj, "open", xhr_open_cb);
        set_fn(scope, obj, "send", xhr_send_cb);
        set_fn(scope, obj, "setRequestHeader", noop);
        set_fn(scope, obj, "getResponseHeader", xhr_get_response_header_cb);
        set_fn(
            scope,
            obj,
            "getAllResponseHeaders",
            xhr_get_all_response_headers_cb,
        );
        set_fn(scope, obj, "abort", noop);
        set_fn(scope, obj, "addEventListener", xhr_add_event_listener_cb);
        rv.set(obj.into());
    }
    fn xhr_open_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let method = args.get(0).to_rust_string_lossy(scope);
        let url = args.get(1).to_rust_string_lossy(scope);
        let method_key = v8_str(scope, "__method");
        let method_val = v8_str(scope, &method);
        this.set(scope, method_key.into(), method_val.into());
        let url_key = v8_str(scope, "__url");
        let url_val = v8_str(scope, &url);
        this.set(scope, url_key.into(), url_val.into());
        set_int(scope, this, "readyState", 1);
    }
    fn xhr_send_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let url_key = v8_str(scope, "__url");
        let url = match this.get(scope, url_key.into()) {
            Some(v) => match v.to_string(scope) {
                Some(s) => s.to_rust_string_lossy(scope),
                None => String::new(),
            },
            None => String::new(),
        };
        if url.is_empty() {
            set_int(scope, this, "status", 0);
            set_int(scope, this, "readyState", 4);
            return;
        }
        // Resolve relative URLs against window.location.href, falling back to
        // the page base URL so path-only requests don't become local file reads.
        let resolved_url = {
            let context = scope.get_current_context();
            let global = context.global(scope);
            let loc_key = v8_str(scope, "location");
            let loc_url = global
                .get(scope, loc_key.into())
                .and_then(|v| v.to_object(scope))
                .and_then(|loc| {
                    let href_key = v8_str(scope, "href");
                    loc.get(scope, href_key.into())
                })
                .and_then(|v| v.to_string(scope))
                .map(|s| s.to_rust_string_lossy(scope));
            let base_url = loc_url.or_else(|| page_base_url()).unwrap_or_default();
            if base_url.is_empty() {
                url.clone()
            } else {
                incognidium_net::resolve_url(&base_url, &url).unwrap_or_else(|_| url.clone())
            }
        };
        match incognidium_net::fetch_url(&resolved_url) {
            Ok(resp) => {
                eprintln!(
                    "[xhr OK] {} -> {} ({} bytes)",
                    resolved_url,
                    resp.status,
                    resp.body.len()
                );
                set_int(scope, this, "status", resp.status as i32);
                set_str(scope, this, "responseText", &resp.body);
                set_str(scope, this, "response", &resp.body);
                set_int(scope, this, "readyState", 4);
                // Fire registered event listeners and on* handlers.
                fire_xhr_event(scope, this, "load");
                fire_xhr_event(scope, this, "readystatechange");
            }
            Err(e) => {
                eprintln!("[xhr ERR] {} -> {}", resolved_url, e);
                set_int(scope, this, "status", 0);
                set_str(scope, this, "responseText", "");
                set_str(scope, this, "response", "");
                set_int(scope, this, "readyState", 4);
                fire_xhr_event(scope, this, "error");
                fire_xhr_event(scope, this, "readystatechange");
            }
        }
    }
    fn xhr_get_response_header_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        rv.set_null();
    }
    fn xhr_get_all_response_headers_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        rv.set(v8::String::new(scope, "").unwrap().into());
    }
    fn xhr_add_event_listener_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let event = args.get(0).to_rust_string_lossy(scope);
        let cb = args.get(1);
        if let Ok(func) = v8::Local::<v8::Function>::try_from(cb) {
            let key = match event.as_str() {
                "load" => Some("__load_cb"),
                "readystatechange" => Some("__readystatechange_cb"),
                _ => None,
            };
            if let Some(k) = key {
                let cb_key = v8_str(scope, k);
                this.set(scope, cb_key.into(), func.into());
            }
        }
    }
    let xhr_key = v8_str(scope, "XMLHttpRequest");
    let xhr_tmpl = v8::FunctionTemplate::new(scope, xhr_ctor);
    let xhr_fn = xhr_tmpl.get_function(scope).unwrap();
    global.set(scope, xhr_key.into(), xhr_fn.into());

    // performance
    let perf = v8::Object::new(scope);
    set_fn(scope, perf, "now", performance_now_cb);
    set_fn(scope, perf, "mark", performance_mark_cb);
    set_fn(scope, perf, "measure", performance_measure_cb);
    set_fn(scope, perf, "clearMarks", performance_clear_marks_cb);
    set_fn(scope, perf, "clearMeasures", performance_clear_measures_cb);
    set_fn(
        scope,
        perf,
        "getEntriesByName",
        performance_get_entries_by_name_cb,
    );
    set_fn(
        scope,
        perf,
        "getEntriesByType",
        performance_get_entries_by_type_cb,
    );
    set_fn(scope, perf, "getEntries", performance_get_entries_cb);
    // Timing properties
    let nav_start = v8::Number::new(scope, 0.0);
    let t_key = v8_str(scope, "timing");
    let timing = v8::Object::new(scope);
    set_num(scope, timing, "navigationStart", 0.0);
    set_num(scope, timing, "unloadEventStart", 0.0);
    set_num(scope, timing, "unloadEventEnd", 0.0);
    set_num(scope, timing, "redirectStart", 0.0);
    set_num(scope, timing, "redirectEnd", 0.0);
    set_num(scope, timing, "fetchStart", 0.0);
    set_num(scope, timing, "domainLookupStart", 0.0);
    set_num(scope, timing, "domainLookupEnd", 0.0);
    set_num(scope, timing, "connectStart", 0.0);
    set_num(scope, timing, "connectEnd", 0.0);
    set_num(scope, timing, "secureConnectionStart", 0.0);
    set_num(scope, timing, "requestStart", 0.0);
    set_num(scope, timing, "responseStart", 0.0);
    set_num(scope, timing, "responseEnd", 0.0);
    set_num(scope, timing, "domLoading", 0.0);
    set_num(scope, timing, "domInteractive", 0.0);
    set_num(scope, timing, "domContentLoadedEventStart", 0.0);
    set_num(scope, timing, "domContentLoadedEventEnd", 0.0);
    set_num(scope, timing, "domComplete", 0.0);
    set_num(scope, timing, "loadEventStart", 0.0);
    set_num(scope, timing, "loadEventEnd", 0.0);
    perf.set(scope, t_key.into(), timing.into());

    // performance.memory (Chrome-only, but widely used)
    let memory = v8::Object::new(scope);
    set_num(scope, memory, "usedJSHeapSize", 0.0);
    set_num(scope, memory, "totalJSHeapSize", 0.0);
    set_num(scope, memory, "jsHeapSizeLimit", 2190000000.0); // ~2GB
    let mk = v8_str(scope, "memory");
    perf.set(scope, mk.into(), memory.into());

    let pk = v8_str(scope, "performance");
    global.set(scope, pk.into(), perf.into());

    // ── DOM constructors (commonly referenced as globals: typeof Element, instanceof Node, etc.) ──
    fn observer_ctor(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        set_fn(scope, obj, "observe", noop);
        set_fn(scope, obj, "unobserve", noop);
        set_fn(scope, obj, "disconnect", noop);
        set_fn(scope, obj, "takeRecords", noop_empty_arr);
        rv.set(obj.into());
    }
    fn empty_ctor(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        rv.set(v8::Object::new(scope).into());
    }
    let dom_ctors = [
        "MutationObserver",
        "IntersectionObserver",
        "ResizeObserver",
        "ReportingObserver",
    ];
    for n in dom_ctors {
        let key = v8_str(scope, n);
        let tmpl = v8::FunctionTemplate::new(scope, observer_ctor);
        let f = tmpl.get_function(scope).unwrap();
        global.set(scope, key.into(), f.into());
    }

    // PerformanceObserver — needs a static `supportedEntryTypes` array.
    fn performance_observer_ctor(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        set_fn(scope, obj, "observe", noop);
        set_fn(scope, obj, "disconnect", noop);
        set_fn(scope, obj, "takeRecords", noop_empty_arr);
        rv.set(obj.into());
    }
    let po_key = v8_str(scope, "PerformanceObserver");
    let po_tmpl = v8::FunctionTemplate::new(scope, performance_observer_ctor);
    let po_fn = po_tmpl.get_function(scope).unwrap();
    let supported_types = v8::Array::new(scope, 3);
    let supported_vals = ["navigation", "resource", "mark"];
    for (i, val) in supported_vals.iter().enumerate() {
        let s = v8_str(scope, val);
        supported_types.set_index(scope, i as u32, s.into());
    }
    let supported_key = v8_str(scope, "supportedEntryTypes");
    po_fn.set(scope, supported_key.into(), supported_types.into());
    global.set(scope, po_key.into(), po_fn.into());

    // URL constructor — real implementation using Rust url crate
    fn url_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let url_str = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            String::new()
        };
        let base_str = if args.length() > 1 {
            args.get(1).to_rust_string_lossy(scope)
        } else {
            String::new()
        };
        let base_resolved = if !base_str.is_empty() {
            base_str.clone()
        } else {
            page_base_url().unwrap_or_default()
        };
        let parsed = if !base_resolved.is_empty() {
            url::Url::parse(&base_resolved)
                .ok()
                .and_then(|base| base.join(&url_str).ok())
        } else {
            url::Url::parse(&url_str).ok()
        };
        let obj = v8::Object::new(scope);
        let (href, protocol, host, hostname, port, pathname, search, hash, origin) = match parsed {
            Some(ref u) => (
                u.as_str().to_string(),
                format!("{}:", u.scheme()),
                u.host_str()
                    .map(|h| {
                        if let Some(p) = u.port() {
                            format!("{}:{}", h, p)
                        } else {
                            h.to_string()
                        }
                    })
                    .unwrap_or_default(),
                u.host_str().unwrap_or("").to_string(),
                u.port().map(|p| p.to_string()).unwrap_or_default(),
                u.path().to_string(),
                if u.query().is_some() {
                    format!("?{}", u.query().unwrap())
                } else {
                    String::new()
                },
                if u.fragment().is_some() {
                    format!("#{}", u.fragment().unwrap())
                } else {
                    String::new()
                },
                format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")),
            ),
            None => {
                // Lenient fallback for non-hierarchical URLs that real browsers
                // accept: about:blank, data:..., javascript:..., blob:..., etc.
                if let Some(colon) = url_str.find(':') {
                    let scheme = &url_str[..colon];
                    if !scheme.is_empty()
                        && scheme
                            .chars()
                            .all(|c| c.is_ascii_alphabetic() || c == '+' || c == '-' || c == '.')
                    {
                        let rest = &url_str[colon + 1..];
                        let protocol = format!("{}:", scheme.to_lowercase());
                        let pathname = rest.to_string();
                        (
                            url_str.clone(),
                            protocol,
                            String::new(),
                            String::new(),
                            String::new(),
                            pathname,
                            String::new(),
                            String::new(),
                            String::new(),
                        )
                    } else {
                        (
                            url_str.clone(),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                            url_str.clone(),
                            String::new(),
                            String::new(),
                            String::new(),
                        )
                    }
                } else {
                    (
                        url_str.clone(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        url_str.clone(),
                        String::new(),
                        String::new(),
                        String::new(),
                    )
                }
            }
        };
        set_str(scope, obj, "href", &href);
        set_str(scope, obj, "protocol", &protocol);
        set_str(scope, obj, "host", &host);
        set_str(scope, obj, "hostname", &hostname);
        set_str(scope, obj, "port", &port);
        set_str(scope, obj, "pathname", &pathname);
        set_str(scope, obj, "search", &search);
        set_str(scope, obj, "hash", &hash);
        set_str(scope, obj, "origin", &origin);
        // toString returns href
        fn url_to_string(
            scope: &mut v8::HandleScope,
            args: v8::FunctionCallbackArguments,
            mut rv: v8::ReturnValue,
        ) {
            let this = args.this();
            if let Some(href) = get_prop(scope, this, "href") {
                rv.set(href);
            }
        }
        set_fn(scope, obj, "toString", url_to_string);

        // searchParams — core-js and many loaders check for this property; if
        // it's missing they install a strict polyfill that throws on schemes we
        // deliberately handle leniently (about:blank, data:, etc.).
        let global = scope.get_current_context().global(scope);
        let usp_key = v8_str(scope, "URLSearchParams");
        if let Some(usp_val) = global.get(scope, usp_key.into()) {
            if let Ok(usp_fn) = v8::Local::<v8::Function>::try_from(usp_val) {
                let query_val = v8_str(scope, &search);
                if let Some(sp) = usp_fn.new_instance(scope, &[query_val.into()]) {
                    let sp_key = v8_str(scope, "searchParams");
                    let _ = obj.set(scope, sp_key.into(), sp.into());
                }
            }
        }

        rv.set(obj.into());
    }

    fn url_can_parse_cb(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        // Accept everything our URL constructor accepts.
        rv.set(v8::Boolean::new(scope, true).into());
    }

    let url_key = v8_str(scope, "URL");
    let url_tmpl = v8::FunctionTemplate::new(scope, url_ctor);
    let url_f = url_tmpl.get_function(scope).unwrap();
    let url_obj: v8::Local<v8::Object> = url_f.into();
    set_fn(scope, url_obj, "canParse", url_can_parse_cb);
    global.set(scope, url_key.into(), url_f.into());

    // Lock in our lenient URL implementation so bundled polyfills (e.g. core-js
    // inside Liadm/vice scripts) cannot replace it with a strict one that
    // throws "Invalid scheme" on about:blank, data:, etc.
    let protect_url = v8_str(
        scope,
        "try { Object.defineProperty(window, 'URL', { value: window.URL, writable: true, configurable: false, enumerable: true }); } catch(e) {}",
    );
    if let Some(s) = v8::Script::compile(scope, protect_url, None) {
        let _ = s.run(scope);
    }

    // NodeFilter constants for DOM traversal
    let node_filter = v8::Object::new(scope);
    set_int(scope, node_filter, "SHOW_ELEMENT", 1);
    set_int(scope, node_filter, "SHOW_ATTRIBUTE", 2);
    set_int(scope, node_filter, "SHOW_TEXT", 4);
    set_int(scope, node_filter, "SHOW_CDATA_SECTION", 8);
    set_int(scope, node_filter, "SHOW_ENTITY_REFERENCE", 16);
    set_int(scope, node_filter, "SHOW_ENTITY", 32);
    set_int(scope, node_filter, "SHOW_PROCESSING_INSTRUCTION", 64);
    set_int(scope, node_filter, "SHOW_COMMENT", 128);
    set_int(scope, node_filter, "SHOW_DOCUMENT", 256);
    set_int(scope, node_filter, "SHOW_DOCUMENT_TYPE", 512);
    set_int(scope, node_filter, "SHOW_DOCUMENT_FRAGMENT", 1024);
    set_int(scope, node_filter, "SHOW_NOTATION", 2048);
    set_int(scope, node_filter, "SHOW_ALL", 65535);
    set_int(scope, node_filter, "FILTER_ACCEPT", 1);
    set_int(scope, node_filter, "FILTER_REJECT", 2);
    set_int(scope, node_filter, "FILTER_SKIP", 3);
    let nfk = v8_str(scope, "NodeFilter");
    global.set(scope, nfk.into(), node_filter.into());

    // Empty constructors / type tags — code does `typeof Element !== "undefined"`
    // or `node instanceof Node`, so just having a function is usually enough.
    let empty_ctors = [
        "Node",
        "Element",
        "HTMLElement",
        "HTMLDivElement",
        "HTMLSpanElement",
        "HTMLInputElement",
        "HTMLButtonElement",
        "HTMLAnchorElement",
        "HTMLImageElement",
        "HTMLCanvasElement",
        "HTMLVideoElement",
        "HTMLAudioElement",
        "HTMLIFrameElement",
        "HTMLFormElement",
        "HTMLSelectElement",
        "HTMLTextAreaElement",
        "HTMLTableElement",
        "HTMLScriptElement",
        "HTMLStyleElement",
        "HTMLLinkElement",
        "HTMLMetaElement",
        "HTMLBodyElement",
        "HTMLHtmlElement",
        "HTMLHeadElement",
        "HTMLOptionElement",
        "Text",
        "Comment",
        "DocumentFragment",
        "Document",
        "DocumentType",
        "Event",
        "CustomEvent",
        "MouseEvent",
        "KeyboardEvent",
        "TouchEvent",
        "PointerEvent",
        "WheelEvent",
        "DragEvent",
        "FocusEvent",
        "InputEvent",
        "UIEvent",
        "MessageEvent",
        "StorageEvent",
        "EventTarget",
        "ResizeObserver",
        "IntersectionObserver",
        "MutationObserver",
        "DOMException",
        "DOMRect",
        "DOMTokenList",
        "NodeList",
        "HTMLCollection",
        "ShadowRoot",
        "CSSStyleSheet",
        "CSSRule",
        "FormData",
        "URLSearchParams",
        "Blob",
        "File",
        "FileReader",
        "FileList",
        "Image",
        "Audio",
        "Headers",
        "Request",
        "Response",
        "WebSocket",
        "Worker",
        "SharedWorker",
        "Notification",
        "ServiceWorker",
        // MessageChannel/MessagePort are defined above.
        "Range",
        "Selection",
        "DOMParser",
        "XMLSerializer",
    ];

    // Event constructor with proper properties
    fn event_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);

        // type argument
        let type_str = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            "".to_string()
        };
        set_str(scope, obj, "type", &type_str);

        // options argument
        let mut bubbles = false;
        let mut cancelable = false;
        let mut composed = false;

        if args.length() > 1 {
            let opts = args.get(1);
            if opts.is_object() {
                if let Some(opts_obj) = opts.to_object(scope) {
                    // bubbles
                    let bubbles_key = v8_str(scope, "bubbles");
                    if let Some(bv) = opts_obj.get(scope, bubbles_key.into()) {
                        bubbles = bv.is_true();
                    }
                    // cancelable
                    let cancelable_key = v8_str(scope, "cancelable");
                    if let Some(cv) = opts_obj.get(scope, cancelable_key.into()) {
                        cancelable = cv.is_true();
                    }
                    // composed
                    let composed_key = v8_str(scope, "composed");
                    if let Some(cv) = opts_obj.get(scope, composed_key.into()) {
                        composed = cv.is_true();
                    }
                }
            }
        }

        set_bool(scope, obj, "bubbles", bubbles);
        set_bool(scope, obj, "cancelable", cancelable);
        set_bool(scope, obj, "composed", composed);
        set_bool(scope, obj, "defaultPrevented", false);
        set_bool(scope, obj, "isTrusted", false);
        set_str(scope, obj, "eventPhase", "0");
        rv.set(obj.into());
    }
    let evt_key = v8_str(scope, "Event");
    let evt_tmpl = v8::FunctionTemplate::new(scope, event_ctor);
    let evt_f = evt_tmpl.get_function(scope).unwrap();
    global.set(scope, evt_key.into(), evt_f.into());

    // CustomEvent needs a proper constructor with detail property
    fn custom_event_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        // event type
        let type_str = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            "custom".to_string()
        };
        set_str(scope, obj, "type", &type_str);
        // detail from options argument
        let detail = if args.length() > 1 {
            args.get(1)
        } else {
            v8::undefined(scope).into()
        };
        let dk = v8_str(scope, "detail");
        obj.set(scope, dk.into(), detail);
        rv.set(obj.into());
    }
    let ce_key = v8_str(scope, "CustomEvent");
    let ce_tmpl = v8::FunctionTemplate::new(scope, custom_event_ctor);
    let ce_f = ce_tmpl.get_function(scope).unwrap();
    global.set(scope, ce_key.into(), ce_f.into());

    // MessageChannel stub — React Scheduler and many widgets use a channel to
    // schedule microtasks. Provide port1/port2 with synchronous postMessage.
    let msg_channel_stub = v8_str(
        scope,
        r#"
        (function() {
            function makePort() {
                var port = {};
                var onmessage = null;
                Object.defineProperty(port, "onmessage", {
                    get: function() { return onmessage; },
                    set: function(v) { onmessage = v; }
                });
                port.postMessage = function() {};
                port.start = function() {};
                port.close = function() {};
                port.addEventListener = function() {};
                port.removeEventListener = function() {};
                return port;
            }
            window.MessageChannel = function() {
                var p1 = makePort();
                var p2 = makePort();
                p1.postMessage = function(msg) {
                    if (typeof p2.onmessage === "function") {
                        try { p2.onmessage({ data: msg, ports: [] }); } catch(e) {}
                    }
                };
                p2.postMessage = function(msg) {
                    if (typeof p1.onmessage === "function") {
                        try { p1.onmessage({ data: msg, ports: [] }); } catch(e) {}
                    }
                };
                return { port1: p1, port2: p2 };
            };
        })();
        "#,
    );
    if let Some(s) = v8::Script::compile(scope, msg_channel_stub, None) {
        let _ = s.run(scope);
    }

    // AbortController / AbortSignal — real enough for fetch/XHR users.
    fn abort_signal_ctor(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        set_bool(scope, obj, "aborted", false);
        set_null(scope, obj, "reason");
        set_fn(scope, obj, "throwIfAborted", noop);
        set_fn(scope, obj, "addEventListener", noop);
        set_fn(scope, obj, "removeEventListener", noop);
        rv.set(obj.into());
    }
    let abort_signal_key = v8_str(scope, "AbortSignal");
    let abort_signal_tmpl = v8::FunctionTemplate::new(scope, abort_signal_ctor);
    let abort_signal_fn = abort_signal_tmpl.get_function(scope).unwrap();
    global.set(scope, abort_signal_key.into(), abort_signal_fn.into());

    fn abort_controller_abort_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let reason = if args.length() > 0 {
            args.get(0)
        } else {
            v8::undefined(scope).into()
        };
        let signal_key = v8_str(scope, "signal");
        if let Some(sig) = this
            .get(scope, signal_key.into())
            .and_then(|v| v.to_object(scope))
        {
            set_bool(scope, sig, "aborted", true);
            let reason_key = v8_str(scope, "reason");
            sig.set(scope, reason_key.into(), reason);
        }
    }
    fn abort_controller_ctor(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        let signal = v8::Object::new(scope);
        set_bool(scope, signal, "aborted", false);
        set_null(scope, signal, "reason");
        set_fn(scope, signal, "throwIfAborted", noop);
        set_fn(scope, signal, "addEventListener", noop);
        set_fn(scope, signal, "removeEventListener", noop);
        let signal_key = v8_str(scope, "signal");
        obj.set(scope, signal_key.into(), signal.into());
        set_fn(scope, obj, "abort", abort_controller_abort_cb);
        rv.set(obj.into());
    }
    let abort_controller_key = v8_str(scope, "AbortController");
    let abort_controller_tmpl = v8::FunctionTemplate::new(scope, abort_controller_ctor);
    let abort_controller_fn = abort_controller_tmpl.get_function(scope).unwrap();
    global.set(
        scope,
        abort_controller_key.into(),
        abort_controller_fn.into(),
    );

    for &n in empty_ctors.iter() {
        // Skip Event and CustomEvent since we already defined them above
        if n == "Event" || n == "CustomEvent" {
            continue;
        }
        let key = v8_str(scope, n);
        let tmpl = v8::FunctionTemplate::new(scope, empty_ctor);
        let f = tmpl.get_function(scope).unwrap();
        global.set(scope, key.into(), f.into());
    }

    // localStorage / sessionStorage
    fn make_storage<'s>(scope: &mut v8::HandleScope<'s>) -> v8::Local<'s, v8::Object> {
        let s = v8::Object::new(scope);
        set_fn(scope, s, "getItem", noop_null);
        set_fn(scope, s, "setItem", noop);
        set_fn(scope, s, "removeItem", noop);
        set_fn(scope, s, "clear", noop);
        set_fn(scope, s, "key", noop_null);
        set_int(scope, s, "length", 0);
        s
    }
    let ls = make_storage(scope);
    let lsk = v8_str(scope, "localStorage");
    global.set(scope, lsk.into(), ls.into());
    let ss = make_storage(scope);
    let ssk = v8_str(scope, "sessionStorage");
    global.set(scope, ssk.into(), ss.into());

    // Consent management stubs (CMP / TCF / GPP)
    let cmp = v8::Object::new(scope);
    set_fn(scope, cmp, "getConsentData", noop);
    set_fn(scope, cmp, "getVendorConsents", noop);
    let cmp_key = v8_str(scope, "__cmp");
    global.set(scope, cmp_key.into(), cmp.into());

    let tcf = v8::Object::new(scope);
    set_fn(scope, tcf, "registerEventListener", noop);
    set_fn(scope, tcf, "unregisterEventListener", noop);
    let tcf_key = v8_str(scope, "__tcfapi");
    global.set(scope, tcf_key.into(), tcf.into());

    let gpp = v8::Object::new(scope);
    set_fn(scope, gpp, "addEventListener", noop);
    set_fn(scope, gpp, "removeEventListener", noop);
    set_fn(scope, gpp, "ping", noop);
    let gpp_key = v8_str(scope, "__gpp");
    global.set(scope, gpp_key.into(), gpp.into());
    set_fn(scope, global, "__gppLocator", noop);

    // Common ad-tech / analytics stubs
    let freestar = v8::Object::new(scope);
    set_fn(scope, freestar, "addScript", noop);
    let freestar_queue = v8::Array::new(scope, 0);
    let freestar_queue_key = v8_str(scope, "queue");
    freestar.set(scope, freestar_queue_key.into(), freestar_queue.into());
    set_fn(scope, freestar, "config", noop);
    let freestar_key = v8_str(scope, "freestar");
    global.set(scope, freestar_key.into(), freestar.into());

    let googletag = v8::Object::new(scope);
    let googletag_cmd = v8::Array::new(scope, 0);
    let googletag_cmd_key = v8_str(scope, "cmd");
    googletag.set(scope, googletag_cmd_key.into(), googletag_cmd.into());
    set_fn(scope, googletag, "pubads", noop_obj);
    set_fn(scope, googletag, "defineSlot", noop_null);
    set_fn(scope, googletag, "display", noop);
    set_fn(scope, googletag, "enableServices", noop);
    let gt_key = v8_str(scope, "googletag");
    global.set(scope, gt_key.into(), googletag.into());

    let data_layer = v8::Array::new(scope, 0);
    let dl_key = v8_str(scope, "dataLayer");
    global.set(scope, dl_key.into(), data_layer.into());

    let gaq = v8::Array::new(scope, 0);
    let gaq_key = v8_str(scope, "_gaq");
    global.set(scope, gaq_key.into(), gaq.into());

    let comscore = v8::Array::new(scope, 0);
    let cs_key = v8_str(scope, "_comscore");
    global.set(scope, cs_key.into(), comscore.into());

    // Webpack / module stubs
    let webpack_req = v8::Object::new(scope);
    set_str(scope, webpack_req, "p", "/");
    let wpk = v8_str(scope, "__webpack_require__");
    global.set(scope, wpk.into(), webpack_req.into());

    // URLSearchParams stub with .get()
    fn url_search_params_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this_obj = args.this();
        let query = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            String::new()
        };
        set_str(scope, this_obj, "__query", &query);
        rv.set(this_obj.into());
    }
    fn url_search_params_get_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let q_key = v8_str(scope, "__query");
        let query = match this.get(scope, q_key.into()) {
            Some(v) => match v.to_string(scope) {
                Some(s) => s.to_rust_string_lossy(scope),
                None => String::new(),
            },
            None => String::new(),
        };
        let name = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            String::new()
        };
        // Very basic query string parsing
        let val = if let Some(pos) = query.find(&format!("{}=", name)) {
            let start = pos + name.len() + 1;
            if let Some(end) = query[start..].find(&['&', '#']) {
                query[start..start + end].to_string()
            } else {
                query[start..].to_string()
            }
        } else {
            String::new()
        };
        rv.set(v8_str(scope, &val).into());
    }
    let usp_tmpl = v8::FunctionTemplate::new(scope, url_search_params_ctor);
    let usp_fn = usp_tmpl.get_function(scope).unwrap();
    let usp_proto_key = v8_str(scope, "prototype");
    if let Some(usp_proto_val) = usp_fn.get(scope, usp_proto_key.into()) {
        if let Ok(usp_proto) = v8::Local::<v8::Object>::try_from(usp_proto_val) {
            let get_key = v8_str(scope, "get");
            if let Some(get_fn) = v8::Function::new(scope, url_search_params_get_cb) {
                usp_proto.set(scope, get_key.into(), get_fn.into());
            }
        }
    }
    let usp_key = v8_str(scope, "URLSearchParams");
    global.set(scope, usp_key.into(), usp_fn.into());

    // Headers constructor stub
    fn headers_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this_obj = args.this();
        set_str(scope, this_obj, "__raw", "");
        rv.set(this_obj.into());
    }
    fn headers_get_cb2(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let _name = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            String::new()
        };
        rv.set(v8_str(scope, "").into());
    }
    let headers_tmpl = v8::FunctionTemplate::new(scope, headers_ctor);
    let headers_fn = headers_tmpl.get_function(scope).unwrap();
    let headers_proto_key = v8_str(scope, "prototype");
    if let Some(headers_proto_val) = headers_fn.get(scope, headers_proto_key.into()) {
        if let Ok(headers_proto) = v8::Local::<v8::Object>::try_from(headers_proto_val) {
            let get_key = v8_str(scope, "get");
            if let Some(get_fn) = v8::Function::new(scope, headers_get_cb2) {
                headers_proto.set(scope, get_key.into(), get_fn.into());
            }
        }
    }
    let headers_key = v8_str(scope, "Headers");
    global.set(scope, headers_key.into(), headers_fn.into());

    // TextEncoder / TextDecoder — Bing and other sites call
    // `new TextEncoder().encode(str)` and `new TextDecoder().decode(buf)`.
    fn text_encoder_ctor(
        scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        set_str(scope, obj, "encoding", "utf-8");
        set_fn(scope, obj, "encode", text_encoder_encode);
        set_fn(scope, obj, "encodeInto", text_encoder_encode);
        rv.set(obj.into());
    }
    fn text_encoder_encode(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let input = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            String::new()
        };
        let bytes = input.into_bytes();
        let len = bytes.len();
        let ab = v8::ArrayBuffer::new(scope, len);
        if let Some(ta) = v8::Uint8Array::new(scope, ab, 0, len) {
            for (i, b) in bytes.iter().enumerate() {
                let val = v8::Integer::new_from_unsigned(scope, *b as u32);
                ta.set_index(scope, i as u32, val.into());
            }
            rv.set(ta.into());
        } else {
            rv.set(v8::undefined(scope).into());
        }
    }
    fn text_decoder_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let obj = v8::Object::new(scope);
        let label = if args.length() > 0 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            "utf-8".to_string()
        };
        let mut fatal = false;
        let mut ignore_bom = false;
        if args.length() > 1 {
            if let Some(opts) = args.get(1).to_object(scope) {
                let fatal_key = v8_str(scope, "fatal");
                if let Some(v) = opts.get(scope, fatal_key.into()) {
                    fatal = v.is_true();
                }
                let bom_key = v8_str(scope, "ignoreBOM");
                if let Some(v) = opts.get(scope, bom_key.into()) {
                    ignore_bom = v.is_true();
                }
            }
        }
        set_str(scope, obj, "encoding", &label.to_lowercase());
        set_bool(scope, obj, "fatal", fatal);
        set_bool(scope, obj, "ignoreBOM", ignore_bom);
        set_fn(scope, obj, "decode", text_decoder_decode);
        rv.set(obj.into());
    }
    fn text_decoder_decode(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let mut bytes: Vec<u8> = Vec::new();
        if args.length() > 0 {
            let input = args.get(0);
            if let Ok(ta) = v8::Local::<v8::Uint8Array>::try_from(input) {
                let len = ta.byte_length();
                for i in 0..len {
                    if let Some(v) = ta.get_index(scope, i as u32) {
                        if let Ok(int) = v8::Local::<v8::Integer>::try_from(v) {
                            bytes.push(int.value() as u8);
                        }
                    }
                }
            } else if let Ok(ab) = v8::Local::<v8::ArrayBuffer>::try_from(input) {
                let len = ab.byte_length();
                if len > 0 {
                    if let Some(ta) = v8::Uint8Array::new(scope, ab, 0, len) {
                        for i in 0..len {
                            if let Some(v) = ta.get_index(scope, i as u32) {
                                if let Ok(int) = v8::Local::<v8::Integer>::try_from(v) {
                                    bytes.push(int.value() as u8);
                                }
                            }
                        }
                    }
                }
            }
        }
        let decoded = String::from_utf8_lossy(&bytes);
        rv.set(v8_str(scope, &decoded).into());
    }
    let te_key = v8_str(scope, "TextEncoder");
    let te_tmpl = v8::FunctionTemplate::new(scope, text_encoder_ctor);
    let te_fn = te_tmpl.get_function(scope).unwrap();
    global.set(scope, te_key.into(), te_fn.into());

    let td_key = v8_str(scope, "TextDecoder");
    let td_tmpl = v8::FunctionTemplate::new(scope, text_decoder_ctor);
    let td_fn = td_tmpl.get_function(scope).unwrap();
    global.set(scope, td_key.into(), td_fn.into());

    // CNN-specific stubs
    let cnn_helpers = v8::Object::new(scope);
    set_fn(scope, cnn_helpers, "isEspanolPage", noop_false);
    set_fn(scope, cnn_helpers, "isArabicPage", noop_false);
    set_fn(scope, cnn_helpers, "isEditionPage", noop_false);
    set_fn(scope, cnn_helpers, "isDomesticPage", noop_true);
    set_fn(scope, cnn_helpers, "getAdfuelSrc", noop);
    let cnn = v8::Object::new(scope);
    set_fn(scope, cnn, "helpers", noop);
    let cnn_helpers_key = v8_str(scope, "helpers");
    cnn.set(scope, cnn_helpers_key.into(), cnn_helpers.into());
    let cnn_key = v8_str(scope, "CNN");
    global.set(scope, cnn_key.into(), cnn.into());

    let wm_userconsent = v8::Object::new(scope);
    set_fn(scope, wm_userconsent, "inUserConsentState", noop_true);
    set_fn(scope, wm_userconsent, "isInGdprRegion", noop_false);
    set_fn(scope, wm_userconsent, "addScript", noop);
    set_fn(scope, wm_userconsent, "addScriptTag", noop);
    set_fn(scope, wm_userconsent, "getAckTermsNeeded", noop_false);
    set_fn(scope, wm_userconsent, "isReady", noop_true);
    set_fn(scope, wm_userconsent, "addScriptElement", noop);
    set_fn(scope, wm_userconsent, "getGeoCountry", noop);
    set_fn(scope, wm_userconsent, "getVersion", noop);
    set_fn(scope, wm_userconsent, "getSimpleConsentState", noop);
    set_fn(scope, wm_userconsent, "getLinkTitle", noop_str);
    set_fn(scope, wm_userconsent, "getLinkAction", noop);
    set_fn(scope, wm_userconsent, "get", noop);
    let wm = v8::Object::new(scope);
    let uc_key = v8_str(scope, "UserConsent");
    wm.set(scope, uc_key.into(), wm_userconsent.into());
    let wm_key = v8_str(scope, "WM");
    global.set(scope, wm_key.into(), wm.into());

    let wbd_userconsent = v8::Object::new(scope);
    set_fn(scope, wbd_userconsent, "inUserConsentState", noop_true);
    set_fn(scope, wbd_userconsent, "isInGdprRegion", noop_false);
    set_fn(scope, wbd_userconsent, "addScript", noop);
    set_fn(scope, wbd_userconsent, "addScriptTag", noop);
    set_fn(scope, wbd_userconsent, "getAckTermsNeeded", noop_false);
    set_fn(scope, wbd_userconsent, "isReady", noop_true);
    set_fn(scope, wbd_userconsent, "addScriptElement", noop);
    set_fn(scope, wbd_userconsent, "getGeoCountry", noop);
    set_fn(scope, wbd_userconsent, "getVersion", noop);
    set_fn(scope, wbd_userconsent, "getSimpleConsentState", noop);
    let wbd = v8::Object::new(scope);
    let wbd_uc_key = v8_str(scope, "UserConsent");
    wbd.set(scope, wbd_uc_key.into(), wbd_userconsent.into());
    let wbd_key = v8_str(scope, "WBD");
    global.set(scope, wbd_key.into(), wbd.into());

    // window.kiln stub
    let kiln = v8::Object::new(scope);
    let kiln_key = v8_str(scope, "kiln");
    global.set(scope, kiln_key.into(), kiln.into());

    // window.scrollTo stub
    set_fn(scope, global, "scrollTo", noop);

    // IntersectionObserver stub that fires callbacks immediately
    fn intersection_observer_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this_obj = args.this();
        let cb_key = v8_str(scope, "__cb");
        let cb = if args.length() > 0 {
            args.get(0)
        } else {
            v8::undefined(scope).into()
        };
        this_obj.set(scope, cb_key.into(), cb);
        set_fn(scope, this_obj, "observe", intersection_observer_observe_cb);
        set_fn(scope, this_obj, "disconnect", noop);
        set_fn(scope, this_obj, "unobserve", noop);
        rv.set(this_obj.into());
    }
    fn intersection_observer_observe_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this_obj = args.this();
        let cb_key = v8_str(scope, "__cb");
        let cb_val = this_obj.get(scope, cb_key.into());
        let el = if args.length() > 0 {
            args.get(0)
        } else {
            v8::undefined(scope).into()
        };
        let entries = v8::Array::new(scope, 1);
        let entry = v8::Object::new(scope);
        set_bool(scope, entry, "isIntersecting", true);
        set_bool(scope, entry, "isVisible", true);
        set_num(scope, entry, "intersectionRatio", 1.0);
        let target_key = v8_str(scope, "target");
        entry.set(scope, target_key.into(), el);
        let idx_val = v8::Integer::new(scope, 0);
        entries.set(scope, idx_val.into(), entry.into());
        if let Some(cb_val) = cb_val {
            if let Ok(cb) = v8::Local::<v8::Function>::try_from(cb_val) {
                let undef = v8::undefined(scope).into();
                let _ = cb.call(scope, undef, &[entries.into()]);
            }
        }
    }
    let io_tmpl = v8::FunctionTemplate::new(scope, intersection_observer_ctor);
    let io_fn = io_tmpl.get_function(scope).unwrap();
    let io_key = v8_str(scope, "IntersectionObserver");
    global.set(scope, io_key.into(), io_fn.into());

    // ResizeObserver stub that fires callbacks immediately
    fn resize_observer_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this_obj = args.this();
        let cb_key = v8_str(scope, "__cb");
        let cb = if args.length() > 0 {
            args.get(0)
        } else {
            v8::undefined(scope).into()
        };
        this_obj.set(scope, cb_key.into(), cb);
        set_fn(scope, this_obj, "observe", resize_observer_observe_cb);
        set_fn(scope, this_obj, "disconnect", noop);
        set_fn(scope, this_obj, "unobserve", noop);
        rv.set(this_obj.into());
    }
    fn resize_observer_observe_cb(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue,
    ) {
        let this_obj = args.this();
        let cb_key = v8_str(scope, "__cb");
        let cb_val = this_obj.get(scope, cb_key.into());
        let el = if args.length() > 0 {
            args.get(0)
        } else {
            v8::undefined(scope).into()
        };
        let entries = v8::Array::new(scope, 1);
        let entry = v8::Object::new(scope);
        let content_rect = v8::Object::new(scope);
        set_num(scope, content_rect, "width", 1024.0);
        set_num(scope, content_rect, "height", 768.0);
        set_num(scope, content_rect, "x", 0.0);
        set_num(scope, content_rect, "y", 0.0);
        set_num(scope, content_rect, "top", 0.0);
        set_num(scope, content_rect, "bottom", 768.0);
        set_num(scope, content_rect, "left", 0.0);
        set_num(scope, content_rect, "right", 1024.0);
        let target_key = v8_str(scope, "target");
        let cr_key = v8_str(scope, "contentRect");
        let cbs_key = v8_str(scope, "contentBoxSize");
        let empty_arr = v8::Array::new(scope, 0);
        entry.set(scope, target_key.into(), el);
        entry.set(scope, cr_key.into(), content_rect.into());
        entry.set(scope, cbs_key.into(), empty_arr.into());
        let idx_val = v8::Integer::new(scope, 0);
        entries.set(scope, idx_val.into(), entry.into());
        if let Some(cb_val) = cb_val {
            if let Ok(cb) = v8::Local::<v8::Function>::try_from(cb_val) {
                let undef = v8::undefined(scope).into();
                let _ = cb.call(scope, undef, &[entries.into()]);
            }
        }
    }
    let ro_tmpl = v8::FunctionTemplate::new(scope, resize_observer_ctor);
    let ro_fn = ro_tmpl.get_function(scope).unwrap();
    let ro_key = v8_str(scope, "ResizeObserver");
    global.set(scope, ro_key.into(), ro_fn.into());

    // MutationObserver stub
    fn mutation_observer_ctor(
        scope: &mut v8::HandleScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this_obj = args.this();
        set_fn(scope, this_obj, "observe", noop);
        set_fn(scope, this_obj, "disconnect", noop);
        set_fn(scope, this_obj, "takeRecords", noop_empty_arr);
        rv.set(this_obj.into());
    }
    let mo_tmpl = v8::FunctionTemplate::new(scope, mutation_observer_ctor);
    let mo_fn = mo_tmpl.get_function(scope).unwrap();
    let mo_key = v8_str(scope, "MutationObserver");
    global.set(scope, mo_key.into(), mo_fn.into());

    // CSS stub (CSS.escape, CSS.supports)
    let css = v8::Object::new(scope);
    set_fn(scope, css, "escape", noop_str);
    set_fn(scope, css, "supports", noop_true);
    let css_key = v8_str(scope, "CSS");
    global.set(scope, css_key.into(), css.into());

    // window.scrollTo stub
    set_fn(scope, global, "scrollTo", noop);

    // URLSearchParams stub — many sites use it to build/parse query strings.
    let usp_stub = v8_str(
        scope,
        r#"
        (function() {
            function decode(v) {
                try { return decodeURIComponent(v.replace(/\+/g, ' ')); } catch(e) { return v; }
            }
            function encode(v) { return encodeURIComponent(v); }
            function URLSearchParams(init) {
                if (!(this instanceof URLSearchParams)) return new URLSearchParams(init);
                this._map = {};
                if (typeof init === 'string' && init) {
                    init.split('&').forEach(function(pair) {
                        var idx = pair.indexOf('=');
                        var k = idx >= 0 ? pair.slice(0, idx) : pair;
                        var v = idx >= 0 ? pair.slice(idx + 1) : '';
                        if (k) this.append(decode(k), decode(v));
                    }, this);
                } else if (init && typeof init === 'object') {
                    for (var k in init) this.append(k, init[k]);
                }
            }
            URLSearchParams.prototype.get = function(k) { return this._map[k] || null; };
            URLSearchParams.prototype.getAll = function(k) {
                var v = this._map[k];
                return v ? (v.indexOf(',') >= 0 ? v.split(',') : [v]) : [];
            };
            URLSearchParams.prototype.set = function(k, v) { this._map[k] = String(v); };
            URLSearchParams.prototype.append = function(k, v) {
                k = String(k);
                if (k in this._map) this._map[k] += ',' + String(v);
                else this._map[k] = String(v);
            };
            URLSearchParams.prototype.has = function(k) { return k in this._map; };
            URLSearchParams.prototype.delete = function(k) { delete this._map[k]; };
            URLSearchParams.prototype.toString = function() {
                var out = [];
                for (var k in this._map) out.push(encode(k) + '=' + encode(this._map[k]));
                return out.join('&');
            };
            URLSearchParams.prototype.forEach = function(cb, thisArg) {
                for (var k in this._map) cb.call(thisArg, this._map[k], k, this);
            };
            URLSearchParams.prototype.keys = function() { return Object.keys(this._map); };
            URLSearchParams.prototype.values = function() { return Object.values(this._map); };
            URLSearchParams.prototype.entries = function() {
                var out = [];
                for (var k in this._map) out.push([k, this._map[k]]);
                return out;
            };
            window.URLSearchParams = URLSearchParams;

            function FormData(init) {
                if (!(this instanceof FormData)) return new FormData(init);
                this._map = {};
                if (init && typeof init === 'object') {
                    for (var k in init) this.append(k, init[k]);
                }
            }
            FormData.prototype.append = function(k, v) {
                k = String(k);
                if (k in this._map) this._map[k].push(v);
                else this._map[k] = [v];
            };
            FormData.prototype.delete = function(k) { delete this._map[k]; };
            FormData.prototype.get = function(k) {
                var v = this._map[k];
                return v ? v[0] : null;
            };
            FormData.prototype.getAll = function(k) { return this._map[k] || []; };
            FormData.prototype.has = function(k) { return k in this._map; };
            FormData.prototype.set = function(k, v) { this._map[k] = [v]; };
            FormData.prototype.entries = function() {
                var out = [];
                for (var k in this._map) {
                    for (var i = 0; i < this._map[k].length; i++) out.push([k, this._map[k][i]]);
                }
                return out;
            };
            FormData.prototype.keys = function() { return Object.keys(this._map); };
            FormData.prototype.values = function() {
                var out = [];
                for (var k in this._map) out.push(this._map[k][0]);
                return out;
            };
            FormData.prototype.forEach = function(cb, thisArg) {
                var entries = this.entries();
                for (var i = 0; i < entries.length; i++) cb.call(thisArg, entries[i][1], entries[i][0], this);
            };
            window.FormData = FormData;

            window.HTMLDocument = function HTMLDocument() {};
            window.Window = function Window() {};
            window.structuredClone = function structuredClone(value) { return value; };
            document.styleSheets = [];
        })();
        "#,
    );
    if let Some(s) = v8::Script::compile(scope, usp_stub, None) {
        let _ = s.run(scope);
    }

    // Make consent stubs callable as well as exposing their methods.
    let callable_consent_stub = v8_str(
        scope,
        r#"
        (function() {
            if (typeof window.__tcfapi === 'function') return;
            var obj = window.__tcfapi || {};
            var fn = function() {};
            fn.registerEventListener = obj.registerEventListener || function() {};
            fn.unregisterEventListener = obj.unregisterEventListener || function() {};
            window.__tcfapi = fn;
        })();
        "#,
    );
    if let Some(s) = v8::Script::compile(scope, callable_consent_stub, None) {
        let _ = s.run(scope);
    }
}

// ── public entry point ───────────────────────────────────────────────────

const MAX_SCRIPT_SIZE: usize = 16 * 1024 * 1024; // 16MB per script
const MAX_TOTAL_JS: usize = 64 * 1024 * 1024; // 64MB total
const MAX_JS_TIME_SECS: u64 = 30;
const MAX_TIMEOUT_CALLBACKS: usize = 2_000; // Per-page setTimeout/setInterval budget

pub fn execute_scripts_v8(doc: Document, scripts: &[super::ScriptEntry]) -> Document {
    init_v8();
    cache_clear();
    clear_timeout_queue();
    clear_loaded_scripts();
    clear_window_event_listeners();
    DOCUMENT_OBJ.with(|d| *d.borrow_mut() = None);

    let dom = Arc::new(Mutex::new(DomState { document: doc }));
    set_dom(dom.clone());

    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
    {
        let handle_scope = &mut v8::HandleScope::new(isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        let global = context.global(scope);

        install_globals(scope, global);

        // Stub locale-sensitive APIs before page scripts run.
        // The v8 build in this project does not bundle ICU data, so calling
        // Date.prototype.toLocaleString crashes the isolate with an OOM inside
        // DateTimePatternGeneratorCache::CreateGenerator. Provide safe fallbacks.
        let date_locale_stub = v8_str(
            scope,
            r#"
            (function() {
                var dp = Date.prototype;
                var fallback = function(method) {
                    return function(locales, options) {
                        var d = this;
                        if (method === 'toLocaleDateString') {
                            return d.getFullYear() + '-' + String(d.getMonth()+1).padStart(2,'0') + '-' + String(d.getDate()).padStart(2,'0');
                        }
                        if (method === 'toLocaleTimeString') {
                            return String(d.getHours()).padStart(2,'0') + ':' + String(d.getMinutes()).padStart(2,'0') + ':' + String(d.getSeconds()).padStart(2,'0');
                        }
                        return d.toLocaleDateString(locales, options) + ' ' + d.toLocaleTimeString(locales, options);
                    };
                };
                try { dp.toLocaleString = fallback('toLocaleString'); } catch(e) {}
                try { dp.toLocaleDateString = fallback('toLocaleDateString'); } catch(e) {}
                try { dp.toLocaleTimeString = fallback('toLocaleTimeString'); } catch(e) {}

                var supportedLocales = function(locales) {
                    if (Array.isArray(locales)) return locales;
                    if (typeof locales === 'string') return [locales];
                    return ['en-US'];
                };

                try {
                    if (typeof Intl === 'undefined') window.Intl = {};
                    var makeFormat = function(formatter) {
                        return function(locales, options) {
                            this.format = formatter;
                            this.formatToParts = function(value) { return []; };
                            this.resolvedOptions = function() { return { locale: 'en-US' }; };
                        };
                    };
                    Intl.DateTimeFormat = makeFormat(function(d) {
                        if (!d) d = new Date();
                        return d.toISOString ? d.toISOString() : String(d);
                    });
                    Intl.DateTimeFormat.supportedLocalesOf = supportedLocales;
                    Intl.NumberFormat = makeFormat(function(n) { return String(n); });
                    Intl.NumberFormat.supportedLocalesOf = supportedLocales;
                    Intl.Collator = function(locales, options) {
                        this.compare = function(a, b) { return String(a).localeCompare(String(b)); };
                        this.resolvedOptions = function() { return { locale: 'en-US' }; };
                    };
                    Intl.Collator.supportedLocalesOf = supportedLocales;
                    Intl.ListFormat = function(locales, options) {
                        this.format = function(list) { return Array.isArray(list) ? list.join(', ') : String(list); };
                        this.formatToParts = function(list) { return []; };
                        this.resolvedOptions = function() { return { locale: 'en-US', type: 'conjunction', style: 'long' }; };
                    };
                    Intl.ListFormat.supportedLocalesOf = supportedLocales;
                    Intl.RelativeTimeFormat = function(locales, options) {
                        this.format = function(value, unit) { return String(value) + ' ' + String(unit); };
                        this.formatToParts = function(value, unit) { return []; };
                        this.resolvedOptions = function() { return { locale: 'en-US', numeric: 'auto', style: 'long' }; };
                    };
                    Intl.RelativeTimeFormat.supportedLocalesOf = supportedLocales;
                    Intl.PluralRules = function(locales, options) {
                        this.select = function(n) { return 'other'; };
                        this.resolvedOptions = function() { return { locale: 'en-US', type: 'cardinal' }; };
                    };
                    Intl.PluralRules.supportedLocalesOf = supportedLocales;
                    Intl.DisplayNames = function(locales, options) {
                        this.of = function(code) { return String(code); };
                        this.resolvedOptions = function() { return { locale: 'en-US', type: 'language' }; };
                    };
                    Intl.DisplayNames.supportedLocalesOf = supportedLocales;
                    Intl.Segmenter = function(locales, options) {
                        this.segment = function(str) {
                            var s = String(str);
                            return [{ segment: s, index: 0, isWordLike: s.length > 0 }];
                        };
                        this.resolvedOptions = function() { return { locale: 'en-US', granularity: 'grapheme' }; };
                    };
                    Intl.Segmenter.supportedLocalesOf = supportedLocales;
                    Intl.Locale = function(tag) {
                        this.baseName = typeof tag === 'string' ? tag : 'en-US';
                        this.language = 'en';
                        this.script = '';
                        this.region = 'US';
                        this.toString = function() { return this.baseName; };
                        this.maximize = function() { return new Intl.Locale(this.baseName); };
                        this.minimize = function() { return new Intl.Locale(this.baseName); };
                    };
                    Intl.getCanonicalLocales = function(locales) {
                        if (Array.isArray(locales)) return locales;
                        if (typeof locales === 'string') return [locales];
                        return ['en-US'];
                    };
                } catch(e) {}
            })();
        "#,
        );
        if let Some(stub_script) = v8::Script::compile(scope, date_locale_stub, None) {
            let _ = stub_script.run(scope);
        }

        // Update location and document URL from the page origin.  Scripts may be
        // loaded from third-party CDNs, so strip any script path down to the
        // scheme + host before using it as the base URL for relative fetches.
        let base_url = scripts
            .first()
            .and_then(|s| {
                let raw = if s.origin.starts_with("http") || s.origin.starts_with("/") {
                    s.origin.clone()
                } else if let Some(pos) = s.origin.find(" in ") {
                    s.origin[pos + 4..].to_string()
                } else {
                    String::new()
                };
                if raw.starts_with("http") {
                    url::Url::parse(&raw)
                        .ok()
                        .map(|u| u.origin().ascii_serialization())
                } else {
                    Some(raw)
                }
            })
            .unwrap_or_default();
        set_page_base_url(base_url.clone());
        if !base_url.is_empty() {
            let loc_key = v8_str(scope, "location");
            if let Some(loc_val) = global.get(scope, loc_key.into()) {
                if let Ok(loc) = v8::Local::<v8::Object>::try_from(loc_val) {
                    set_str(scope, loc, "href", &base_url);
                    set_str(scope, loc, "origin", &base_url);
                    // Parse hostname from URL
                    if let Ok(url) = url::Url::parse(&base_url) {
                        set_str(scope, loc, "hostname", url.host_str().unwrap_or(""));
                        set_str(
                            scope,
                            loc,
                            "host",
                            &format!(
                                "{}{}",
                                url.host_str().unwrap_or(""),
                                if url.port().is_some() {
                                    format!(":{}", url.port().unwrap())
                                } else {
                                    String::new()
                                }
                            ),
                        );
                        set_str(
                            scope,
                            loc,
                            "port",
                            &url.port().map(|p| p.to_string()).unwrap_or_default(),
                        );
                        set_str(scope, loc, "pathname", url.path());
                        set_str(scope, loc, "search", url.query().unwrap_or(""));
                        set_str(scope, loc, "hash", url.fragment().unwrap_or(""));
                    }
                }
            }
            if let Some(doc) = document_obj(scope) {
                set_str(scope, doc, "URL", &base_url);
                set_str(scope, doc, "documentURI", &base_url);
                let loc_key = v8_str(scope, "location");
                if let Some(loc_val) = global.get(scope, loc_key.into()) {
                    doc.set(scope, loc_key.into(), loc_val);
                }
            }
        }

        let js_start = std::time::Instant::now();
        let max_time = std::time::Duration::from_secs(MAX_JS_TIME_SECS);
        let mut total_bytes = 0usize;

        for script in scripts {
            let mut source = script.source.clone();
            // Optional: capture JS stacks for hard-to-diagnose script errors.
            if std::env::var_os("INCOGNIDIUM_JS_STACK").is_some() {
                source = format!(
                    "try {{\n{}\n}} catch (__e) {{ console.error('JS stack in {}: ' + __e + '\\n' + (__e.stack || '')); throw __e; }}\n//# sourceURL={}\n",
                    source,
                    script.origin,
                    script.origin
                );
            }
            // Ensure WM.UserConsent stubs survive script mutations (e.g. Optimizely)
            {
                let fix = v8_str(
                    scope,
                    r#"
                    if (window.WM && window.WM.UserConsent) {
                        if (typeof window.WM.UserConsent.getLinkTitle !== 'function') {
                            window.WM.UserConsent.getLinkTitle = function() { return ''; };
                        }
                        if (typeof window.WM.UserConsent.getLinkAction !== 'function') {
                            window.WM.UserConsent.getLinkAction = function() {};
                        }
                        if (typeof window.WM.UserConsent.get !== 'function') {
                            window.WM.UserConsent.get = function() {};
                        }
                        if (typeof window.WM.UserConsent.getConsentState !== 'function') {
                            window.WM.UserConsent.getConsentState = function() { return {}; };
                        }
                    }
                    if (window.WBD && window.WBD.UserConsent) {
                        if (typeof window.WBD.UserConsent.getLinkTitle !== 'function') {
                            window.WBD.UserConsent.getLinkTitle = function() { return ''; };
                        }
                        if (typeof window.WBD.UserConsent.getLinkAction !== 'function') {
                            window.WBD.UserConsent.getLinkAction = function() {};
                        }
                        if (typeof window.WBD.UserConsent.get !== 'function') {
                            window.WBD.UserConsent.get = function() {};
                        }
                    }
                "#,
                );
                if let Some(s) = v8::Script::compile(scope, fix, None) {
                    let _ = s.run(scope);
                }
            }
            // Wrap CNN's mountLegacyServices / mountComponentModules in try-catch
            // so a single failing legacy service doesn't abort the entire script
            if source.contains("mountLegacyServices()") {
                source = source.replace(
                    "mountLegacyServices();",
                    "try{mountLegacyServices();}catch(e){console.error(e);}",
                );
                source = source.replace(
                    "mountComponentModules();",
                    "try{mountComponentModules();}catch(e){console.error(e);}",
                );
            }
            // CNN's webpack bootstrap passes only 2 args to module factories,
            // but factories expect 3 (module, exports, __webpack_require__).
            // Patch the bootstrap to pass the require function as the third arg.
            if source.contains("require=function(global)") {
                source = source.replace(
                    "window.modules[global].call(moduleEl,moduleEl,require)",
                    "window.modules[global].call(moduleEl,moduleEl,moduleEl,require)",
                );
                source = source.replace(
                    "window.modules[global].call(module,module,require)",
                    "window.modules[global].call(module,module,module.exports,require)",
                );
                // Log the real error before re-throwing
                source = source.replace(
                    "catch(error){throw new Error('Cannot call module ',global);}",
                    "catch(error){console.error('Factory error for',global,':',error.message);throw new Error('Cannot call module '+global);}",
                );
            }
            // V8 builds without full ICU data throw a SyntaxError for Unicode property
            // escapes such as \p{Emoji_Presentation}. Patch the common emoji-detection
            // regex used by MSN (and others) to a fallback character class so the
            // script can load.
            if source.contains("\\p{Emoji_Presentation}") {
                source = source.replace(
                    r"/(\p{Emoji_Presentation}|\p{Emoji}\uFE0F)/u",
                    r"/([\u{1F600}-\u{1F64F}]|\u{1F300}-\u{1F5FF}|\u{1F680}-\u{1F6FF}|\u{1F1E0}-\u{1F1FF}|\u{2600}-\u{26FF}|\u{2700}-\u{27BF})/u",
                );
            }

            if js_start.elapsed() > max_time {
                eprintln!(
                    "JS time limit reached ({:.1}s), skipping remaining scripts",
                    js_start.elapsed().as_secs_f32()
                );
                break;
            }
            if source.len() > MAX_SCRIPT_SIZE {
                eprintln!(
                    "JS skip ({}KB > {}KB limit): {}",
                    source.len() / 1024,
                    MAX_SCRIPT_SIZE / 1024,
                    script.origin
                );
                continue;
            }
            total_bytes += source.len();
            if total_bytes > MAX_TOTAL_JS {
                eprintln!(
                    "JS skip (total {}KB > {}KB page limit): {}",
                    total_bytes / 1024,
                    MAX_TOTAL_JS / 1024,
                    script.origin
                );
                continue;
            }

            let start = std::time::Instant::now();
            // Set document.currentScript before executing. Build a real <script>
            // element wrapper (not attached to the DOM) so scripts can call
            // getAttribute / hasAttribute on it.
            let current_script_src =
                if script.origin.starts_with("http") || script.origin.starts_with("/") {
                    script.origin.clone()
                } else if let Some(pos) = script.origin.find(" in ") {
                    script.origin[pos + 4..].to_string()
                } else {
                    String::new()
                };
            let current_script_id = with_dom(|state| {
                let id = state.document.nodes.len();
                let mut el = ElementData::new("script");
                if !current_script_src.is_empty() {
                    el.attributes
                        .insert("src".to_string(), current_script_src.clone());
                }
                el.attributes
                    .insert("type".to_string(), "text/javascript".to_string());
                state.document.nodes.push(Node {
                    id,
                    parent: None,
                    children: Vec::new(),
                    data: NodeData::Element(el),
                });
                id
            });
            if let Some(doc) = document_obj(scope) {
                let cs_key = v8_str(scope, "currentScript");
                let script_el = wrap_element(scope, current_script_id);
                doc.set(scope, cs_key.into(), script_el.into());
            }
            {
                let tc = &mut v8::TryCatch::new(scope);
                let source_v8 = v8_str(tc, &source);
                match v8::Script::compile(tc, source_v8, None) {
                    Some(script_obj) => match script_obj.run(tc) {
                        Some(_) => {}
                        None => {
                            let err = tc
                                .exception()
                                .and_then(|e| e.to_string(tc))
                                .map(|s| s.to_rust_string_lossy(tc))
                                .unwrap_or_else(|| "unknown error".into());
                            // Print source location without walking the full JS stack,
                            // which can overflow the native thread stack on deeply
                            // recursive scripts.
                            let mut loc = String::new();
                            let mut snippet = String::new();
                            if let Some(ex) = tc.exception() {
                                let msg = v8::Exception::create_message(tc, ex);
                                if let Some(line) = msg.get_line_number(tc) {
                                    loc.push_str(&format!(" line {}", line));
                                }
                                if let Some(sl) = msg.get_source_line(tc) {
                                    let text = sl.to_rust_string_lossy(tc);
                                    if !text.is_empty() {
                                        loc.push_str(&format!(" near: {}", text.trim()));
                                    }
                                }
                                let pos = msg.get_start_position() as usize;
                                if pos < source.len() {
                                    let start = pos.saturating_sub(120);
                                    let end = (pos + 120).min(source.len());
                                    snippet = source[start..end].replace('\n', "\\n");
                                    if start > 0 {
                                        snippet.insert_str(0, "...");
                                    }
                                    if end < source.len() {
                                        snippet.push_str("...");
                                    }
                                }
                            }
                            eprintln!("JS error in {}: {}{}", script.origin, err, loc);
                            if !snippet.is_empty() {
                                eprintln!("  snippet: {}", snippet);
                            }
                        }
                    },
                    None => {
                        let err = tc
                            .exception()
                            .and_then(|e| e.to_string(tc))
                            .map(|s| s.to_rust_string_lossy(tc))
                            .unwrap_or_else(|| "unknown parse error".into());
                        let mut loc = String::new();
                        let mut snippet = String::new();
                        if let Some(ex) = tc.exception() {
                            let msg = v8::Exception::create_message(tc, ex);
                            if let Some(line) = msg.get_line_number(tc) {
                                loc.push_str(&format!(" line {}", line));
                            }
                            if let Some(sl) = msg.get_source_line(tc) {
                                let text = sl.to_rust_string_lossy(tc);
                                if !text.is_empty() {
                                    loc.push_str(&format!(" near: {}", text.trim()));
                                }
                            }
                            let pos = msg.get_start_position() as usize;
                            if pos < source.len() {
                                let start = pos.saturating_sub(120);
                                let end = (pos + 120).min(source.len());
                                snippet = source[start..end].replace('\n', "\\n");
                                if start > 0 {
                                    snippet.insert_str(0, "...");
                                }
                                if end < source.len() {
                                    snippet.push_str("...");
                                }
                            }
                        }
                        eprintln!("JS parse error in {}: {}{}", script.origin, err, loc);
                        if !snippet.is_empty() {
                            eprintln!("  snippet: {}", snippet);
                        }
                    }
                }
            }
            // Clear document.currentScript after execution
            if let Some(doc) = document_obj(scope) {
                let cs_key = v8_str(scope, "currentScript");
                let null_val = v8::null(scope).into();
                doc.set(scope, cs_key.into(), null_val);
            }
            let elapsed = start.elapsed();
            if elapsed.as_secs() > 3 {
                eprintln!("JS slow ({:.1}s): {}", elapsed.as_secs_f32(), script.origin);
            }
            // Drain a small number of timeouts registered by this script before
            // moving on. A per-page budget prevents infinite synchronous recursion
            // from chained setTimeout(0) loops on WordPress/React bundles.
            drain_timeout_queue(scope, 10);
            scope.perform_microtask_checkpoint();
        }
        // Run any remaining timeouts enqueued by the final scripts.
        drain_timeout_queue(scope, MAX_TIMEOUT_CALLBACKS);
        scope.perform_microtask_checkpoint();

        // Fire DOMContentLoaded / load events so pages that deferred bootstrap
        // code in addEventListener handlers actually run it.
        if let Some(doc) = document_obj(scope) {
            set_str(scope, doc, "readyState", "interactive");
        }
        dispatch_window_event(scope, "DOMContentLoaded");
        if let Some(doc) = document_obj(scope) {
            set_str(scope, doc, "readyState", "complete");
        }
        dispatch_window_event(scope, "load");
    }

    let _ = take_dom();
    let state = dom.lock().unwrap();
    state.document.clone()
}
