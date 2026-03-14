//! DOM bindings for the Boa JavaScript engine.
//!
//! Uses thread-local storage for DOM state (same pattern as murkiu-bindings).

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use boa_engine::{
    Context, JsResult, JsValue, Source,
    native_function::NativeFunction,
    object::JsObject,
    property::Attribute,
    JsString,
};

use incognidium_dom::*;

/// Shared DOM state accessible from native JS functions via thread-local.
pub struct DomState {
    pub document: Document,
}

type SharedDom = Arc<Mutex<DomState>>;

thread_local! {
    static DOM: RefCell<Option<SharedDom>> = RefCell::new(None);
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

/// Install all DOM globals on the given Boa context.
pub fn install_dom_bindings(ctx: &mut Context, dom: SharedDom) {
    set_dom(dom);
    install_console(ctx);
    install_document(ctx);
    install_window(ctx);
    install_timer_stubs(ctx);
}

fn noop(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn noop_null(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::null())
}

fn noop_zero(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(0))
}

fn noop_false(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(false))
}

fn noop_empty_string(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(JsString::from("")))
}

fn set_fn(obj: &JsObject, name: &str, f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>, ctx: &mut Context) {
    obj.set(
        JsString::from(name),
        NativeFunction::from_fn_ptr(f).to_js_function(ctx.realm()),
        false,
        ctx,
    ).ok();
}

fn set_str(obj: &JsObject, name: &str, val: &str, ctx: &mut Context) {
    obj.set(JsString::from(name), JsValue::from(JsString::from(val)), false, ctx).ok();
}

fn set_int(obj: &JsObject, name: &str, val: i32, ctx: &mut Context) {
    obj.set(JsString::from(name), JsValue::from(val), false, ctx).ok();
}

fn set_bool(obj: &JsObject, name: &str, val: bool, ctx: &mut Context) {
    obj.set(JsString::from(name), JsValue::from(val), false, ctx).ok();
}

