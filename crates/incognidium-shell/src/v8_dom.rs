//! DOM bindings for the V8 JavaScript engine (via the `v8` crate).
//!
//! V8 is ~100x faster than Boa and can actually execute modern framework
//! bundles (React, Vue, etc.) in reasonable time.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};

use incognidium_dom::*;

/// Shared DOM state accessible from native JS functions via thread-local.
pub struct DomState {
    pub document: Document,
}

type SharedDom = Arc<Mutex<DomState>>;

thread_local! {
    static DOM: RefCell<Option<SharedDom>> = RefCell::new(None);
    static WRAPPER_CACHE: RefCell<HashMap<NodeId, v8::Global<v8::Object>>> = RefCell::new(HashMap::new());
}

fn cache_get<'s>(scope: &mut v8::HandleScope<'s>, node_id: NodeId) -> Option<v8::Local<'s, v8::Object>> {
    WRAPPER_CACHE.with(|c| {
        c.borrow().get(&node_id).map(|g| v8::Local::new(scope, g))
    })
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

fn noop_empty_arr(
    scope: &mut v8::HandleScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let arr = v8::Array::new(scope, 0);
    rv.set(arr.into());
}

// ── wrap_element ─────────────────────────────────────────────────────────

fn wrap_element<'s>(
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
        if let Some(v) = id_attr {
            set_str(scope, obj, "id", &v);
        }
        if let Some(v) = class_attr {
            set_str(scope, obj, "className", &v);
        }
    } else if node_type == 3 {
        if let Some(t) = text_content {
            set_str(scope, obj, "textContent", &t);
            set_str(scope, obj, "nodeValue", &t);
        }
    }

    // methods
    set_fn(scope, obj, "appendChild", append_child_cb);
    set_fn(scope, obj, "removeChild", remove_child_cb);
    set_fn(scope, obj, "insertBefore", insert_before_cb);
    set_fn(scope, obj, "replaceChild", noop);
    set_fn(scope, obj, "cloneNode", noop);
    set_fn(scope, obj, "remove", noop);
    set_fn(scope, obj, "setAttribute", set_attribute_cb);
    set_fn(scope, obj, "getAttribute", get_attribute_cb);
    set_fn(scope, obj, "hasAttribute", has_attribute_cb);
    set_fn(scope, obj, "removeAttribute", remove_attribute_cb);
    set_fn(scope, obj, "addEventListener", noop);
    set_fn(scope, obj, "removeEventListener", noop);
    set_fn(scope, obj, "dispatchEvent", noop);
    set_fn(scope, obj, "querySelector", noop_null);
    set_fn(scope, obj, "querySelectorAll", noop_empty_arr);
    set_fn(scope, obj, "getElementsByTagName", noop_empty_arr);
    set_fn(scope, obj, "getElementsByClassName", noop_empty_arr);
    set_fn(scope, obj, "getBoundingClientRect", noop);
    set_fn(scope, obj, "focus", noop);
    set_fn(scope, obj, "blur", noop);
    set_fn(scope, obj, "click", noop);
    set_fn(scope, obj, "contains", noop_false);
    set_fn(scope, obj, "matches", noop_false);
    set_fn(scope, obj, "closest", noop_null);
    set_fn(scope, obj, "insertAdjacentHTML", noop);
    set_fn(scope, obj, "insertAdjacentElement", noop);

    // style (stub object w/ setters)
    let style = v8::Object::new(scope);
    set_fn(scope, style, "setProperty", noop);
    set_fn(scope, style, "getPropertyValue", noop_null);
    set_fn(scope, style, "removeProperty", noop);
    let style_key = v8_str(scope, "style");
    obj.set(scope, style_key.into(), style.into());

    // classList
    let classlist = v8::Object::new(scope);
    set_fn(scope, classlist, "add", noop);
    set_fn(scope, classlist, "remove", noop);
    set_fn(scope, classlist, "toggle", noop_false);
    set_fn(scope, classlist, "contains", noop_false);
    set_fn(scope, classlist, "replace", noop);
    let cl_key = v8_str(scope, "classList");
    obj.set(scope, cl_key.into(), classlist.into());

    // dataset
    let ds = v8::Object::new(scope);
    let ds_key = v8_str(scope, "dataset");
    obj.set(scope, ds_key.into(), ds.into());

    // parentNode, firstChild, lastChild, nextSibling, previousSibling, childNodes
    // We snapshot these at wrap time (React generally reads them immediately after
    // creating/querying a node).
    let (parent, first, last, next, prev, children_ids, child_count) = with_dom(|state| {
        if let Some(node) = state.document.nodes.get(node_id) {
            let parent = node.parent;
            let first = node.children.first().copied();
            let last = node.children.last().copied();
            let children_ids = node.children.clone();
            let (next, prev) = if let Some(pid) = node.parent {
                let siblings = &state.document.nodes[pid].children;
                let idx = siblings.iter().position(|&c| c == node_id);
                match idx {
                    Some(i) => (siblings.get(i + 1).copied(), if i > 0 { siblings.get(i - 1).copied() } else { None }),
                    None => (None, None),
                }
            } else {
                (None, None)
            };
            (parent, first, last, next, prev, children_ids.clone(), children_ids.len())
        } else {
            (None, None, None, None, None, Vec::new(), 0)
        }
    });

    let set_node_ref = |scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>, key: &str, nid: Option<NodeId>| {
        let k = v8_str(scope, key);
        match nid {
            Some(n) => {
                let el = wrap_element_shallow(scope, n);
                obj.set(scope, k.into(), el.into());
            }
            None => {
                let null = v8::null(scope);
                obj.set(scope, k.into(), null.into());
            }
        }
    };
    set_node_ref(scope, obj, "parentNode", parent);
    set_node_ref(scope, obj, "parentElement", parent);
    set_node_ref(scope, obj, "firstChild", first);
    set_node_ref(scope, obj, "lastChild", last);
    set_node_ref(scope, obj, "nextSibling", next);
    set_node_ref(scope, obj, "previousSibling", prev);
    set_node_ref(scope, obj, "firstElementChild", first);
    set_node_ref(scope, obj, "lastElementChild", last);
    set_node_ref(scope, obj, "nextElementSibling", next);
    set_node_ref(scope, obj, "previousElementSibling", prev);

    let children_arr = v8::Array::new(scope, child_count as i32);
    for (i, &cid) in children_ids.iter().enumerate() {
        let el = wrap_element_shallow(scope, cid);
        children_arr.set_index(scope, i as u32, el.into());
    }
    let ck = v8_str(scope, "childNodes");
    obj.set(scope, ck.into(), children_arr.into());
    let chk = v8_str(scope, "children");
    obj.set(scope, chk.into(), children_arr.into());
    set_int(scope, obj, "childElementCount", child_count as i32);

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
        if let Some(v) = id_attr {
            set_str(scope, obj, "id", &v);
        }
        if let Some(v) = class_attr {
            set_str(scope, obj, "className", &v);
        }
    } else if node_type == 3 {
        if let Some(t) = text_content {
            set_str(scope, obj, "textContent", &t);
            set_str(scope, obj, "nodeValue", &t);
        }
    }
    set_fn(scope, obj, "appendChild", append_child_cb);
    set_fn(scope, obj, "removeChild", remove_child_cb);
    set_fn(scope, obj, "insertBefore", insert_before_cb);
    set_fn(scope, obj, "setAttribute", set_attribute_cb);
    set_fn(scope, obj, "getAttribute", get_attribute_cb);
    set_fn(scope, obj, "hasAttribute", has_attribute_cb);
    set_fn(scope, obj, "removeAttribute", remove_attribute_cb);
    set_fn(scope, obj, "addEventListener", noop);
    set_fn(scope, obj, "removeEventListener", noop);
    set_fn(scope, obj, "querySelector", noop_null);
    set_fn(scope, obj, "querySelectorAll", noop_empty_arr);
    set_fn(scope, obj, "getElementsByTagName", noop_empty_arr);
    set_fn(scope, obj, "getElementsByClassName", noop_empty_arr);
    set_fn(scope, obj, "contains", noop_false);
    set_fn(scope, obj, "matches", noop_false);
    set_fn(scope, obj, "closest", noop_null);
    set_fn(scope, obj, "getBoundingClientRect", noop);
    let style = v8::Object::new(scope);
    set_fn(scope, style, "setProperty", noop);
    set_fn(scope, style, "getPropertyValue", noop_null);
    set_fn(scope, style, "removeProperty", noop);
    let style_key = v8_str(scope, "style");
    obj.set(scope, style_key.into(), style.into());
    let classlist = v8::Object::new(scope);
    set_fn(scope, classlist, "add", noop);
    set_fn(scope, classlist, "remove", noop);
    set_fn(scope, classlist, "toggle", noop_false);
    set_fn(scope, classlist, "contains", noop_false);
    let cl_key = v8_str(scope, "classList");
    obj.set(scope, cl_key.into(), classlist.into());
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
    rv.set(child_val);
}

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
        state.document.nodes[parent].children.retain(|&c| c != child);
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
    let obj = wrap_element(scope, node_id);
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

// ── install globals ──────────────────────────────────────────────────────

fn install_globals(scope: &mut v8::HandleScope, global: v8::Local<v8::Object>) {
    // console
    let console = v8::Object::new(scope);
    set_fn(scope, console, "log", console_log);
    set_fn(scope, console, "warn", console_warn);
    set_fn(scope, console, "error", console_error);
    set_fn(scope, console, "info", noop);
    set_fn(scope, console, "debug", noop);
    set_fn(scope, console, "trace", noop);
    set_fn(scope, console, "dir", noop);
    set_fn(scope, console, "table", noop);
    set_fn(scope, console, "group", noop);
    set_fn(scope, console, "groupEnd", noop);
    set_fn(scope, console, "time", noop);
    set_fn(scope, console, "timeEnd", noop);
    set_fn(scope, console, "assert", noop);
    set_fn(scope, console, "clear", noop);
    set_fn(scope, console, "count", noop);
    let ck = v8_str(scope, "console");
    global.set(scope, ck.into(), console.into());

    // document
    let doc_obj = v8::Object::new(scope);
    set_fn(scope, doc_obj, "getElementById", get_element_by_id_cb);
    set_fn(scope, doc_obj, "createElement", create_element_cb);
    set_fn(scope, doc_obj, "createElementNS", create_element_cb);
    set_fn(scope, doc_obj, "createTextNode", create_text_node_cb);
    set_fn(scope, doc_obj, "querySelector", query_selector_cb);
    set_fn(scope, doc_obj, "querySelectorAll", noop_empty_arr);
    set_fn(scope, doc_obj, "getElementsByTagName", noop_empty_arr);
    set_fn(scope, doc_obj, "getElementsByClassName", noop_empty_arr);
    set_fn(scope, doc_obj, "getElementsByName", noop_empty_arr);
    set_fn(scope, doc_obj, "addEventListener", noop);
    set_fn(scope, doc_obj, "removeEventListener", noop);
    set_fn(scope, doc_obj, "createEvent", noop);
    set_fn(scope, doc_obj, "createDocumentFragment", create_element_cb);
    set_fn(scope, doc_obj, "createComment", create_text_node_cb);
    set_fn(scope, doc_obj, "createRange", noop);
    set_fn(scope, doc_obj, "execCommand", noop_false);
    set_str(scope, doc_obj, "readyState", "complete");
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
    let dk = v8_str(scope, "document");
    global.set(scope, dk.into(), doc_obj.into());

    // window / self / globalThis already available; set self=window=globalThis
    let wk = v8_str(scope, "window");
    global.set(scope, wk.into(), global.into());
    let sk = v8_str(scope, "self");
    global.set(scope, sk.into(), global.into());

    // navigator
    let nav = v8::Object::new(scope);
    set_str(
        scope,
        nav,
        "userAgent",
        "Mozilla/5.0 (X11; Linux x86_64) Incognidium/0.1",
    );
    set_str(scope, nav, "language", "en-US");
    set_str(scope, nav, "platform", "Linux x86_64");
    set_bool(scope, nav, "cookieEnabled", false);
    set_bool(scope, nav, "onLine", true);
    set_int(scope, nav, "hardwareConcurrency", 4);
    set_str(scope, nav, "appName", "Incognidium");
    set_str(scope, nav, "appVersion", "0.1");
    set_str(scope, nav, "vendor", "");
    set_fn(scope, nav, "sendBeacon", noop_false);
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
    set_fn(scope, loc, "reload", noop);
    set_fn(scope, loc, "replace", noop);
    set_fn(scope, loc, "assign", noop);
    let lk = v8_str(scope, "location");
    global.set(scope, lk.into(), loc.into());

    // history
    let hist = v8::Object::new(scope);
    set_fn(scope, hist, "pushState", noop);
    set_fn(scope, hist, "replaceState", noop);
    set_fn(scope, hist, "back", noop);
    set_fn(scope, hist, "forward", noop);
    set_fn(scope, hist, "go", noop);
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

    // addEventListener/removeEventListener on window
    set_fn(scope, global, "addEventListener", noop);
    set_fn(scope, global, "removeEventListener", noop);
    set_fn(scope, global, "dispatchEvent", noop);
    set_fn(scope, global, "scrollTo", noop);
    set_fn(scope, global, "scrollBy", noop);
    set_fn(scope, global, "scroll", noop);
    set_fn(scope, global, "alert", noop);
    set_fn(scope, global, "confirm", noop_false);
    set_fn(scope, global, "prompt", noop_null);
    set_fn(scope, global, "getComputedStyle", noop);
    set_fn(scope, global, "matchMedia", noop);
    set_fn(scope, global, "requestAnimationFrame", noop);
    set_fn(scope, global, "cancelAnimationFrame", noop);
    set_fn(scope, global, "setTimeout", noop);
    set_fn(scope, global, "clearTimeout", noop);
    set_fn(scope, global, "setInterval", noop);
    set_fn(scope, global, "clearInterval", noop);
    set_fn(scope, global, "queueMicrotask", noop);
    set_fn(scope, global, "fetch", noop);
    set_fn(scope, global, "btoa", noop_null);
    set_fn(scope, global, "atob", noop_null);

    // performance
    let perf = v8::Object::new(scope);
    set_fn(scope, perf, "now", noop);
    set_fn(scope, perf, "mark", noop);
    set_fn(scope, perf, "measure", noop);
    set_fn(scope, perf, "getEntriesByName", noop_empty_arr);
    set_fn(scope, perf, "getEntriesByType", noop_empty_arr);
    let pk = v8_str(scope, "performance");
    global.set(scope, pk.into(), perf.into());

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
}