fn install_console(ctx: &mut Context) {
    let console = JsObject::default();

    fn console_log(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let line: String = args.iter()
            .map(|a| a.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        eprintln!("[console.log] {line}");
        Ok(JsValue::undefined())
    }
    fn console_warn(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let line: String = args.iter()
            .map(|a| a.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        eprintln!("[console.warn] {line}");
        Ok(JsValue::undefined())
    }
    fn console_error(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let line: String = args.iter()
            .map(|a| a.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        eprintln!("[console.error] {line}");
        Ok(JsValue::undefined())
    }

    set_fn(&console, "log", console_log, ctx);
    set_fn(&console, "warn", console_warn, ctx);
    set_fn(&console, "error", console_error, ctx);
    set_fn(&console, "info", noop, ctx);
    set_fn(&console, "debug", noop, ctx);
    set_fn(&console, "trace", noop, ctx);
    set_fn(&console, "dir", noop, ctx);
    set_fn(&console, "table", noop, ctx);
    set_fn(&console, "group", noop, ctx);
    set_fn(&console, "groupEnd", noop, ctx);
    set_fn(&console, "time", noop, ctx);
    set_fn(&console, "timeEnd", noop, ctx);
    set_fn(&console, "assert", noop, ctx);
    set_fn(&console, "clear", noop, ctx);
    set_fn(&console, "count", noop, ctx);
    set_fn(&console, "countReset", noop, ctx);

    ctx.register_global_property(JsString::from("console"), console, Attribute::all()).ok();
}

fn install_document(ctx: &mut Context) {
    let doc_obj = JsObject::default();

    fn get_element_by_id(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let id = args.get(0)
            .map(|v| v.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .transpose()?
            .unwrap_or_default();
        let node_id = with_dom(|state| state.document.get_element_by_id(&id));
        match node_id {
            Some(nid) => wrap_element(nid, ctx),
            None => Ok(JsValue::null()),
        }
    }

    fn create_element(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let tag = args.get(0)
            .map(|v| v.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .transpose()?
            .unwrap_or_else(|| "div".into());
        let node_id = with_dom(|state| {
            let id = state.document.nodes.len();
            state.document.nodes.push(Node {
                id,
                parent: None,
                children: Vec::new(),
                data: NodeData::Element(ElementData::new(&tag)),
            });
            id
        });
        wrap_element(node_id, ctx)
    }

    fn create_text_node(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let text = args.get(0)
            .map(|v| v.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .transpose()?
            .unwrap_or_default();
        with_dom(|state| {
            let id = state.document.nodes.len();
            state.document.nodes.push(Node {
                id,
                parent: None,
                children: Vec::new(),
                data: NodeData::Text(TextData { content: text }),
            });
        });
        Ok(JsValue::undefined())
    }

    fn query_selector(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let sel = args.get(0)
            .map(|v| v.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .transpose()?
            .unwrap_or_default();
        let node_id = with_dom(|state| {
            let bridge = murkiu_bindings::DomBridge::new(state.document.clone());
            bridge.query_selector(&sel)
        });
        match node_id {
            Some(nid) => wrap_element(nid, ctx),
            None => Ok(JsValue::null()),
        }
    }

    fn query_selector_all(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let sel = args.get(0)
            .map(|v| v.to_string(ctx).map(|s| s.to_std_string_escaped()))
            .transpose()?
            .unwrap_or_default();
        let ids = with_dom(|state| {
            let bridge = murkiu_bindings::DomBridge::new(state.document.clone());
            bridge.query_selector_all(&sel)
        });
        let arr = boa_engine::object::builtins::JsArray::new(ctx);
        for nid in ids {
            let el = wrap_element(nid, ctx)?;
            arr.push(el, ctx)?;
        }
        Ok(arr.into())
    }

    fn get_elements_by_tag(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        query_selector_all(&JsValue::undefined(), args, ctx)
    }

    fn get_elements_by_class(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
        let class = args.get(0)
            .map(|v| v.to_string(ctx).map(|s| format!(".{}", s.to_std_string_escaped())))
            .transpose()?
            .unwrap_or_default();
        let ids = with_dom(|state| {
            let bridge = murkiu_bindings::DomBridge::new(state.document.clone());
            bridge.query_selector_all(&class)
        });
        let arr = boa_engine::object::builtins::JsArray::new(ctx);
        for nid in ids {
            let el = wrap_element(nid, ctx)?;
            arr.push(el, ctx)?;
        }
        Ok(arr.into())
    }

    set_fn(&doc_obj, "getElementById", get_element_by_id, ctx);
    set_fn(&doc_obj, "createElement", create_element, ctx);
    set_fn(&doc_obj, "createTextNode", create_text_node, ctx);
    set_fn(&doc_obj, "querySelector", query_selector, ctx);
    set_fn(&doc_obj, "querySelectorAll", query_selector_all, ctx);
    set_fn(&doc_obj, "getElementsByTagName", get_elements_by_tag, ctx);
    set_fn(&doc_obj, "getElementsByClassName", get_elements_by_class, ctx);
    set_fn(&doc_obj, "addEventListener", noop, ctx);
    set_fn(&doc_obj, "removeEventListener", noop, ctx);
    set_fn(&doc_obj, "createEvent", noop, ctx);
    set_fn(&doc_obj, "createDocumentFragment", noop, ctx);
    set_fn(&doc_obj, "createComment", noop, ctx);
    set_fn(&doc_obj, "createRange", noop, ctx);
    set_fn(&doc_obj, "execCommand", noop_false, ctx);

    set_str(&doc_obj, "readyState", "complete", ctx);
    doc_obj.set(JsString::from("body"), JsValue::null(), false, ctx).ok();
    doc_obj.set(JsString::from("documentElement"), JsValue::null(), false, ctx).ok();
    doc_obj.set(JsString::from("head"), JsValue::null(), false, ctx).ok();
    set_str(&doc_obj, "cookie", "", ctx);
    set_str(&doc_obj, "referrer", "", ctx);
    set_str(&doc_obj, "title", "", ctx);
    set_str(&doc_obj, "domain", "", ctx);
    set_str(&doc_obj, "URL", "", ctx);
    set_str(&doc_obj, "characterSet", "UTF-8", ctx);
    set_str(&doc_obj, "contentType", "text/html", ctx);
    set_str(&doc_obj, "compatMode", "CSS1Compat", ctx);

    ctx.register_global_property(JsString::from("document"), doc_obj, Attribute::all()).ok();
}

fn install_window(ctx: &mut Context) {
    let win = JsObject::default();

    set_int(&win, "innerWidth", 1024, ctx);
    set_int(&win, "innerHeight", 768, ctx);
    set_int(&win, "outerWidth", 1024, ctx);
    set_int(&win, "outerHeight", 768, ctx);
    set_int(&win, "screenX", 0, ctx);
    set_int(&win, "screenY", 0, ctx);
    set_int(&win, "pageXOffset", 0, ctx);
    set_int(&win, "pageYOffset", 0, ctx);
    set_int(&win, "scrollX", 0, ctx);
    set_int(&win, "scrollY", 0, ctx);
    set_int(&win, "devicePixelRatio", 1, ctx);
    set_bool(&win, "closed", false, ctx);
    set_str(&win, "name", "", ctx);
    set_str(&win, "origin", "", ctx);

    set_fn(&win, "addEventListener", noop, ctx);
    set_fn(&win, "removeEventListener", noop, ctx);
    set_fn(&win, "dispatchEvent", noop, ctx);
    set_fn(&win, "scrollTo", noop, ctx);
    set_fn(&win, "scrollBy", noop, ctx);
    set_fn(&win, "open", noop_null, ctx);
    set_fn(&win, "close", noop, ctx);
    set_fn(&win, "alert", noop, ctx);
    set_fn(&win, "confirm", noop_false, ctx);
    set_fn(&win, "prompt", noop_null, ctx);
    set_fn(&win, "fetch", noop, ctx);
    set_fn(&win, "postMessage", noop, ctx);
    set_fn(&win, "requestAnimationFrame", noop_zero, ctx);
    set_fn(&win, "cancelAnimationFrame", noop, ctx);
    set_fn(&win, "getComputedStyle", noop, ctx);
    set_fn(&win, "matchMedia", noop, ctx);
    set_fn(&win, "btoa", noop_empty_string, ctx);
    set_fn(&win, "atob", noop_empty_string, ctx);
    set_fn(&win, "requestIdleCallback", noop_zero, ctx);
    set_fn(&win, "cancelIdleCallback", noop, ctx);
    set_fn(&win, "getSelection", noop_null, ctx);
    set_fn(&win, "resizeTo", noop, ctx);
    set_fn(&win, "resizeBy", noop, ctx);
    set_fn(&win, "moveTo", noop, ctx);
    set_fn(&win, "moveBy", noop, ctx);
    set_fn(&win, "print", noop, ctx);
    set_fn(&win, "stop", noop, ctx);
    set_fn(&win, "focus", noop, ctx);
    set_fn(&win, "blur", noop, ctx);

    // navigator
    let nav = JsObject::default();
    set_str(&nav, "userAgent", "Mozilla/5.0 (X11; Linux x86_64) Incognidium/0.1", ctx);
    set_str(&nav, "language", "en-US", ctx);
    set_str(&nav, "platform", "Linux x86_64", ctx);
    set_bool(&nav, "cookieEnabled", false, ctx);
    set_bool(&nav, "onLine", true, ctx);
    set_int(&nav, "hardwareConcurrency", 4, ctx);
    set_str(&nav, "appName", "Incognidium", ctx);
    set_str(&nav, "appVersion", "0.1", ctx);
    set_str(&nav, "vendor", "", ctx);
    set_fn(&nav, "sendBeacon", noop_false, ctx);
    win.set(JsString::from("navigator"), nav, false, ctx).ok();

    // location
    let loc = JsObject::default();
    set_str(&loc, "href", "", ctx);
    set_str(&loc, "hostname", "", ctx);
    set_str(&loc, "pathname", "/", ctx);
    set_str(&loc, "search", "", ctx);
    set_str(&loc, "hash", "", ctx);
    set_str(&loc, "protocol", "https:", ctx);
    set_str(&loc, "origin", "", ctx);
    set_str(&loc, "host", "", ctx);
    set_str(&loc, "port", "", ctx);
    set_fn(&loc, "reload", noop, ctx);
    set_fn(&loc, "replace", noop, ctx);
    set_fn(&loc, "assign", noop, ctx);
    win.set(JsString::from("location"), loc, false, ctx).ok();

    // history
    let history = JsObject::default();
    set_fn(&history, "pushState", noop, ctx);
    set_fn(&history, "replaceState", noop, ctx);
    set_fn(&history, "back", noop, ctx);
    set_fn(&history, "forward", noop, ctx);
    set_fn(&history, "go", noop, ctx);
    set_int(&history, "length", 1, ctx);
    win.set(JsString::from("history"), history, false, ctx).ok();

    // screen
    let screen = JsObject::default();
    set_int(&screen, "width", 1920, ctx);
    set_int(&screen, "height", 1080, ctx);
    set_int(&screen, "availWidth", 1920, ctx);
    set_int(&screen, "availHeight", 1080, ctx);
    set_int(&screen, "colorDepth", 24, ctx);
    set_int(&screen, "pixelDepth", 24, ctx);
    win.set(JsString::from("screen"), screen, false, ctx).ok();

    // performance
    let perf = JsObject::default();
    set_fn(&perf, "now", noop_zero, ctx);
    set_fn(&perf, "mark", noop, ctx);
    set_fn(&perf, "measure", noop, ctx);
    set_fn(&perf, "getEntriesByName", noop, ctx);
    set_fn(&perf, "getEntriesByType", noop, ctx);
    win.set(JsString::from("performance"), perf, false, ctx).ok();

    ctx.register_global_property(JsString::from("window"), win, Attribute::all()).ok();
    // self = window, globalThis = window handled by boa already
}

fn install_timer_stubs(ctx: &mut Context) {
    ctx.register_global_property(JsString::from("setTimeout"), NativeFunction::from_fn_ptr(noop_zero).to_js_function(ctx.realm()), Attribute::all()).ok();
    ctx.register_global_property(JsString::from("setInterval"), NativeFunction::from_fn_ptr(noop_zero).to_js_function(ctx.realm()), Attribute::all()).ok();
    ctx.register_global_property(JsString::from("clearTimeout"), NativeFunction::from_fn_ptr(noop).to_js_function(ctx.realm()), Attribute::all()).ok();
    ctx.register_global_property(JsString::from("clearInterval"), NativeFunction::from_fn_ptr(noop).to_js_function(ctx.realm()), Attribute::all()).ok();
    ctx.register_global_property(JsString::from("requestAnimationFrame"), NativeFunction::from_fn_ptr(noop_zero).to_js_function(ctx.realm()), Attribute::all()).ok();
    ctx.register_global_property(JsString::from("cancelAnimationFrame"), NativeFunction::from_fn_ptr(noop).to_js_function(ctx.realm()), Attribute::all()).ok();
    ctx.register_global_property(JsString::from("queueMicrotask"), NativeFunction::from_fn_ptr(noop).to_js_function(ctx.realm()), Attribute::all()).ok();
}

/// Wrap a DOM node ID as a JS object with element properties.
fn wrap_element(node_id: NodeId, ctx: &mut Context) -> JsResult<JsValue> {
    let obj = JsObject::default();

    with_dom(|state| {
        let node = &state.document.nodes[node_id];
        obj.set(JsString::from("__node_id__"), JsValue::from(node_id as i32), false, ctx).ok();

        match &node.data {
            NodeData::Element(el) => {
                set_str(&obj, "tagName", &el.tag_name.to_uppercase(), ctx);
                set_str(&obj, "nodeName", &el.tag_name.to_uppercase(), ctx);
                set_int(&obj, "nodeType", 1, ctx);
                if let Some(id) = el.attributes.get("id") {
                    set_str(&obj, "id", id, ctx);
                }
                if let Some(class) = el.attributes.get("class") {
                    set_str(&obj, "className", class, ctx);
                }
            }
            NodeData::Text(t) => {
                set_int(&obj, "nodeType", 3, ctx);
                set_str(&obj, "textContent", &t.content, ctx);
                set_str(&obj, "nodeValue", &t.content, ctx);
            }
            _ => {}
        }
    });

    // Element methods
    set_fn(&obj, "appendChild", noop, ctx);
    set_fn(&obj, "removeChild", noop, ctx);
    set_fn(&obj, "insertBefore", noop, ctx);
    set_fn(&obj, "replaceChild", noop, ctx);
    set_fn(&obj, "cloneNode", noop, ctx);
    set_fn(&obj, "remove", noop, ctx);
    set_fn(&obj, "setAttribute", noop, ctx);
    set_fn(&obj, "getAttribute", noop_null, ctx);
    set_fn(&obj, "hasAttribute", noop_false, ctx);
    set_fn(&obj, "removeAttribute", noop, ctx);
    set_fn(&obj, "addEventListener", noop, ctx);
    set_fn(&obj, "removeEventListener", noop, ctx);
    set_fn(&obj, "dispatchEvent", noop, ctx);
    set_fn(&obj, "querySelector", noop_null, ctx);
    set_fn(&obj, "querySelectorAll", noop, ctx);
    set_fn(&obj, "getElementsByTagName", noop, ctx);
    set_fn(&obj, "getElementsByClassName", noop, ctx);
    set_fn(&obj, "getBoundingClientRect", noop, ctx);
    set_fn(&obj, "focus", noop, ctx);
    set_fn(&obj, "blur", noop, ctx);
    set_fn(&obj, "click", noop, ctx);
    set_fn(&obj, "contains", noop_false, ctx);
    set_fn(&obj, "matches", noop_false, ctx);
    set_fn(&obj, "closest", noop_null, ctx);
    set_fn(&obj, "insertAdjacentHTML", noop, ctx);
    set_fn(&obj, "insertAdjacentElement", noop, ctx);

    // style, classList, dataset
    let style = JsObject::default();
    set_fn(&style, "setProperty", noop, ctx);
    set_fn(&style, "getPropertyValue", noop_empty_string, ctx);
    set_fn(&style, "removeProperty", noop, ctx);
    obj.set(JsString::from("style"), style, false, ctx)?;

    let classlist = JsObject::default();
    set_fn(&classlist, "add", noop, ctx);
    set_fn(&classlist, "remove", noop, ctx);
    set_fn(&classlist, "toggle", noop_false, ctx);
    set_fn(&classlist, "contains", noop_false, ctx);
    set_fn(&classlist, "replace", noop, ctx);
    obj.set(JsString::from("classList"), classlist, false, ctx)?;

    obj.set(JsString::from("dataset"), JsObject::default(), false, ctx)?;

    Ok(obj.into())
}

/// Execute scripts using Boa engine. Returns the (possibly modified) Document.
pub fn execute_scripts_boa(
    doc: Document,
    scripts: &[super::ScriptEntry],
) -> Document {
    let dom = Arc::new(Mutex::new(DomState { document: doc }));
    let mut ctx = Context::default();

    install_dom_bindings(&mut ctx, dom.clone());

    for script in scripts {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ctx.eval(Source::from_bytes(script.source.as_bytes()))
        }));
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                eprintln!("JS error in {}: {e}", script.origin);
            }
            Err(_) => {
                eprintln!("JS panic in {} (caught)", script.origin);
            }
        }
    }

    let state = dom.lock().unwrap();
    state.document.clone()
}