// ── public entry point ───────────────────────────────────────────────────

const MAX_SCRIPT_SIZE: usize = 16 * 1024 * 1024; // 16MB per script
const MAX_TOTAL_JS: usize = 64 * 1024 * 1024; // 64MB total
const MAX_JS_TIME_SECS: u64 = 30;

pub fn execute_scripts_v8(
    doc: Document,
    scripts: &[super::ScriptEntry],
) -> Document {
    init_v8();
    cache_clear();

    let dom = Arc::new(Mutex::new(DomState { document: doc }));
    set_dom(dom.clone());

    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
    {
        let handle_scope = &mut v8::HandleScope::new(isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        let global = context.global(scope);

        install_globals(scope, global);

        let js_start = std::time::Instant::now();
        let max_time = std::time::Duration::from_secs(MAX_JS_TIME_SECS);
        let mut total_bytes = 0usize;

        for script in scripts {
            if js_start.elapsed() > max_time {
                eprintln!(
                    "JS time limit reached ({:.1}s), skipping remaining scripts",
                    js_start.elapsed().as_secs_f32()
                );
                break;
            }
            if script.source.len() > MAX_SCRIPT_SIZE {
                eprintln!(
                    "JS skip ({}KB > {}KB limit): {}",
                    script.source.len() / 1024,
                    MAX_SCRIPT_SIZE / 1024,
                    script.origin
                );
                continue;
            }
            total_bytes += script.source.len();
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
            let tc = &mut v8::TryCatch::new(scope);
            let source = v8_str(tc, &script.source);
            match v8::Script::compile(tc, source, None) {
                Some(script_obj) => match script_obj.run(tc) {
                    Some(_) => {}
                    None => {
                        let err = tc
                            .exception()
                            .and_then(|e| e.to_string(tc))
                            .map(|s| s.to_rust_string_lossy(tc))
                            .unwrap_or_else(|| "unknown error".into());
                        eprintln!("JS error in {}: {}", script.origin, err);
                    }
                },
                None => {
                    let err = tc
                        .exception()
                        .and_then(|e| e.to_string(tc))
                        .map(|s| s.to_rust_string_lossy(tc))
                        .unwrap_or_else(|| "unknown parse error".into());
                    eprintln!("JS parse error in {}: {}", script.origin, err);
                }
            }
            let elapsed = start.elapsed();
            if elapsed.as_secs() > 3 {
                eprintln!("JS slow ({:.1}s): {}", elapsed.as_secs_f32(), script.origin);
            }
        }
    }

    let _ = take_dom();
    let state = dom.lock().unwrap();
    state.document.clone()
}
