//! Shared logic for incognidium-shell and its binaries.

#[cfg(feature = "v8-engine")]
pub mod v8_dom;

#[cfg(feature = "boa-engine")]
pub mod boa_dom;

use std::collections::HashMap;
use std::path::Path;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::ImageEncoder;
use tiny_skia::Pixmap;

use incognidium_dom::{Document, NodeData};
use incognidium_html::parse_html;
use incognidium_layout::{first_srcset_url, FlatBox, LayoutBox};
use incognidium_net::{fetch_bytes_with_referer, fetch_url, resolve_url};
use incognidium_paint::ImageData;
use incognidium_style::{BackgroundImage, ContainerType, CssColor, Display, SizeValue, StyleMap};
use incognidium_style::{CalcExpression, CalcValue};
use std::sync::Arc;

use incognidium_css::CssValue;

/// A script to execute, with its source code and a label for error messages.
pub struct ScriptEntry {
    pub source: String,
    pub origin: String,
}

/// Look for a `<meta http-equiv="refresh" content="...;url=...">` directive
/// in the raw HTML body. This is the standard server-side/noscript redirect
/// fallback used by some sites, whose meta tag sits inside a
/// `<noscript>` block that the HTML parser treats as raw text. Returns the
/// resolved target URL if one refresh directive is found and it points to a
/// different URL.
pub fn meta_refresh_target(html: &str, base_url: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut search_from = 0usize;
    let mut found: Option<String> = None;

    while let Some(tag_start) = lower[search_from..].find("<meta") {
        let abs_start = search_from + tag_start;
        let tag_end = lower[abs_start..]
            .find('>')
            .map(|i| abs_start + i + 1)
            .unwrap_or(lower.len());
        let tag = &lower[abs_start..tag_end];

        if tag.contains("http-equiv")
            && tag.contains("refresh")
            && !tag.contains("http-equiv=\"expires\"")
        {
            // Find the original-case content attribute in the same tag.
            let original_tag = &html[abs_start..tag_end.min(html.len())];
            if let Some(content_start) = original_tag.to_ascii_lowercase().find("content=") {
                let after = &original_tag[content_start + 8..];
                // content="..." or content='...'
                let quote = after.chars().next()?;
                let end = 1 + after[1..].find(quote)?;
                let content = &after[1..end];
                let lower_content = content.to_ascii_lowercase();
                if let Some(idx) = lower_content.find("url=") {
                    let url_part = content[idx + 4..]
                        .trim()
                        .trim_matches(|c: char| c == '"' || c == '\'');
                    if !url_part.is_empty() {
                        if let Ok(resolved) = resolve_url(base_url, url_part) {
                            if resolved != base_url {
                                if found.is_some() {
                                    // Multiple conflicting refresh directives:
                                    // don't guess.
                                    return None;
                                }
                                found = Some(resolved);
                            }
                        }
                    }
                }
            }
        }
        search_from = tag_end;
    }

    found
}

/// Collect scripts from the DOM in document order, handling both inline and
/// external `<script src="...">` tags.
///
/// - Skips `type="module"` scripts (ES modules not supported)
/// - Limits external script fetches to 20
/// - Maintains document order for execution
pub fn collect_scripts(doc: &incognidium_dom::Document, base_url: &str) -> Vec<ScriptEntry> {
    const MAX_EXTERNAL_SCRIPTS: usize = 20;

    let mut scripts = Vec::new();
    let mut external_count = 0usize;

    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "script" {
                // Skip non-executable script types
                if let Some(script_type) = el.get_attr("type") {
                    let st = script_type.to_lowercase();
                    if st == "module"
                        || st == "application/json"
                        || st == "application/ld+json"
                        || st == "text/template"
                        || st == "text/html"
                        || st == "importmap"
                        || st == "speculationrules"
                    {
                        continue;
                    }
                }

                if let Some(src) = el.get_attr("src") {
                    // External script
                    if external_count >= MAX_EXTERNAL_SCRIPTS {
                        continue;
                    }
                    let resolved = match resolve_url(base_url, src) {
                        Ok(u) => u,
                        Err(e) => {
                            eprintln!("Failed to resolve script URL {src}: {e}");
                            continue;
                        }
                    };
                    match fetch_url(&resolved) {
                        Ok(resp) => {
                            if !resp.body.is_empty() {
                                scripts.push(ScriptEntry {
                                    source: resp.body,
                                    origin: resolved,
                                });
                            }
                            external_count += 1;
                        }
                        Err(e) => {
                            eprintln!("Failed to fetch script {resolved}: {e}");
                            external_count += 1;
                        }
                    }
                } else {
                    // Inline script
                    let mut text = String::new();
                    for &child_id in &node.children {
                        if let incognidium_dom::NodeData::Text(ref t) = doc.nodes[child_id].data {
                            text.push_str(&t.content);
                        }
                    }
                    if !text.is_empty() {
                        scripts.push(ScriptEntry {
                            source: text,
                            origin: format!("inline <script> in {}", base_url),
                        });
                    }
                }
            }
        }
    }
    scripts
}

/// Execute scripts using whichever JS engine is enabled at build time.
/// With `v8-engine` (default): fast, runs real framework bundles.
/// With `boa-engine`: pure Rust, no external C++ engine bindings, slower.
/// Env `INCOGNIDIUM_JS=off` skips JS entirely.
pub fn execute_scripts_on_doc(
    doc: incognidium_dom::Document,
    scripts: &[ScriptEntry],
    _image_cache: &mut HashMap<String, ImageData>,
    base_url: &str,
) -> incognidium_dom::Document {
    let mut doc = doc;
    if std::env::var("INCOGNIDIUM_JS").ok().as_deref() == Some("off") {
        preprocess_document(&mut doc, base_url);
        return doc;
    }
    #[cfg(feature = "v8-engine")]
    {
        doc = v8_dom::execute_scripts_v8(doc, scripts);
    }
    #[cfg(all(feature = "boa-engine", not(feature = "v8-engine")))]
    {
        doc = boa_dom::execute_scripts_boa(doc, scripts);
    }
    #[cfg(not(any(feature = "v8-engine", feature = "boa-engine")))]
    {
        let _ = scripts;
    }
    preprocess_document(&mut doc, base_url);
    doc
}

/// Append a fetched stylesheet body to `out`, following its @import rules.
///
/// Imported sheets are fetched through `resolve_and_fetch(base_url, href) ->
/// Option<(resolved_url, body)>` and inlined where the @import appears, so
/// pages that load their design tokens through @import get them applied.
/// Conditional imports — `@import url(x) (prefers-color-scheme: dark)` — are
/// wrapped in their matching @media block so the stylesheet parser evaluates
/// the gate exactly like an @media rule. `seen` breaks import cycles; nesting
/// is capped by `MAX_IMPORT_DEPTH`.
pub fn append_stylesheet_with_imports(
    out: &mut String,
    body: &str,
    base_url: &str,
    resolve_and_fetch: &mut dyn FnMut(&str, &str) -> Option<(String, String)>,
    seen: &mut std::collections::HashSet<String>,
    depth: usize,
) {
    const MAX_IMPORT_DEPTH: usize = 4;

    // Relative url() references in this sheet resolve against this sheet's own
    // URL; rewrite them before the sheet is inlined into the combined
    // stylesheet, which no longer carries that origin.
    let body = &absolutize_css_urls(body, base_url);

    if depth >= MAX_IMPORT_DEPTH {
        out.push_str(body);
        out.push('\n');
        return;
    }

    let stylesheet = incognidium_css::parse_css(body);
    if stylesheet.imports.is_empty() {
        out.push_str(body);
        out.push('\n');
        return;
    }

    // An imported sheet is treated as if its contents replaced the @import
    // rule, so the imported rules come before the importing sheet's own rules
    // and lose same-specificity cascade ties to them.
    let mut imported = String::new();
    for rule in &stylesheet.imports {
        if let Some((url, import_body)) = resolve_and_fetch(base_url, &rule.url) {
            if !seen.insert(url.clone()) {
                continue;
            }
            let gate = rule
                .media
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("all"));
            match gate {
                Some(m) => {
                    imported.push_str(&format!("@media {} {{\n", m));
                    append_stylesheet_with_imports(
                        &mut imported,
                        &import_body,
                        &url,
                        resolve_and_fetch,
                        seen,
                        depth + 1,
                    );
                    imported.push_str("\n}\n");
                }
                None => append_stylesheet_with_imports(
                    &mut imported,
                    &import_body,
                    &url,
                    resolve_and_fetch,
                    seen,
                    depth + 1,
                ),
            }
        }
    }
    out.push_str(&imported);
    out.push_str(body);
    out.push('\n');
}

/// Rewrite relative `url(...)` references in a stylesheet body to absolute URLs
/// resolved against the sheet's own URL.
///
/// Per CSS, relative references inside a stylesheet (`@font-face` sources,
/// background images, …) resolve against the stylesheet's URL, not the
/// document's. Fetched sheets are inlined into one combined stylesheet before
/// parsing, which loses that origin — rewriting here keeps every inlined rule
/// pointing at the right host.
fn absolutize_css_urls(body: &str, base_url: &str) -> String {
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut in_comment = false;
    let mut in_string: Option<u8> = None;
    while i < len {
        let b = bytes[i];
        if in_comment {
            if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                out.push_str("*/");
                i += 2;
                in_comment = false;
                continue;
            }
            out.push(b as char);
            i += 1;
            continue;
        }
        if let Some(q) = in_string {
            if b == b'\\' && i + 1 < len {
                out.push(b as char);
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if Some(b) == in_string {
                in_string = None;
            }
            out.push(b as char);
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            in_comment = true;
            out.push_str("/*");
            i += 2;
            continue;
        }
        if b == b'"' || b == b'\'' {
            in_string = Some(b);
            out.push(b as char);
            i += 1;
            continue;
        }
        // Match `url(` only outside comments and strings, and only where it is
        // a token of its own (not part of an identifier like `background:`).
        let is_url_token = b == b'u'
            && body[i..].len() >= 4
            && body[i..].get(..4).map(|s| s.eq_ignore_ascii_case("url(")) == Some(true)
            && !out.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if is_url_token {
            let open = i + 3;
            // Collect the argument up to the closing parenthesis.
            let mut j = open + 1;
            let mut arg = String::new();
            let mut arg_quote: Option<u8> = None;
            let mut closed = false;
            while j < len {
                let cb = bytes[j];
                if let Some(q) = arg_quote {
                    if cb == b'\\' && j + 1 < len {
                        arg.push(cb as char);
                        arg.push(bytes[j + 1] as char);
                        j += 2;
                        continue;
                    }
                    if cb == q {
                        arg_quote = None;
                        j += 1;
                        continue;
                    }
                    arg.push(cb as char);
                    j += 1;
                    continue;
                }
                if cb == b'"' || cb == b'\'' {
                    arg_quote = Some(cb);
                    j += 1;
                    continue;
                }
                if cb == b')' {
                    closed = true;
                    j += 1;
                    break;
                }
                arg.push(cb as char);
                j += 1;
            }
            out.push_str("url(");
            if closed {
                let trimmed = arg.trim();
                let leave_verbatim = trimmed.is_empty()
                    || trimmed.starts_with("http://")
                    || trimmed.starts_with("https://")
                    || trimmed.starts_with("data:")
                    || trimmed.starts_with('#')
                    || trimmed.eq_ignore_ascii_case("none");
                if leave_verbatim {
                    out.push_str(&arg);
                } else if let Ok(resolved) = resolve_url(base_url, trimmed) {
                    out.push('"');
                    out.push_str(&resolved);
                    out.push('"');
                } else {
                    out.push_str(&arg);
                }
                out.push(')');
                i = j;
                continue;
            }
            // Unterminated: emit the rest verbatim.
            out.push_str(&arg);
            out.push(')');
            i = j;
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    out
}

/// Strip dark mode styles from CSS text.
///
/// Some pages ship both light and dark variable sets. The dark set can arrive
/// inside `prefers-color-scheme: dark` media queries or in plain rules keyed
/// off a night theme class. Because our renderer does not report a real
/// color-scheme preference, those blocks can end up matching and turning the
/// page black. Removing them leaves the light/default styles intact.
pub fn strip_dark_mode_media_queries(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut i = 0usize;
    let bytes = css.as_bytes();
    let len = bytes.len();

    fn is_night_selector(sel: &str) -> bool {
        let lower = sel.to_ascii_lowercase();
        lower.contains("-night") && lower.contains("theme")
    }

    while i < len {
        // Find next @media
        if let Some(at_pos) = css[i..].find("@media") {
            let at_pos = i + at_pos;
            out.push_str(&css[i..at_pos]);

            // Find the opening brace
            let after_at = at_pos + 6;
            let open = css[after_at..].find('{').map(|p| after_at + p);
            if let Some(open_pos) = open {
                let prelude = css[after_at..open_pos].to_ascii_lowercase();
                // Check if this media query is for dark color scheme
                let is_dark_media =
                    prelude.contains("prefers-color-scheme") && prelude.contains("dark");
                if is_dark_media {
                    // Skip this block by brace counting
                    let mut depth = 1usize;
                    let mut j = open_pos + 1;
                    while j < len && depth > 0 {
                        match bytes[j] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    i = j;
                } else {
                    // Keep the block
                    out.push_str(&css[at_pos..=open_pos]);
                    i = open_pos + 1;
                }
            } else {
                // Malformed: no opening brace; keep rest
                out.push_str(&css[at_pos..]);
                break;
            }
        } else {
            out.push_str(&css[i..]);
            break;
        }
    }

    // Second pass: strip top-level rules whose selector is a night theme class.
    // This handles `html.skin-theme-clientpref-night { ... }` and similar.
    let mut final_out = String::with_capacity(out.len());
    let mut k = 0usize;
    while k < out.len() {
        // Find next top-level opening brace. We look for the first '{' after k
        // that is not inside a string/comment is sufficient for this stripper.
        if let Some(open_pos) = out[k..].find('{') {
            let open_pos = k + open_pos;
            // Walk back to find the start of the selector
            let sel_start = {
                let mut s = open_pos;
                while s > k {
                    if out.as_bytes()[s - 1] == b'}' || out.as_bytes()[s - 1] == b'{' {
                        break;
                    }
                    s -= 1;
                }
                s
            };
            let selector = &out[sel_start..open_pos];
            if is_night_selector(selector) {
                // Skip this rule
                let mut depth = 1usize;
                let mut j = open_pos + 1;
                while j < out.len() && depth > 0 {
                    match out.as_bytes()[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                k = j;
            } else {
                final_out.push_str(&out[k..=open_pos]);
                k = open_pos + 1;
            }
        } else {
            final_out.push_str(&out[k..]);
            break;
        }
    }

    final_out
}

/// Check whether a node or any of its descendants contributes visible content.
///
/// Used by the placeholder trimmers to decide whether an element that looks
/// like a wrapper is actually empty (no text, no images/media, no form
/// controls, and no meaningful accessibility text).
fn has_visible_content(doc: &Document, id: incognidium_dom::NodeId) -> bool {
    let node = &doc.nodes[id];
    match &node.data {
        incognidium_dom::NodeData::Text(t) => !t.content.trim().is_empty(),
        incognidium_dom::NodeData::Element(el) => {
            if matches!(
                el.tag_name.as_str(),
                "img"
                    | "picture"
                    | "video"
                    | "audio"
                    | "svg"
                    | "canvas"
                    | "iframe"
                    | "object"
                    | "embed"
                    | "input"
                    | "textarea"
                    | "select"
                    | "button"
            ) {
                return true;
            }
            for attr in ["alt", "aria-label", "title", "placeholder"] {
                if let Some(v) = el.get_attr(attr) {
                    if !v.trim().is_empty() {
                        return true;
                    }
                }
            }
            node.children
                .iter()
                .any(|&cid| has_visible_content(doc, cid))
        }
        _ => false,
    }
}

/// Strip lazy-image skeleton wrappers that would otherwise paint as large gray
/// blocks when their images are not loaded.
///
/// Strip lazy-image skeleton wrappers that would otherwise paint as large gray
/// blocks when their images are not loaded.
///
/// Pages commonly use skeleton wrappers like
/// `<div class="w-full aspect-[16/9] bg-gray-100 animate-pulse"><img loading="lazy" ...></div>`.
/// The `onload` handler that removes the skeleton classes never fires in the
/// headless renderer, so each wrapper can reserve an oversized gray rectangle.
/// This helper removes the skeleton background and aspect-ratio classes from
/// such wrappers and turns the contained `<img>` into `loading="eager"`, so the
/// wrapper either renders the loaded image or collapses cleanly.
pub fn strip_lazy_image_skeletons(doc: &mut Document) {
    let mut to_strip: Vec<(incognidium_dom::NodeId, String)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let cls = el.classes();
            let has_skeleton_bg = cls.iter().any(|c| {
                c.starts_with("bg-gray-")
                    || c.starts_with("bg-slate-")
                    || c.starts_with("bg-zinc-")
                    || c.starts_with("bg-neutral-")
                    || c.starts_with("bg-stone-")
                    || c.starts_with("bg-carbon-")
            });
            // Utility animation classes used for skeleton pulses. Match both the
            // base class and prefixed variants (`motion-safe:animate-pulse`, etc.).
            let has_pulse = cls.iter().any(|c| {
                *c == "animate-pulse"
                    || c.ends_with(":animate-pulse")
                    || *c == "animate-ping"
                    || c.ends_with(":animate-ping")
                    || *c == "animate-bounce"
                    || c.ends_with(":animate-bounce")
            });
            let has_shimmer = cls.iter().any(|c| c.starts_with("shimmer_"));
            if !has_skeleton_bg && !has_pulse && !has_shimmer {
                continue;
            }
            // Only strip if the wrapper contains a lazy <img>.
            let has_lazy_img = doc.nodes[id].children.iter().any(|&cid| {
                if let NodeData::Element(child_el) = &doc.nodes[cid].data {
                    child_el.tag_name == "img" && child_el.get_attr("loading") == Some("lazy")
                } else {
                    false
                }
            });
            if !has_lazy_img {
                continue;
            }
            let new_classes: Vec<&str> = cls
                .into_iter()
                .filter(|c| {
                    !c.starts_with("aspect-")
                        && !c.starts_with("bg-gray-")
                        && !c.starts_with("bg-slate-")
                        && !c.starts_with("bg-zinc-")
                        && !c.starts_with("bg-neutral-")
                        && !c.starts_with("bg-stone-")
                        && !c.starts_with("bg-carbon-")
                        && !c.starts_with("shimmer_")
                        && !(*c == "animate-pulse"
                            || (*c).ends_with(":animate-pulse")
                            || *c == "animate-ping"
                            || (*c).ends_with(":animate-ping")
                            || *c == "animate-bounce"
                            || (*c).ends_with(":animate-bounce"))
                })
                .collect();
            to_strip.push((id, new_classes.join(" ")));
        }
    }

    let stripped_count = to_strip.len();
    let wrapper_ids: std::collections::HashSet<incognidium_dom::NodeId> =
        to_strip.iter().map(|(id, _)| *id).collect();

    for (id, new_classes) in to_strip {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            if new_classes.is_empty() {
                el.attributes.remove("class");
            } else {
                el.attributes.insert("class".to_string(), new_classes);
            }
        }
    }

    // Convert only the lazy images inside the stripped skeleton wrappers to eager,
    // so we don't blow through the global image-fetch cap for every lazy image
    // on unrelated sites.
    let mut eager_count = 0usize;
    let mut stack: Vec<incognidium_dom::NodeId> = wrapper_ids.iter().copied().collect();
    while let Some(id) = stack.pop() {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            if el.tag_name == "img" && el.get_attr("loading") == Some("lazy") {
                el.attributes
                    .insert("loading".to_string(), "eager".to_string());
                eager_count += 1;
            }
        }
        for &cid in &doc.nodes[id].children {
            stack.push(cid);
        }
    }

    if stripped_count > 0 || eager_count > 0 {
        eprintln!(
            "Stripped {} lazy-image skeleton(s), converted {} image(s) to eager",
            stripped_count, eager_count
        );
    }
}

/// Strip inline `background-color` styles from image wrappers that act as lazy-load
/// placeholders (e.g. `<a style="background-color:var(--gray400)">`).
///
/// When scripting is enabled the site's JS removes these placeholders after the
/// image loads, but in the headless renderer the inline style persists and paints
/// a gray box behind/around the image.
pub fn strip_inline_bg_placeholders(doc: &mut Document) {
    let mut stripped = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let Some(style) = el.get_attr("style") else {
                continue;
            };
            if !style.to_lowercase().contains("background-color") {
                continue;
            }
            // Only strip if this element directly contains an <img> with a src.
            let has_img_with_src = doc.nodes[id].children.iter().any(|&cid| {
                if let NodeData::Element(child_el) = &doc.nodes[cid].data {
                    child_el.tag_name == "img" && child_el.get_attr("src").is_some()
                } else {
                    false
                }
            });
            if !has_img_with_src {
                continue;
            }
            // Remove the background-color declaration from the inline style.
            let cleaned = remove_bg_color_from_style(style);
            if cleaned != style {
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    if cleaned.trim().is_empty() {
                        el_mut.attributes.remove("style");
                    } else {
                        el_mut.attributes.insert("style".to_string(), cleaned);
                    }
                    stripped += 1;
                }
            }
        }
    }
    if stripped > 0 {
        eprintln!(
            "Stripped {} inline background-color placeholder(s)",
            stripped
        );
    }
}

/// Strip `alt` attributes from images whose captions or adjacent headings already
/// contain the same text.
///
/// Real browsers do not show `alt` text when the image renders, but Incognidium's
/// extracted text (and any broken-image fallback) lays it out, so a caption/title
/// can appear twice. Empty the `alt` on images when a sibling (or nearby
/// descendant of the same card/figure/list item) already shows the same text.
/// The check is generic: it only strips when a duplicate is actually found, so
/// accessibility-only descriptions are preserved.
pub fn strip_duplicate_img_alt_text(doc: &mut Document, _base_url: &str) {
    let mut stripped = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "img" {
                continue;
            }
            let alt = el.get_attr("alt").unwrap_or("");
            if alt.is_empty() {
                continue;
            }

            // Look at siblings and descendants of each ancestor up to the
            // nearest content wrapper (card, article, figure, or list item).
            let mut cur = doc.nodes[id].parent;
            let mut found_dup = false;
            while let Some(pid) = cur {
                if let NodeData::Element(parent_el) = &doc.nodes[pid].data {
                    let parent_class = parent_el.get_attr("class").unwrap_or("");

                    // Direct sibling caption/heading/link text.
                    for &cid in &doc.nodes[pid].children {
                        if cid == id {
                            continue;
                        }
                        if let NodeData::Element(sib) = &doc.nodes[cid].data {
                            let sib_class = sib.get_attr("class").unwrap_or("");
                            if sib_class.contains("credit-caption")
                                || sib.tag_name == "figcaption"
                                || sib.tag_name == "h1"
                                || sib.tag_name == "h2"
                                || sib.tag_name == "h3"
                                || sib.tag_name == "h4"
                                || sib.tag_name == "h5"
                                || sib.tag_name == "h6"
                                || sib.tag_name == "a"
                            {
                                if subtree_contains_text(doc, cid, alt)
                                    || subtree_text_contained_in(doc, cid, alt)
                                {
                                    found_dup = true;
                                    break;
                                }
                            }
                        }
                    }
                    if found_dup {
                        break;
                    }

                    // Also accept a figcaption nested anywhere inside the figure.
                    for &cid in &doc.nodes[pid].children {
                        if cid == id {
                            continue;
                        }
                        if subtree_has_figcaption_with_text(doc, cid, alt) {
                            found_dup = true;
                            break;
                        }
                    }
                    if found_dup {
                        break;
                    }

                    // Card/list wrappers may hold the image in one child and the
                    // headline link in another child (or deeper descendant).
                    for &cid in &doc.nodes[pid].children {
                        if cid == id {
                            continue;
                        }
                        if subtree_contains_heading_or_link_text(doc, cid, alt) {
                            found_dup = true;
                            break;
                        }
                    }
                    if found_dup {
                        break;
                    }

                    let tag = parent_el.tag_name.as_str();
                    if tag == "figure"
                        || parent_class.contains("credit-caption")
                        || tag == "article"
                        || tag == "li"
                        || tag == "ul"
                        || tag == "ol"
                        || parent_class.contains("card")
                        || parent_class.contains("tout")
                        || parent_class.contains("content-cards")
                        || parent_class.contains("group")
                    {
                        // Stop walking once we've inspected the likely content wrapper.
                        break;
                    }
                }
                cur = doc.nodes[pid].parent;
            }
            if found_dup {
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    el_mut.attributes.insert("alt".to_string(), "".to_string());
                    stripped += 1;
                }
            }
        }
    }

    if stripped > 0 {
        eprintln!("Stripped {} duplicate image alt attribute(s)", stripped);
    }
}

/// Remove duplicate `alt` text from `<noscript>` fallback images.
///
/// Lazy-loading images are often shipped as an empty `<img>` (or `<picture>`) plus
/// a `<noscript><img></noscript>` fallback carrying the real `src` and `alt`.
/// The HTML parser keeps both images in the DOM, so the `alt` text is extracted
/// twice. Real browsers with scripting enabled do not render the `<noscript>`
/// subtree at all. When a `<noscript>` image sits inside the same figure, card,
/// or list item as a visible image or picture, clear its `alt` so the description
/// appears once.
pub fn dedupe_noscript_image_alts(doc: &mut Document) {
    let mut cleared = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "noscript" {
                continue;
            }
        } else {
            continue;
        }

        // Collect <img> descendants inside this <noscript>.
        let mut noscript_imgs: Vec<incognidium_dom::NodeId> = Vec::new();
        let mut stack = vec![id];
        while let Some(cid) = stack.pop() {
            if let NodeData::Element(child_el) = &doc.nodes[cid].data {
                if child_el.tag_name == "img" {
                    noscript_imgs.push(cid);
                }
            }
            stack.extend(doc.nodes[cid].children.iter().copied());
        }
        if noscript_imgs.is_empty() {
            continue;
        }

        // Determine whether this noscript lives inside a figure/article/card that
        // already contains a visible <img> or <picture> outside the noscript.
        let mut has_visible_media = false;
        let mut cur = doc.nodes[id].parent;
        while let Some(pid) = cur {
            if let NodeData::Element(parent_el) = &doc.nodes[pid].data {
                let tag = parent_el.tag_name.as_str();
                if tag == "figure" || tag == "article" || tag == "li" || tag == "a" {
                    let mut stack = vec![pid];
                    while let Some(sid) = stack.pop() {
                        if sid == id {
                            // Don't descend into the noscript itself.
                            continue;
                        }
                        if let NodeData::Element(sib_el) = &doc.nodes[sid].data {
                            if sib_el.tag_name == "img" || sib_el.tag_name == "picture" {
                                has_visible_media = true;
                                break;
                            }
                        }
                        stack.extend(doc.nodes[sid].children.iter().copied());
                    }
                    break;
                }
            }
            cur = doc.nodes[pid].parent;
        }

        if !has_visible_media {
            continue;
        }

        for img_id in noscript_imgs {
            if let NodeData::Element(el_mut) = &mut doc.node_mut(img_id).data {
                if !el_mut.get_attr("alt").unwrap_or("").is_empty() {
                    el_mut.attributes.insert("alt".to_string(), "".to_string());
                    cleared += 1;
                }
            }
        }
    }

    if cleared > 0 {
        eprintln!(
            "Cleared {} duplicate alt attribute(s) on <noscript> fallback image(s)",
            cleared
        );
    }
}

/// Remove `aria-label` attributes that duplicate visible descendant text.
///
/// Some pages put the whole headline in an anchor's `aria-label` while also
/// rendering the same text inside the link.
/// Because Incognidium treats `aria-label` as a generated text box, the
/// headline appears twice in the no-JS layout. When the label text is already
/// present in the subtree, drop the attribute; otherwise keep it for
/// accessibility-only controls such as icon buttons.
pub fn strip_duplicate_aria_labels(doc: &mut Document) {
    let mut stripped = 0usize;
    for id in 0..doc.nodes.len() {
        let label = {
            if let NodeData::Element(el) = &doc.nodes[id].data {
                el.get_attr("aria-label").map(|s| s.to_string())
            } else {
                None
            }
        };
        let label = match label {
            Some(l) if !l.trim().is_empty() => l.trim().to_string(),
            _ => continue,
        };

        if subtree_contains_text(doc, id, &label) {
            if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                el_mut.attributes.remove("aria-label");
                stripped += 1;
            }
        }
    }

    if stripped > 0 {
        eprintln!("Stripped {} duplicate aria-label attribute(s)", stripped);
    }
}

fn subtree_contains_heading_or_link_text(
    doc: &Document,
    node_id: incognidium_dom::NodeId,
    text: &str,
) -> bool {
    if let NodeData::Element(el) = &doc.nodes[node_id].data {
        if el.tag_name == "a"
            || el.tag_name == "h1"
            || el.tag_name == "h2"
            || el.tag_name == "h3"
            || el.tag_name == "h4"
            || el.tag_name == "h5"
            || el.tag_name == "h6"
        {
            if subtree_contains_text(doc, node_id, text)
                || subtree_text_contained_in(doc, node_id, text)
            {
                return true;
            }
        }
    }
    for &cid in &doc.nodes[node_id].children {
        if subtree_contains_heading_or_link_text(doc, cid, text) {
            return true;
        }
    }
    false
}

fn subtree_contains_text(doc: &Document, node_id: incognidium_dom::NodeId, text: &str) -> bool {
    if let NodeData::Text(t) = &doc.nodes[node_id].data {
        if t.content.trim().contains(text) {
            return true;
        }
    }
    for &cid in &doc.nodes[node_id].children {
        if subtree_contains_text(doc, cid, text) {
            return true;
        }
    }
    false
}

fn subtree_text_contained_in(doc: &Document, node_id: incognidium_dom::NodeId, text: &str) -> bool {
    if let NodeData::Text(t) = &doc.nodes[node_id].data {
        let trimmed = t.content.trim();
        if !trimmed.is_empty() && text.contains(trimmed) {
            return true;
        }
    }
    for &cid in &doc.nodes[node_id].children {
        if subtree_text_contained_in(doc, cid, text) {
            return true;
        }
    }
    false
}

fn subtree_has_figcaption_with_text(
    doc: &Document,
    node_id: incognidium_dom::NodeId,
    text: &str,
) -> bool {
    if let NodeData::Element(el) = &doc.nodes[node_id].data {
        if el.tag_name == "figcaption"
            && (subtree_contains_text(doc, node_id, text)
                || subtree_text_contained_in(doc, node_id, text))
        {
            return true;
        }
    }
    for &cid in &doc.nodes[node_id].children {
        if subtree_has_figcaption_with_text(doc, cid, text) {
            return true;
        }
    }
    false
}

/// Remove `<title>` and `<desc>` elements inside inline SVGs.
///
/// SVG metadata children are meant for accessibility trees and tooltips, not
/// visible page text. Incognidium's static renderer extracts them as text
/// boxes, so logos and icons leak labels like "Example logo" into the layout.
/// Real browsers never render this content, so drop it during preprocessing.
pub fn strip_svg_metadata_text(doc: &mut Document) {
    let mut parent_map: HashMap<incognidium_dom::NodeId, incognidium_dom::NodeId> = HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "title" && el.tag_name != "desc" {
                continue;
            }
        } else {
            continue;
        }

        let mut cur = parent_map.get(&id).copied();
        let mut inside_svg = false;
        let mut visited: std::collections::HashSet<incognidium_dom::NodeId> =
            std::collections::HashSet::new();
        while let Some(pid) = cur {
            if !visited.insert(pid) {
                break;
            }
            if let NodeData::Element(parent_el) = &doc.nodes[pid].data {
                if parent_el.tag_name == "svg" {
                    inside_svg = true;
                    break;
                }
            }
            cur = parent_map.get(&pid).copied();
        }

        if inside_svg {
            to_remove.push(id);
        }
    }

    if to_remove.is_empty() {
        return;
    }

    let removed_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();
    for id in to_remove {
        if let Some(&pid) = parent_map.get(&id) {
            if !removed_set.contains(&pid) {
                doc.nodes[pid].children.retain(|&cid| cid != id);
            }
        }
    }

    eprintln!(
        "Removed {} SVG <title>/<desc> metadata element(s)",
        removed_set.len()
    );
}

/// Fix Next.js `data-nimg="fill"` images that are hidden by CSS classes
/// (e.g. `._1ismqjf{display:none}`) because the JS that toggles visibility
/// never runs in the headless renderer.
pub fn fix_nextjs_fill_images(doc: &mut Document) {
    let mut fixed = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "img" {
                continue;
            }
            if el.get_attr("data-nimg") != Some("fill") {
                continue;
            }
            let has_src = el.get_attr("src").map(|s| !s.is_empty()).unwrap_or(false);
            if !has_src {
                continue;
            }
            // Ensure the image is visible by adding display:block to inline style.
            let style = el.get_attr("style").unwrap_or("");
            let lower = style.to_lowercase();
            if lower.contains("display:none") {
                // Replace display:none with display:block
                let new_style = lower
                    .split(';')
                    .map(|s| {
                        let t = s.trim();
                        if t.starts_with("display:none") {
                            "display: block"
                        } else {
                            t
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    el_mut.attributes.insert("style".to_string(), new_style);
                    fixed += 1;
                }
            } else if !lower.contains("display:") {
                // No display property at all - add display:block
                let new_style = if style.trim().is_empty() {
                    "display: block".to_string()
                } else {
                    format!("{}; display: block", style.trim_end_matches(';'))
                };
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    el_mut.attributes.insert("style".to_string(), new_style);
                    fixed += 1;
                }
            }
        }
    }
    if fixed > 0 {
        eprintln!("Fixed {} Next.js data-nimg='fill' image(s)", fixed);
    }
}

/// Remove `background-color: ...;` (and variants) from a CSS style string.
fn remove_bg_color_from_style(style: &str) -> String {
    let mut result = String::new();
    for decl in style.split(';') {
        let trimmed = decl.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("background-color") {
            continue;
        }
        if !result.is_empty() {
            result.push_str("; ");
        }
        result.push_str(trimmed);
    }
    result
}

/// Promote lazy-loaded `<img>` sources so they render without JS.
///
/// Pages commonly set `src` to a 1x1 transparent placeholder and put the real
/// URL in `data-src`, hiding the image with `.lazyload[data-src]{display:none}`.
/// Without JS the lazy loader never swaps the attributes, so thumbnails and
/// hero images stay invisible. This helper copies `data-src` to `src`, copies
/// `data-srcset` to `srcset`, flips the lazy class to `lazyloaded`, and forces
/// `loading="eager"` so the image is fetched and laid out like a normal `<img>`.
pub fn promote_lazy_image_sources(doc: &mut Document) {
    let mut promoted = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            if el.tag_name != "img" {
                continue;
            }
            // Read all attributes into owned locals before mutating the element.
            let class = el.get_attr("class").unwrap_or("").to_string();
            let src = el.get_attr("src").unwrap_or("").to_string();
            let data_src = el.get_attr("data-src").map(String::from);
            let data_default_src = el.get_attr("data-default-src").map(String::from);
            let data_srcset = el.get_attr("data-srcset").map(String::from);
            let loading = el.get_attr("loading").map(String::from);
            let has_srcset = el.get_attr("srcset").is_some();

            // Use the real source when the current src is missing or looks like a
            // tiny placeholder (data URI, about:blank, or a known transparent GIF).
            let src_is_placeholder = src.is_empty()
                || src.starts_with("data:")
                || src == "about:blank"
                || src == "about:srcdoc"
                || src.ends_with("clear-16x9.gif")
                || src.ends_with("blank.gif")
                || src.ends_with("spacer.gif")
                || src.ends_with("transparent.gif");

            // Some lazy-load libraries use data-src without the standard
            // lazyload/lazyloading class names. Promote any image that has a
            // data-src and a missing/placeholder src. Others use data-default-src
            // for lazy-loaded article images.
            let should_promote =
                (data_src.is_some() || data_default_src.is_some()) && src_is_placeholder;

            let classes: Vec<&str> = class.split_whitespace().collect();
            let has_lazy_class = classes
                .iter()
                .any(|c| *c == "lazyload" || *c == "lazyloading");

            // CSS-module-style lazy classes (e.g. `Image_lazy__...`) hide the
            // image with opacity:0 until JS adds a loaded class.
            let has_css_module_lazy = classes
                .iter()
                .any(|c| c.contains("_lazy__") || c.contains("-lazy-"));
            let has_lazy_loading = loading.as_deref() == Some("lazy");

            if !should_promote && !has_lazy_class && !has_css_module_lazy {
                continue;
            }

            // Prefer data-src over data-default-src when both are present.
            let real_src = data_src.or(data_default_src);
            if let Some(real) = real_src {
                if src_is_placeholder {
                    el.attributes.insert("src".to_string(), real);
                    promoted += 1;
                }
            }
            if let Some(realset) = data_srcset {
                if !has_srcset {
                    el.attributes.insert("srcset".to_string(), realset);
                }
            }

            // Swap lazy classes to lazyloaded so the `.lazyload[data-src]{display:none}`
            // rule no longer matches and the image becomes visible.
            // Also strip CSS-module lazy classes that set opacity:0.
            let new_classes: Vec<&str> = classes
                .iter()
                .filter(|c| !c.contains("_lazy__") && !c.contains("-lazy-"))
                .map(|c| match *c {
                    "lazyload" | "lazyloading" => "lazyloaded",
                    _ => *c,
                })
                .collect();
            let new_class = new_classes.join(" ");
            if new_class.is_empty() {
                el.attributes.remove("class");
            } else {
                el.attributes.insert("class".to_string(), new_class);
            }

            // Ensure the image is fetched even if it was originally below the fold.
            if has_lazy_loading {
                el.attributes
                    .insert("loading".to_string(), "eager".to_string());
            }
        }
    }
    if promoted > 0 {
        eprintln!("Promoted {} lazy image source(s) to src", promoted);
    }
}

/// Remove empty placeholder containers that real browsers hide or fill dynamically.
///
/// Server HTML often includes ad slots, tracking widgets, and CMS placeholder
/// boxes (e.g. `markupbox`, `ad-slot`, `dfp-ad`, `adsbygoogle`, `taboola`,
/// `outbrain`). Without the corresponding ad/tracking JS these boxes have no
/// visible content, but they still occupy CSS-generated height (padding,
/// min-height, margins). This helper drops any such subtree that contains no
/// real content: no text, no images, no media, no form controls, and no
/// meaningful accessibility text. Visible placeholders (e.g. a logo inside a
/// `markupbox`) are preserved.
///
/// It also removes subtrees marked `aria-hidden="true"` when they have no
/// visible content, which is common for off-screen/hidden ad slots.
pub fn remove_empty_placeholders(doc: &mut Document) {
    // Precompute which nodes contain a visual replaced element (image, video,
    // SVG, canvas, etc.) somewhere in their subtree. Accessibility-only
    // wrappers are often marked `aria-hidden="true"`, but many of those wrappers
    // carry real visual content such as article cover images. Don't strip them.
    let mut has_visual_descendant: Vec<bool> = vec![false; doc.nodes.len()];
    for id in (0..doc.nodes.len()).rev() {
        if let incognidium_dom::NodeData::Element(ref el) = doc.nodes[id].data {
            if matches!(
                el.tag_name.as_str(),
                "img"
                    | "svg"
                    | "video"
                    | "audio"
                    | "canvas"
                    | "picture"
                    | "iframe"
                    | "object"
                    | "embed"
            ) {
                has_visual_descendant[id] = true;
            }
        }
        if has_visual_descendant[id] {
            if let Some(parent_id) = doc.nodes[id].parent {
                has_visual_descendant[parent_id] = true;
            }
        }
    }

    // Precompute which nodes contain something that could become visible: a
    // non-empty text node or an element with meaningful descendants. Pages use
    // the generic `hidden` class to toggle visibility via JS or responsive CSS
    // (e.g. `.js-enabled .hidden { display: block; }`). Keeping contentful
    // hidden containers lets later style resolution reveal them; only truly empty
    // hidden wrappers are treated as placeholders.
    let mut has_meaningful_content: Vec<bool> = vec![false; doc.nodes.len()];
    for id in (0..doc.nodes.len()).rev() {
        let node = &doc.nodes[id];
        let mut meaningful = match &node.data {
            incognidium_dom::NodeData::Text(t) => !t.content.trim().is_empty(),
            _ => false,
        };
        if !meaningful {
            for &child_id in &node.children {
                if has_meaningful_content[child_id] {
                    meaningful = true;
                    break;
                }
            }
        }
        has_meaningful_content[id] = meaningful;
        if meaningful {
            if let Some(parent_id) = node.parent {
                has_meaningful_content[parent_id] = true;
            }
        }
    }

    fn is_placeholder(
        el: &incognidium_dom::ElementData,
        has_visual_descendant: bool,
        has_meaningful_content: bool,
    ) -> bool {
        // Inline SVGs are visual replaced elements even when `aria-hidden`.
        // Removing them as "placeholders" strips logos and icons from the page.
        if el.tag_name == "svg" {
            return false;
        }
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        const PLACEHOLDER_CLASSES: [&str; 33] = [
            "markupbox",
            "ad",
            "ads",
            "ad-slot",
            "ad__placeholder",
            "ad-placeholder",
            "ad-container",
            "ad-wrapper",
            "ad-wrap",
            "ad-unit",
            "advertisement",
            "sponsored",
            "dfp-ad",
            "adsbygoogle",
            "taboola",
            "outbrain",
            "hidden",
            "d-none",
            "invisible",
            "sr-only",
            "usa-sr-only",
            "visually-hidden",
            "screen-reader-text",
            "skip-links",
            "skiplink",
            "skip-to-main",
            // Generic skeleton/loading indicators. Real browsers either replace
            // these with content via JS or hide them; in the headless renderer
            // they often render as empty blocks or spinner text.
            "placeholder",
            "skeleton",
            "shimmer",
            "loading",
            "loader",
            "spinner",
            // Utility class for fully transparent/invisible elements. Real
            // browsers still keep them in the accessibility tree, but in a static
            // screenshot they contribute no visual content and often leave empty
            // boxes or off-screen wrappers (e.g. collapsed nav dropdowns).
            "opacity-0",
        ];
        // The generic `hidden` class is widely used to toggle visibility via JS
        // or responsive CSS (e.g. `.js-enabled .hidden { display: block; }`).
        // Keep contentful hidden containers (and those already covered by a
        // responsive display utility) so the renderer can reveal them later.
        if classes.contains("hidden") && (has_meaningful_content || has_visual_descendant) {
            return false;
        }
        if classes.contains("hidden") {
            const RESPONSIVE_BREAKPOINTS: [&str; 5] = ["sm:", "md:", "lg:", "xl:", "xxl:"];
            const RESPONSIVE_DISPLAYS: [&str; 8] = [
                "flex",
                "inline-flex",
                "block",
                "grid",
                "inline",
                "inline-block",
                "table",
                "contents",
            ];
            let has_responsive_display = classes.iter().any(|c| {
                RESPONSIVE_BREAKPOINTS.iter().any(|bp| {
                    c.strip_prefix(bp)
                        .map(|rest| RESPONSIVE_DISPLAYS.contains(&rest))
                        .unwrap_or(false)
                })
            });
            if has_responsive_display {
                return false;
            }
        }

        if classes.iter().any(|c| PLACEHOLDER_CLASSES.contains(c)) {
            return true;
        }
        // Hashed CSS-module / styled-components class names often embed a
        // placeholder token inside a longer name. Match those tokens
        // case-insensitively so the exact list above does not need to enumerate
        // every hashed variant.
        const PLACEHOLDER_SUBSTRINGS: [&str; 8] = [
            "adslot",
            "adsbygoogle",
            "dfp-ad",
            "taboola",
            "outbrain",
            "advertisement",
            // Many publishers wrap blocked ad slots in containers like
            // `.top-banner-ad-container` and `.ad-slot-container`.
            // Match the hyphenated tokens so these empty wrappers are removed.
            "ad-container",
            "ad-slot",
        ];
        if classes.iter().any(|c| {
            let c = c.to_ascii_lowercase();
            PLACEHOLDER_SUBSTRINGS.iter().any(|p| c.contains(p))
        }) {
            return true;
        }
        // `aria-hidden` only removes content from the accessibility tree; the
        // element still renders visually (icons, label buttons). Only treat it
        // as a placeholder when the subtree is empty of anything visible.
        if let Some(v) = el.get_attr("aria-hidden") {
            if v == "true" && !has_visual_descendant && !has_meaningful_content {
                return true;
            }
        }
        // Pages sometimes mark ad slots with `data-testid` attributes. Keep
        // the list ad-specific so that generic names like "loading" do not
        // remove real content containers.
        if let Some(v) = el.get_attr("data-testid") {
            if v == "ad" || v == "ad-slot" || v == "advertisement" {
                return true;
            }
        }
        false
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            // Accessibility-only skip links contain visible text but should still be
            // removed because they are positioned off-screen and pollute extracted text.
            if is_placeholder(el, has_visual_descendant[id], has_meaningful_content[id]) {
                to_remove.push(id);
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    let remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();

    for id in to_remove {
        if let Some(parent_id) = doc.nodes[id].parent {
            let parent = &mut doc.nodes[parent_id];
            parent.children.retain(|cid| !remove_set.contains(cid));
        }
    }
}

/// Trim horizontally-snapping carousels to their declared visible count.
///
/// Some pages render large collections of cards inside scroll-snapping
/// containers (`scroll-container snap-container-x count_N`). The CSS is meant
/// to show only `N` cards at a time and scroll the rest horizontally. Our
/// layout engine does not implement overflow scroll / snap, so every item gets
/// laid out vertically, producing enormous link farms. This helper keeps the
/// first `N` children of each such container and removes the rest, matching
/// the visible state in a real browser.
pub fn trim_scroll_snap_carousels(doc: &mut Document) {
    fn parse_count(classes: &[&str]) -> Option<usize> {
        for c in classes {
            if let Some(num) = c.strip_prefix("count_") {
                if let Ok(n) = num.parse() {
                    return Some(n);
                }
            }
        }
        None
    }

    fn is_scroll_container(el: &incognidium_dom::ElementData) -> bool {
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        classes.contains("scroll-container") && classes.contains("snap-container-x")
    }

    fn is_overflow_container(el: &incognidium_dom::ElementData) -> bool {
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        classes.contains("no-scrollbar") && classes.contains("overflow-x-auto")
    }

    fn is_scroll_item(el: &incognidium_dom::ElementData) -> bool {
        el.classes().contains(&"scroll-item")
    }

    fn is_list_item(el: &incognidium_dom::ElementData) -> bool {
        let tag = el.tag_name.as_str();
        tag == "li" || tag == "article"
    }

    fn is_list(el: &incognidium_dom::ElementData) -> bool {
        let tag = el.tag_name.as_str();
        tag == "ul" || tag == "ol"
    }

    let mut removals: Vec<(incognidium_dom::NodeId, Vec<incognidium_dom::NodeId>)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            let is_scroll = is_scroll_container(el);
            let is_overflow = is_overflow_container(el);
            if !is_scroll && !is_overflow {
                continue;
            }

            let count = if is_overflow {
                // Horizontal overflow carousels: keep the first 4 visible cards.
                4usize
            } else {
                match parse_count(&el.classes()) {
                    Some(n) if n > 0 => n,
                    _ => continue,
                }
            };

            // Scroll-snap containers often expose items as direct children with
            // the scroll-item class.
            if is_scroll {
                let children = doc.nodes[id].children.clone();
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = children
                    .iter()
                    .filter(|&&cid| {
                        if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                            if is_scroll_item(child_el) {
                                kept += 1;
                                return kept > count;
                            }
                        }
                        false
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    removals.push((id, to_remove));
                }
                continue;
            }

            // Overflow-x-auto carousels may expose their items directly, or wrap
            // them in a <ul>/<ol>, or wrap each card in a <div>. Trim the first
            // card-bearing child list/collection we find and leave spacers and
            // decorative wrappers alone.
            let container_children = doc.nodes[id].children.clone();
            let list_child = container_children.iter().find(|&&cid| {
                if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                    is_list(child_el)
                } else {
                    false
                }
            });

            if let Some(&list_id) = list_child {
                let list_children = doc.nodes[list_id].children.clone();
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = list_children
                    .iter()
                    .filter(|&&cid| {
                        if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                            if is_list_item(child_el) {
                                kept += 1;
                                return kept > count;
                            }
                        }
                        false
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    removals.push((list_id, to_remove));
                }
            } else {
                // No list wrapper; trim direct card-like children. Skip purely
                // decorative spacers (aria-hidden) and non-element nodes.
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = container_children
                    .iter()
                    .filter(|&&cid| {
                        if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                            if child_el.get_attr("aria-hidden") == Some("true") {
                                return false;
                            }
                            kept += 1;
                            return kept > count;
                        }
                        false
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    removals.push((id, to_remove));
                }
            }
        }
    }

    for (parent_id, to_remove) in removals {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove.iter().copied().collect();
        doc.nodes[parent_id]
            .children
            .retain(|cid| !set.contains(cid));
    }
}

/// Apply the shared set of DOM cleanups that a real browser performs through
/// JS, consent managers, ads, or responsive breakpoints. Keeping these fixes
/// in one place means the headless renderer, the crawl pipeline, and the
/// desktop shell all see the same sanitized document without duplicating the
/// logic in every binary.
pub fn preprocess_document(doc: &mut Document, _base_url: &str) {
    // JS DOM manipulation can leave cyclic or duplicate child/parent pointers.
    // Repair the tree before any traversal so cleanup passes and downstream
    // layout / paint cannot recurse forever.
    doc.sanitize_tree();

    // General skeleton / placeholder cleanup that applies to any page using
    // common utility-class / lazy-loading patterns.
    strip_lazy_image_skeletons(doc);
    strip_inline_bg_placeholders(doc);
    remove_empty_placeholders(doc);

    // Deduplicate accessibility text that is exposed twice (e.g. an image alt
    // that is also rendered as a visible caption, or SVG metadata that browsers
    // never show visually).
    strip_duplicate_img_alt_text(doc, "");
    dedupe_noscript_image_alts(doc);
    strip_duplicate_aria_labels(doc);
    strip_svg_metadata_text(doc);

    // Remove off-canvas accessibility skip links that real browsers keep hidden.
    remove_skip_links(doc);
}

/// Remove accessibility "Skip to ..." links that are hidden off-screen in real
/// browsers via `position: absolute; top: -9999px`. Our static layout does not
/// clip them reliably, so they can appear as stray text at the top of the
/// rendered page. Drop them for a cleaner screenshot.
fn remove_skip_links(doc: &mut Document) {
    let mut parent_map: HashMap<incognidium_dom::NodeId, incognidium_dom::NodeId> = HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    fn collect_text(doc: &Document, id: incognidium_dom::NodeId, out: &mut String) {
        match &doc.nodes[id].data {
            NodeData::Text(t) => out.push_str(&t.content),
            NodeData::Element(el) if el.tag_name == "script" || el.tag_name == "style" => {}
            _ => {
                for &cid in &doc.nodes[id].children {
                    collect_text(doc, cid, out);
                }
            }
        }
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "a" {
                continue;
            }
            let href = el.get_attr("href").unwrap_or("");
            if !href.starts_with('#') {
                continue;
            }
            let mut text = String::new();
            collect_text(doc, id, &mut text);
            let trimmed = text.trim().to_ascii_lowercase();
            if trimmed.starts_with("skip to") || trimmed == "skip links" {
                to_remove.push(id);
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    let removed_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();
    for id in to_remove {
        if let Some(&parent_id) = parent_map.get(&id) {
            if !removed_set.contains(&parent_id) {
                doc.nodes[parent_id].children.retain(|&cid| cid != id);
            }
        }
    }

    eprintln!("Removed {} skip link(s)", removed_set.len());
}

/// Maximum pixel dimension for rasterized inline SVGs. Icons should stay small;
/// large decorative SVGs are downscaled to keep memory and paint costs sane.
const MAX_INLINE_SVG_DIM: f32 = 512.0;
const MAX_INLINE_SVGS: usize = 100;

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn serialize_svg_subtree(
    doc: &Document,
    node_id: incognidium_dom::NodeId,
    out: &mut String,
    defs: &HashMap<String, incognidium_dom::NodeId>,
    external_defs: &HashMap<String, (String, Option<String>)>,
    depth: usize,
) {
    const MAX_SVG_DEPTH: usize = 10;
    let node = &doc.nodes[node_id];
    match &node.data {
        NodeData::Element(el) => {
            // Expand `<use href="#id">` (and the legacy `xlink:href` form) so that
            // SVG sprite icons -- common in menus and dropdown chevrons -- can be
            // rasterized even though usvg does not resolve external/external-by-id
            // references in the HTML DOM.
            if el.tag_name == "use" && depth < MAX_SVG_DEPTH {
                let href = el
                    .attributes
                    .get("href")
                    .or_else(|| el.attributes.get("xlink:href"))
                    .cloned()
                    .unwrap_or_default();
                if let Some(target_id) = href.strip_prefix('#').and_then(|id| defs.get(id)) {
                    if let Some(target_el) = doc.nodes.get(*target_id).and_then(|n| match &n.data {
                        NodeData::Element(ref e) => Some(e),
                        _ => None,
                    }) {
                        let target_node = &doc.nodes[*target_id];
                        let x = el.attributes.get("x").and_then(|s| s.parse::<f32>().ok());
                        let y = el.attributes.get("y").and_then(|s| s.parse::<f32>().ok());
                        let use_transform =
                            el.attributes.get("transform").cloned().unwrap_or_default();

                        // SVG <use> placement is a translate(x,y) followed by the element's
                        // own transform.  Combine them into a single transform on the emitted
                        // wrapper/target so references like
                        // <use href="#wordmark" transform="translate(34)"/> render at the
                        // correct position.
                        let mut transform_parts = Vec::new();
                        if !use_transform.is_empty() {
                            transform_parts.push(use_transform);
                        }
                        match (x, y) {
                            (Some(xv), Some(yv)) => {
                                transform_parts.push(format!("translate({}, {})", xv, yv));
                            }
                            (Some(xv), None) => {
                                transform_parts.push(format!("translate({}, 0)", xv));
                            }
                            (None, Some(yv)) => {
                                transform_parts.push(format!("translate(0, {})", yv));
                            }
                            _ => {}
                        }

                        if target_el.tag_name == "symbol" {
                            // A <symbol> is a template: render its children inside a
                            // group, applying any use offset/transform.  The surrounding
                            // <svg> supplies the viewport in the common sprite-sheet case.
                            out.push_str("<g");
                            if !transform_parts.is_empty() {
                                out.push_str(" transform=\"");
                                out.push_str(&escape_xml_attr(&transform_parts.join(" ")));
                                out.push('"');
                            }
                            out.push('>');
                            for &child_id in &target_node.children {
                                serialize_svg_subtree(
                                    doc,
                                    child_id,
                                    out,
                                    defs,
                                    external_defs,
                                    depth + 1,
                                );
                            }
                            out.push_str("</g>");
                        } else {
                            // For a direct shape/group reference, emit the referenced
                            // element, merging the <use>'s placement into a transform
                            // while preserving the target's own transform (applied last
                            // to the target's children).
                            let target_transform = target_el
                                .attributes
                                .get("transform")
                                .cloned()
                                .unwrap_or_default();
                            if !target_transform.is_empty() {
                                transform_parts.push(target_transform);
                            }
                            let combined_transform = transform_parts.join(" ");

                            out.push('<');
                            out.push_str(&target_el.tag_name);
                            for (k, v) in &target_el.attributes {
                                if k == "id" || k == "transform" {
                                    continue;
                                }
                                out.push(' ');
                                out.push_str(k);
                                out.push_str("=\"");
                                out.push_str(&escape_xml_attr(v));
                                out.push('"');
                            }
                            if !combined_transform.is_empty() {
                                out.push_str(" transform=\"");
                                out.push_str(&escape_xml_attr(&combined_transform));
                                out.push('"');
                            }
                            if target_node.children.is_empty() {
                                out.push_str("/>");
                            } else {
                                out.push('>');
                                for &child_id in &target_node.children {
                                    serialize_svg_subtree(
                                        doc,
                                        child_id,
                                        out,
                                        defs,
                                        external_defs,
                                        depth + 1,
                                    );
                                }
                                out.push_str("</");
                                out.push_str(&target_el.tag_name);
                                out.push('>');
                            }
                        }
                        return;
                    }
                }

                // Expand `<use href="url#id">` references that point at external SVG
                // sprite sheets.  The sprite is fetched once, the referenced symbol is
                // extracted and cached as a standalone SVG fragment, and inlined here.
                if let Some((fragment, viewbox)) = external_defs.get(&href) {
                    let x = el.attributes.get("x").and_then(|s| s.parse::<f32>().ok());
                    let y = el.attributes.get("y").and_then(|s| s.parse::<f32>().ok());
                    let width = el.attributes.get("width").cloned();
                    let height = el.attributes.get("height").cloned();
                    let use_transform = el.attributes.get("transform").cloned().unwrap_or_default();

                    // Emit a nested SVG viewport that preserves the referenced
                    // symbol's viewBox so scaling/aspect ratio matches the real
                    // browser.  The original `<use>` x/y/width/height become the
                    // nested SVG's geometry; if width/height are absent we let the
                    // nested SVG fill the referencing viewport.
                    out.push_str("<svg");
                    if let Some(xv) = x {
                        out.push_str(" x=\"");
                        out.push_str(&escape_xml_attr(&format!("{}", xv)));
                        out.push('"');
                    }
                    if let Some(yv) = y {
                        out.push_str(" y=\"");
                        out.push_str(&escape_xml_attr(&format!("{}", yv)));
                        out.push('"');
                    }
                    if let Some(w) = &width {
                        out.push_str(" width=\"");
                        out.push_str(&escape_xml_attr(w));
                        out.push('"');
                    }
                    if let Some(h) = &height {
                        out.push_str(" height=\"");
                        out.push_str(&escape_xml_attr(h));
                        out.push('"');
                    }
                    if let Some(vb) = viewbox {
                        out.push_str(" viewBox=\"");
                        out.push_str(&escape_xml_attr(vb));
                        out.push('"');
                        out.push_str(" preserveAspectRatio=\"xMidYMid meet\"");
                    }
                    if !use_transform.is_empty() {
                        out.push_str(" transform=\"");
                        out.push_str(&escape_xml_attr(&use_transform));
                        out.push('"');
                    }
                    out.push_str(" xmlns=\"http://www.w3.org/2000/svg\">");
                    out.push_str(fragment);
                    out.push_str("</svg>");
                    return;
                }
            }

            out.push('<');
            out.push_str(&el.tag_name);
            for (k, v) in &el.attributes {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                out.push_str(&escape_xml_attr(v));
                out.push('"');
            }
            // Inline SVGs in HTML often omit the SVG namespace, but usvg needs it
            // to parse the standalone XML document we build here.
            if el.tag_name == "svg" && !el.attributes.contains_key("xmlns") {
                out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
            }
            if node.children.is_empty() {
                // SVG elements like <line>, <path>, <circle> are typically empty in source;
                // write them as self-closing for compact XML.
                out.push_str("/>");
            } else {
                out.push('>');
                for &child_id in &node.children {
                    serialize_svg_subtree(doc, child_id, out, defs, external_defs, depth + 1);
                }
                out.push_str("</");
                out.push_str(&el.tag_name);
                out.push('>');
            }
        }
        NodeData::Text(t) => {
            out.push_str(&escape_xml_text(&t.content));
        }
        NodeData::Comment(c) => {
            out.push_str("<!--");
            out.push_str(c);
            out.push_str("-->");
        }
        _ => {}
    }
}

fn css_color_to_svg(color: CssColor) -> String {
    if color.a == 255 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        let a = color.a as f32 / 255.0;
        format!("rgba({}, {}, {}, {})", color.r, color.g, color.b, a)
    }
}

/// Inject presentation attributes (fill, stroke, etc.) into the root `<svg>`
/// tag of a serialized SVG fragment.  This ensures computed CSS styles that
/// apply to the inline `<svg>` element in the HTML document are visible to
/// `usvg` when it rasterizes the standalone XML document.
fn inject_svg_presentation_attrs(svg: &mut String, attrs: &[(String, String)]) {
    if attrs.is_empty() {
        return;
    }
    let Some(svg_start) = svg.find("<svg") else {
        return;
    };
    let after_tag = &svg[svg_start + 4..];
    // Find the closing `>` of the opening `<svg …>` tag.
    let Some(close_idx) = after_tag.find('>') else {
        return;
    };
    let tag_end = svg_start + 4 + close_idx;
    let tag_content = &svg[svg_start..tag_end];

    // Strip any existing occurrences of the attributes we are about to inject
    // so that the new values take precedence (XML parsers use the first match).
    let mut cleaned_tag = tag_content.to_string();
    for (k, _) in attrs {
        // Remove ` name="value"` or ` name='value'` variants.
        // This is a simple heuristic sufficient for serialized SVG.
        let pattern_space = format!(" {}=\"", k);
        while let Some(start) = cleaned_tag.find(&pattern_space) {
            let after_eq = start + pattern_space.len();
            if let Some(end) = cleaned_tag[after_eq..].find('"') {
                cleaned_tag.drain(start..after_eq + end + 1);
            } else {
                break;
            }
        }
        let pattern_space_s = format!(" {}='", k);
        while let Some(start) = cleaned_tag.find(&pattern_space_s) {
            let after_eq = start + pattern_space_s.len();
            if let Some(end) = cleaned_tag[after_eq..].find('\'') {
                cleaned_tag.drain(start..after_eq + end + 1);
            } else {
                break;
            }
        }
    }

    let mut injection = String::new();
    for (k, v) in attrs {
        injection.push(' ');
        injection.push_str(k);
        injection.push_str("=\"");
        injection.push_str(&escape_xml_attr(v));
        injection.push('"');
    }
    // Replace the old tag content with the cleaned one + new attributes.
    let before = svg[..svg_start].to_string();
    let after = svg[tag_end..].to_string();
    *svg = format!("{}{}{}", before, cleaned_tag, injection);
    svg.push_str(&after);
}

/// Convert a resolved CSS value to a string suitable for an SVG attribute.
/// This is intentionally minimal: inline SVGs mostly need colors, lengths,
/// percentages, and keywords.
fn css_value_to_svg_string(value: &CssValue) -> String {
    match value {
        CssValue::Color(c) => css_color_to_svg(*c),
        CssValue::Keyword(k) => k.clone(),
        CssValue::Length(n, u) => {
            let unit = match u {
                incognidium_css::LengthUnit::Px => "px",
                incognidium_css::LengthUnit::Em => "em",
                incognidium_css::LengthUnit::Rem => "rem",
                incognidium_css::LengthUnit::Pt => "pt",
                incognidium_css::LengthUnit::Percent => "%",
                incognidium_css::LengthUnit::Vw => "vw",
                incognidium_css::LengthUnit::Vh => "vh",
                incognidium_css::LengthUnit::Fr => "fr",
                incognidium_css::LengthUnit::Vmin => "vmin",
                incognidium_css::LengthUnit::Vmax => "vmax",
                incognidium_css::LengthUnit::Ex => "ex",
                incognidium_css::LengthUnit::Ch => "ch",
                incognidium_css::LengthUnit::Cap => "cap",
                incognidium_css::LengthUnit::Cm => "cm",
                incognidium_css::LengthUnit::Mm => "mm",
                incognidium_css::LengthUnit::In => "in",
                incognidium_css::LengthUnit::Pc => "pc",
            };
            format!("{}{}", n, unit)
        }
        CssValue::Percentage(p) => format!("{}%", p),
        CssValue::Number(n) => format!("{}", n),
        CssValue::Auto => "auto".to_string(),
        _ => {
            // Fallback: try to emit a sensible string. For complex values this
            // will not be SVG-valid, but it preserves the original token text
            // better than silently dropping it.
            format!("{:?}", value)
        }
    }
}

/// Resolve CSS `var(--name)` references inside a serialized SVG, using the
/// element's inherited custom properties. `fallback` is used when a variable is
/// not defined. This fixes inline SVGs that set `fill="var(--icon-color)"`
/// before `usvg` rasterizes them.
fn resolve_css_vars_in_svg(
    svg: &str,
    vars: &std::collections::HashMap<String, CssValue>,
) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find("var(") {
        out.push_str(&rest[..start]);
        let inner_start = start + 4;
        if inner_start >= rest.len() {
            // Malformed; keep the remainder unchanged.
            out.push_str(&rest[start..]);
            break;
        }
        // Find the matching ')' honoring nested parentheses.
        let mut depth = 1;
        let mut inner_end = None;
        for (i, c) in rest[inner_start..].char_indices() {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
                if depth == 0 {
                    inner_end = Some(inner_start + i);
                    break;
                }
            }
        }
        let Some(inner_end) = inner_end else {
            // No closing paren; keep remainder.
            out.push_str(&rest[start..]);
            break;
        };
        let inner = &rest[inner_start..inner_end];
        let mut parts = inner.splitn(2, ',');
        let name = parts.next().unwrap_or("").trim();
        let fallback = parts.next().map(|s| s.trim());
        let replacement = vars.get(name).map(|v| css_value_to_svg_string(v));
        if let Some(repl) = replacement {
            out.push_str(&repl);
        } else if let Some(fb) = fallback {
            out.push_str(fb);
        } else {
            // Variable undefined and no fallback: leave the var() call in place
            // so the SVG still contains a recognizable token.
            out.push_str(&rest[start..=inner_end]);
        }
        rest = &rest[inner_end + 1..];
    }
    out.push_str(rest);
    out
}

fn render_svg_xml_with_max_dim(
    svg: &str,
    current_color: CssColor,
    vars: Option<&std::collections::HashMap<String, CssValue>>,
    target_width: Option<f32>,
    target_height: Option<f32>,
    max_dimension: f32,
) -> Option<ImageData> {
    // Inline SVGs frequently use `currentColor` for strokes/fills so they match
    // the surrounding text color. usvg alone cannot resolve CSS `currentColor`,
    // so substitute the computed (or default) color before rasterizing.
    let color_str = css_color_to_svg(current_color);
    let mut svg = svg.replace("currentColor", &color_str);
    if let Some(vars) = vars {
        svg = resolve_css_vars_in_svg(&svg, vars);
    }
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&svg, &opt).ok()?;
    let size = tree.size();
    let intrinsic_w = size.width();
    let intrinsic_h = size.height();
    if intrinsic_w <= 0.0 || intrinsic_h <= 0.0 {
        return None;
    }

    // Raster at the CSS-resolved box size when one is provided; otherwise use
    // the SVG's intrinsic size. This prevents inline SVG logos from being
    // rendered at viewBox-unit scale and then down-scaled during paint, which
    // made them appear oversized in flex headers.
    let target_w = target_width.unwrap_or(intrinsic_w).max(1.0);
    let target_h = target_height.unwrap_or(intrinsic_h).max(1.0);
    let max_target_dim = target_w.max(target_h);
    let cap = if max_target_dim > max_dimension {
        max_dimension / max_target_dim
    } else {
        1.0
    };
    let render_w = (target_w * cap).ceil().max(1.0);
    let render_h = (target_h * cap).ceil().max(1.0);
    let scale_x = render_w / intrinsic_w;
    let scale_y = render_h / intrinsic_h;

    let w = render_w as u32;
    let h = render_h as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    let transform = tiny_skia::Transform::from_scale(scale_x, scale_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia uses premultiplied BGRA; convert to straight RGBA.
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in pixmap.pixels() {
        let a = px.alpha();
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let inv = 255.0 / a as f32;
            out.push(((px.red() as f32 * inv).min(255.0)) as u8);
            out.push(((px.green() as f32 * inv).min(255.0)) as u8);
            out.push(((px.blue() as f32 * inv).min(255.0)) as u8);
            out.push(a);
        }
    }
    Some(ImageData {
        pixels: out,
        width: w,
        height: h,
    })
}

/// Rasterize an SVG document, capping the output to keep inline logos from
/// being rendered at viewBox-unit scale and then down-scaled during paint.
fn render_svg_xml(
    svg: &str,
    current_color: CssColor,
    vars: Option<&std::collections::HashMap<String, CssValue>>,
    target_width: Option<f32>,
    target_height: Option<f32>,
) -> Option<ImageData> {
    render_svg_xml_with_max_dim(
        svg,
        current_color,
        vars,
        target_width,
        target_height,
        MAX_INLINE_SVG_DIM,
    )
}

/// Rasterize inline `<svg>` elements and turn them into `<img>` placeholders
/// that reference the raster in `image_cache`. This lets the existing layout
/// and paint pipelines render icon menus, logos, and other simple inline SVGs
/// without needing a full SVG layout implementation.
/// Resolve a CSS `<length> | <percentage> | calc()` size to pixels so that
/// inline SVG placeholders are rasterized at the same dimensions the author
/// intended, even when the resolved value is not stored as `SizeValue::Px`.
/// Viewport and containing-block sizes are approximated here because layout
/// has not run yet; for the common case of `rem`/`em`/`px` calc() expressions
/// used by icon fonts this is exact.
fn resolve_size_for_svg(
    value: &SizeValue,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
    is_height: bool,
) -> Option<f32> {
    fn calc_value_to_px(
        val: &CalcValue,
        font_size: f32,
        viewport_width: f32,
        viewport_height: f32,
        is_height: bool,
    ) -> f32 {
        let pct_basis = if is_height {
            viewport_height
        } else {
            viewport_width
        };
        match val {
            CalcValue::Px(v) => *v,
            CalcValue::Percent(p) => p / 100.0 * pct_basis,
            CalcValue::Em(e) => e * font_size,
            CalcValue::Rem(r) => r * 16.0,
            CalcValue::Vw(v) => v * viewport_width / 100.0,
            CalcValue::Vh(v) => v * viewport_height / 100.0,
            CalcValue::Cap(v) => v * font_size * 0.7,
            CalcValue::Cqw(v) => v * viewport_width / 100.0,
            CalcValue::Cqh(v) => v * viewport_height / 100.0,
            CalcValue::Cqi(v) => v * viewport_width / 100.0,
            CalcValue::Cqb(v) => v * viewport_height / 100.0,
            CalcValue::Cqmin(v) => v * viewport_width.min(viewport_height) / 100.0,
            CalcValue::Cqmax(v) => v * viewport_width.max(viewport_height) / 100.0,
        }
    }

    fn calc_expr_to_px(
        expr: &CalcExpression,
        font_size: f32,
        viewport_width: f32,
        viewport_height: f32,
        is_height: bool,
    ) -> f32 {
        match expr {
            CalcExpression::Value(v) => {
                calc_value_to_px(v, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Add(a, b) => {
                calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                    + calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Subtract(a, b) => {
                calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                    - calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Multiply(a, b) => {
                calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                    * calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Divide(a, b) => {
                let denom =
                    calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height);
                if denom == 0.0 {
                    0.0
                } else {
                    calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                        / denom
                }
            }
        }
    }

    let pct_basis = if is_height {
        viewport_height
    } else {
        viewport_width
    };
    match value {
        SizeValue::Px(v) => Some(*v),
        SizeValue::Percent(p) => Some(p / 100.0 * pct_basis),
        SizeValue::Calc(expr) => Some(calc_expr_to_px(
            expr,
            font_size,
            viewport_width,
            viewport_height,
            is_height,
        )),
        SizeValue::Min(vals) => vals
            .iter()
            .map(|v| calc_value_to_px(v, font_size, viewport_width, viewport_height, is_height))
            .reduce(f32::min),
        SizeValue::Max(vals) => vals
            .iter()
            .map(|v| calc_value_to_px(v, font_size, viewport_width, viewport_height, is_height))
            .reduce(f32::max),
        SizeValue::Clamp { min, val, max } => {
            let min_px =
                calc_value_to_px(min, font_size, viewport_width, viewport_height, is_height);
            let val_px =
                calc_value_to_px(val, font_size, viewport_width, viewport_height, is_height);
            let max_px =
                calc_value_to_px(max, font_size, viewport_width, viewport_height, is_height);
            Some(val_px.clamp(min_px, max_px))
        }
        _ => None,
    }
}

/// Collect external SVG sprite references of the form `<use href="url#id">`.
/// Returns a list of `(sprite_url, symbol_id, use_node_id)` tuples so the caller
/// can fetch each unique sprite once and extract the referenced symbols.
fn collect_external_svg_use_refs(doc: &Document) -> Vec<(String, String, incognidium_dom::NodeId)> {
    let mut refs = Vec::new();
    for n in &doc.nodes {
        if let NodeData::Element(el) = &n.data {
            if el.tag_name != "use" {
                continue;
            }
            let href = el
                .attributes
                .get("href")
                .or_else(|| el.attributes.get("xlink:href"))
                .cloned()
                .unwrap_or_default();
            if href.starts_with('#') || !href.contains('#') {
                continue;
            }
            let hash_idx = href.rfind('#').unwrap_or(href.len());
            let sprite_url = href[..hash_idx].to_string();
            let symbol_id = href[hash_idx + 1..].to_string();
            if sprite_url.is_empty() || symbol_id.is_empty() {
                continue;
            }
            refs.push((sprite_url, symbol_id, n.id));
        }
    }
    refs
}

/// Serialize the children of an external SVG `<symbol>` node.  The wrapper is
/// dropped because it is a template, but its `viewBox` is returned so the caller
/// can create a correctly scaled nested SVG viewport in place of the original
/// `<use>`.
fn serialize_external_svg_symbol(
    ext_doc: &Document,
    symbol_id: &str,
) -> Option<(String, Option<String>)> {
    let symbol_id = symbol_id.to_string();
    let symbol_node_id = ext_doc.nodes.iter().find_map(|n| {
        if let NodeData::Element(el) = &n.data {
            if el.tag_name == "symbol" && el.attributes.get("id") == Some(&symbol_id) {
                return Some(n.id);
            }
        }
        None
    })?;
    let mut out = String::new();
    let symbol_node = &ext_doc.nodes[symbol_node_id];
    let viewbox = if let NodeData::Element(el) = &symbol_node.data {
        el.attributes.get("viewBox").cloned()
    } else {
        None
    };
    for &child_id in &symbol_node.children {
        serialize_svg_subtree(
            ext_doc,
            child_id,
            &mut out,
            &HashMap::new(),
            &HashMap::new(),
            0,
        );
    }
    Some((out, viewbox))
}

/// Walk a layout tree and record the content-box size of every node that
/// establishes a container context.  The sizes are used by a second style-
/// resolution pass to evaluate real `@container` queries instead of guessing.
fn collect_container_sizes(
    layout_box: &LayoutBox,
    styles: &StyleMap,
    map: &mut HashMap<incognidium_dom::NodeId, (f32, f32)>,
) {
    if let Some(style) = styles.get(&layout_box.node_id) {
        if matches!(
            style.container_type,
            ContainerType::Size | ContainerType::InlineSize
        ) {
            map.insert(
                layout_box.node_id,
                (
                    layout_box.content_width.max(0.0),
                    layout_box.content_height.max(0.0),
                ),
            );
        }
    }
    for child in &layout_box.children {
        collect_container_sizes(child, styles, map);
    }
}

/// Re-resolve styles using container sizes measured from a real layout pass.
/// This makes `@container` queries match against actual container dimensions
/// rather than falling back to the viewport.
pub fn resolve_styles_with_container_sizes(
    doc: &Document,
    stylesheet: &incognidium_css::Stylesheet,
    viewport_width: f32,
    viewport_height: f32,
    layout_root: &LayoutBox,
    styles: &StyleMap,
) -> StyleMap {
    let mut container_sizes = HashMap::new();
    collect_container_sizes(layout_root, styles, &mut container_sizes);
    incognidium_style::resolve_styles_with_containers(
        doc,
        stylesheet,
        viewport_width,
        viewport_height,
        &container_sizes,
    )
}

pub fn rasterize_inline_svgs(
    doc: &mut Document,
    image_cache: &mut HashMap<String, ImageData>,
    mut styles: Option<&mut StyleMap>,
    viewport_width: f32,
    viewport_height: f32,
    base_url: Option<&str>,
) {
    let svg_ids: Vec<incognidium_dom::NodeId> = doc
        .nodes
        .iter()
        .filter_map(|n| {
            if let NodeData::Element(el) = &n.data {
                if el.tag_name == "svg" {
                    return Some(n.id);
                }
            }
            None
        })
        .collect();

    // Build a global map of `id`-attributed nodes so that SVG `<use href="#id">`
    // references can be expanded while serializing each SVG.  This covers both
    // symbols defined inside the same SVG and icons kept in a hidden sprite sheet
    // elsewhere in the document.
    let mut svg_defs: HashMap<String, incognidium_dom::NodeId> = HashMap::new();
    for n in &doc.nodes {
        if let NodeData::Element(el) = &n.data {
            if let Some(id) = el.attributes.get("id") {
                svg_defs.insert(id.clone(), n.id);
            }
        }
    }

    // Fetch external SVG sprite sheets referenced by `<use href="url#id">` and
    // cache the serialized symbol fragments.  Each unique sprite URL is fetched
    // once; failures are logged and the corresponding <use> remains empty.
    let mut external_defs: HashMap<String, (String, Option<String>)> = HashMap::new();
    let external_refs = collect_external_svg_use_refs(doc);
    let mut fetched_sprites: HashMap<String, Document> = HashMap::new();
    if let Some(base) = base_url {
        for (sprite_url, symbol_id, _use_id) in external_refs {
            let full_url = match resolve_url(base, &sprite_url) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!(
                        "Failed to resolve external SVG sprite {}: {}",
                        sprite_url, e
                    );
                    continue;
                }
            };
            let ext_doc = match fetched_sprites.entry(full_url.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let resp = match fetch_url(&full_url) {
                        Ok(r) => r,
                        Err(err) => {
                            eprintln!("Failed to fetch external SVG sprite {}: {}", full_url, err);
                            continue;
                        }
                    };
                    if resp.status < 200 || resp.status >= 300 {
                        eprintln!(
                            "External SVG sprite {} returned HTTP {}",
                            full_url, resp.status
                        );
                        continue;
                    }
                    e.insert(parse_html(&resp.body))
                }
            };
            let href = format!("{}#{}", sprite_url, symbol_id);
            if external_defs.contains_key(&href) {
                continue;
            }
            match serialize_external_svg_symbol(ext_doc, &symbol_id) {
                Some((fragment, viewbox)) => {
                    external_defs.insert(href, (fragment, viewbox));
                }
                None => {
                    eprintln!(
                        "External SVG sprite {} has no symbol #{}",
                        full_url, symbol_id
                    );
                }
            }
        }
    }

    let mut count = 0usize;
    for id in svg_ids {
        if count >= MAX_INLINE_SVGS {
            break;
        }
        let mut svg_xml = String::new();
        serialize_svg_subtree(doc, id, &mut svg_xml, &svg_defs, &external_defs, 0);
        if svg_xml.is_empty() {
            continue;
        }
        // Inline SVGs frequently rely on CSS `fill` / `stroke` set by author rules
        // (e.g. `.logo { fill: #2c0022; }`).  The computed values are not present
        // in the serialized element attributes, so usvg would default to black.
        // Inject them as presentation attributes on the root `<svg>` so the
        // rasterizer can see them.
        let mut svg_attrs: Vec<(String, String)> = Vec::new();
        if let Some(s) = styles.as_ref().and_then(|s| s.get(&id)) {
            if let Some(fill) = s.fill {
                svg_attrs.push(("fill".into(), css_color_to_svg(fill)));
            }
            if let Some(stroke) = s.stroke {
                svg_attrs.push(("stroke".into(), css_color_to_svg(stroke)));
            }
            if s.stroke_width > 0.0 {
                svg_attrs.push(("stroke-width".into(), format!("{}", s.stroke_width)));
            }
        }
        inject_svg_presentation_attrs(&mut svg_xml, &svg_attrs);

        let current_color = styles
            .as_ref()
            .and_then(|s| s.get(&id))
            .map(|s| s.color)
            .unwrap_or(CssColor::BLACK);
        let parent_id = doc.nodes[id].parent;
        let custom_props = styles
            .as_ref()
            .and_then(|s| s.get(&id))
            .map(|s| s.custom_properties.as_ref());
        let Some(mut img) = render_svg_xml(&svg_xml, current_color, custom_props, None, None)
        else {
            continue;
        };

        let key = format!("inline-svg:{id}");

        // Detach SVG children first so we can safely mutate the node data next.
        let children = std::mem::take(&mut doc.nodes[id].children);
        for child in children {
            doc.nodes[child].parent = None;
        }

        if let NodeData::Element(ref mut el) = doc.nodes[id].data {
            // Preserve author-specified dimensions if present. CSS widths/heights
            // take precedence over SVG attributes and intrinsic raster size,
            // otherwise icon fonts blow up to the raster's native resolution
            // (e.g. 512x512).
            let svg_width_attr = el
                .get_attr("width")
                .and_then(|w| w.trim_end_matches("px").parse::<f32>().ok());
            let svg_height_attr = el
                .get_attr("height")
                .and_then(|h| h.trim_end_matches("px").parse::<f32>().ok());

            let css_width = styles
                .as_ref()
                .and_then(|s| s.get(&id))
                .map(|s| s.width.clone())
                .filter(|w| !matches!(w, SizeValue::Auto | SizeValue::None));
            let css_height = styles
                .as_ref()
                .and_then(|s| s.get(&id))
                .map(|s| s.height.clone())
                .filter(|h| !matches!(h, SizeValue::Auto | SizeValue::None));

            let intrinsic_w = img.width as f32;
            let intrinsic_h = img.height as f32;

            let font_size = styles
                .as_ref()
                .and_then(|s| s.get(&id))
                .map(|s| s.font_size)
                .unwrap_or(16.0);

            let mut width_px = css_width.as_ref().and_then(|w| {
                resolve_size_for_svg(w, font_size, viewport_width, viewport_height, false)
            });
            let mut height_px = css_height.as_ref().and_then(|h| {
                resolve_size_for_svg(h, font_size, viewport_width, viewport_height, true)
            });

            // Only derive a missing dimension from the intrinsic aspect ratio when
            // the known CSS dimension is a definite absolute size. Deriving from a
            // percentage (e.g. `height: 100%` inside a 22 px wrapper) uses the wrong
            // basis before layout has resolved the real containing height, which
            // previously produced absurd placeholder widths such as a wordmark
            // logo expanding to thousands of pixels.
            let is_definite = |v: &SizeValue| {
                matches!(
                    v,
                    SizeValue::Px(_)
                        | SizeValue::Calc(_)
                        | SizeValue::Min(_)
                        | SizeValue::Max(_)
                        | SizeValue::Clamp { .. }
                )
            };
            let definite_width = css_width.as_ref().map_or(false, is_definite);
            let definite_height = css_height.as_ref().map_or(false, is_definite);

            if width_px.is_none() && height_px.is_some() && definite_height && intrinsic_h > 0.0 {
                width_px = Some((height_px.unwrap() / intrinsic_h) * intrinsic_w);
            } else if height_px.is_none()
                && width_px.is_some()
                && definite_width
                && intrinsic_w > 0.0
            {
                height_px = Some((width_px.unwrap() / intrinsic_w) * intrinsic_h);
            }

            let attr_width = if definite_width {
                width_px.or(svg_width_attr).unwrap_or(intrinsic_w)
            } else {
                svg_width_attr.unwrap_or(intrinsic_w)
            }
            .round()
            .max(1.0) as u32;
            let attr_height = if definite_height {
                height_px.or(svg_height_attr).unwrap_or(intrinsic_h)
            } else {
                svg_height_attr.unwrap_or(intrinsic_h)
            }
            .round()
            .max(1.0) as u32;

            // Re-raster at the resolved placeholder size so the cached bitmap matches
            // the CSS box the layout engine will use.  The first render above only
            // served to discover the SVG's intrinsic dimensions for the fallback
            // case; the final bitmap is produced here.
            if let Some(target_img) = render_svg_xml(
                &svg_xml,
                current_color,
                custom_props,
                Some(attr_width as f32),
                Some(attr_height as f32),
            ) {
                img = target_img;
            }

            let scale_to_parent = css_width.is_none()
                && css_height.is_none()
                && svg_width_attr.is_none()
                && svg_height_attr.is_none()
                && parent_id.is_some()
                && styles.as_ref().map_or(false, |s| {
                    if let Some(ps) = s.get(&parent_id.unwrap()) {
                        !matches!(ps.width, SizeValue::Auto | SizeValue::None)
                            && !matches!(ps.height, SizeValue::Auto | SizeValue::None)
                    } else {
                        false
                    }
                });

            // Keep the original tag alive for tag-name matching: the page's
            // CSS and scripts target the element as it was written in the
            // markup, not as the rasterized placeholder we swap in.
            el.selector_tag = Some(el.tag_name.clone());
            el.tag_name = "img".to_string();
            el.attributes.insert("src".to_string(), key.clone());
            el.attributes
                .insert("width".to_string(), attr_width.to_string());
            el.attributes
                .insert("height".to_string(), attr_height.to_string());
            // Alt text so the placeholder is accessible and visible even if
            // the raster is not in cache.
            if !el.attributes.contains_key("alt") {
                let alt = el.get_attr("aria-label").unwrap_or("").to_string();
                el.attributes.insert("alt".to_string(), alt);
            }
            image_cache.insert(key, img);
            // The element was an <svg> during style resolution, so the computed
            // style likely has the UA `display: none` that hides SVGs. The
            // rasterized placeholder is now an <img>; make sure it participates
            // in layout with its explicit dimensions.
            if let Some(styles) = styles.as_deref_mut() {
                let style = styles.entry(id).or_default();
                style.display = Display::Inline;
                if scale_to_parent {
                    // Width fills the containing block; height follows the SVG's
                    // intrinsic aspect ratio. This matches how browsers render an
                    // SVG with no explicit size inside a sized container.
                    style.width = SizeValue::Percent(100.0);
                    style.height = SizeValue::Auto;
                } else {
                    // CSS math functions (calc/min/max/clamp) are evaluated above
                    // so layout_image does not fall back to the intrinsic raster
                    // dimensions. Percentages and explicit pixel sizes are
                    // preserved so width:100% and author px heights still behave
                    // correctly. When the author gives only one definite absolute
                    // dimension, the other is locked to the derived pixel size. If
                    // a dimension is auto and could not be derived from a definite
                    // sibling, leave it auto so the layout engine uses the
                    // placeholder's intrinsic size and the real containing-block
                    // height instead of a viewport-basis guess.
                    let is_math_fn = |v: &SizeValue| {
                        matches!(
                            v,
                            SizeValue::Calc(_)
                                | SizeValue::Min(_)
                                | SizeValue::Max(_)
                                | SizeValue::Clamp { .. }
                        )
                    };
                    style.width = if css_width.as_ref().map_or(false, is_math_fn) {
                        SizeValue::Px(attr_width as f32)
                    } else if css_width.is_some() {
                        css_width.unwrap()
                    } else if width_px.is_some() {
                        SizeValue::Px(width_px.unwrap())
                    } else {
                        SizeValue::Auto
                    };
                    style.height = if css_height.as_ref().map_or(false, is_math_fn) {
                        SizeValue::Px(attr_height as f32)
                    } else if css_height.is_some() {
                        css_height.unwrap()
                    } else if height_px.is_some() {
                        SizeValue::Px(height_px.unwrap())
                    } else {
                        SizeValue::Auto
                    };
                }
                // Preserve the resolved dimensions as an inline style so that after
                // re-resolving styles the placeholder is not resized by author rules
                // that target `img` differently than the original `svg` (e.g.
                // `img,svg,video { width:100% }`). Inline styles have the highest
                // cascade origin and override those generic rules.
                let inline_width = match style.width {
                    SizeValue::Px(v) => format!("width:{}px", v),
                    SizeValue::Percent(p) => format!("width:{}%", p),
                    _ => "width:auto".to_string(),
                };
                let inline_height = match style.height {
                    SizeValue::Px(v) => format!("height:{}px", v),
                    SizeValue::Percent(p) => format!("height:{}%", p),
                    _ => "height:auto".to_string(),
                };
                let existing_style = el.get_attr("style").unwrap_or("").to_string();
                let mut new_style = existing_style;
                if !new_style.is_empty() && !new_style.ends_with(';') {
                    new_style.push(';');
                }
                if !new_style.is_empty() {
                    new_style.push(' ');
                }
                new_style.push_str(&inline_width);
                new_style.push(';');
                new_style.push(' ');
                new_style.push_str(&inline_height);
                new_style.push(';');
                el.attributes.insert("style".to_string(), new_style);
            }
            count += 1;
        }
    }
}

/// Predicate to skip synthetic inline-SVG URLs during normal image fetching.
pub fn is_inline_svg_url(url: &str) -> bool {
    url.starts_with("inline-svg:")
}

/// Encode a tiny-skia pixmap as a PNG using best compression. Smaller than
/// `pixmap.save_png()`/`encode_png()` while remaining lossless, which matters
/// for very long pages in the QA pipeline.
pub fn encode_png_compressed(
    pixmap: &Pixmap,
    writer: impl std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);
    encoder.write_image(
        pixmap.data(),
        pixmap.width(),
        pixmap.height(),
        image::ColorType::Rgba8.into(),
    )?;
    Ok(())
}

/// Convenience wrapper that writes a compressed PNG to disk.
pub fn save_png_compressed(pixmap: &Pixmap, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    encode_png_compressed(pixmap, file)
}

/// Largest dimension we keep for decoded raster images. Downsizing huge source
/// images (e.g. 3840px wide photos) saves memory and paint time without
/// affecting a 1024px-wide headless render.
pub const MAX_IMAGE_DIMENSION: u32 = 2048;

/// Maximum pixel dimension for CSS background images.
///
/// Background images are frequently sprite sheets where `background-position`
/// is expressed in pixels relative to the source's intrinsic size. Capping them
/// too aggressively breaks those offsets, so this limit is larger than the one
/// used for replaced `<img>` content while still guarding memory.
pub const MAX_BACKGROUND_IMAGE_DIMENSION: u32 = 4096;

/// Decode a raster image and cap its pixel dimensions.
///
/// Used by both the interactive browser and the `render_to_png` headless wrapper
/// so very large source images do not blow up memory or layout/paint costs.
pub fn decode_and_downscale_image(bytes: &[u8]) -> Option<ImageData> {
    use image::GenericImageView;
    if let Ok(mut img) = image::load_from_memory(bytes) {
        let (w, h) = img.dimensions();
        if w > MAX_IMAGE_DIMENSION || h > MAX_IMAGE_DIMENSION {
            let ratio = (w as f32).max(h as f32) / MAX_IMAGE_DIMENSION as f32;
            let new_w = ((w as f32) / ratio).max(1.0) as u32;
            let new_h = ((h as f32) / ratio).max(1.0) as u32;
            img = img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3);
        }
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return Some(ImageData {
            pixels: rgba.into_raw(),
            width: w,
            height: h,
        });
    }

    // The `image` crate does not decode SVG. Try rasterizing standalone SVG
    // documents so that logos and icons referenced via `<img src="...svg">` render.
    if looks_like_svg_bytes(bytes) {
        if let Ok(svg) = std::str::from_utf8(bytes) {
            return render_svg_xml(
                svg,
                CssColor {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                None,
                None,
                None,
            );
        }
    }

    None
}

/// Decode an image referenced by `background-image`, preserving its intrinsic size.
///
/// CSS sprite sheets rely on pixel-perfect `background-position` and
/// `background-size` values relative to the source image's intrinsic dimensions.
/// The general content-image decoder aggressively caps SVGs to keep inline
/// logos from being oversized, which breaks sprites, so background images use
/// a larger cap and are only downscaled when they genuinely exceed it.
pub fn decode_background_image(bytes: &[u8]) -> Option<ImageData> {
    use image::GenericImageView;
    if let Ok(mut img) = image::load_from_memory(bytes) {
        let (w, h) = img.dimensions();
        if w > MAX_BACKGROUND_IMAGE_DIMENSION || h > MAX_BACKGROUND_IMAGE_DIMENSION {
            let ratio = (w as f32).max(h as f32) / MAX_BACKGROUND_IMAGE_DIMENSION as f32;
            let new_w = ((w as f32) / ratio).max(1.0) as u32;
            let new_h = ((h as f32) / ratio).max(1.0) as u32;
            img = img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3);
        }
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return Some(ImageData {
            pixels: rgba.into_raw(),
            width: w,
            height: h,
        });
    }

    if looks_like_svg_bytes(bytes) {
        if let Ok(svg) = std::str::from_utf8(bytes) {
            if let Some(img) = render_svg_xml_with_max_dim(
                svg,
                CssColor {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                None,
                None,
                None,
                MAX_BACKGROUND_IMAGE_DIMENSION as f32,
            ) {
                return Some(img);
            }
        }
    }

    None
}

/// Collect `mask-image` icon URLs into the fetch queue (unresolvable data
/// URIs are decoded inline by the caller).
fn collect_mask_backgrounds(
    styles: &StyleMap,
    base_url: &str,
    urls: &mut Vec<(String, String)>,
    results: &mut Vec<(String, ImageData)>,
    seen: &mut std::collections::HashSet<String>,
) {
    for style in styles.values() {
        let Some(mask_src) = &style.mask_image else {
            continue;
        };
        if seen.contains(mask_src) {
            continue;
        }
        if let Some(img) = decode_data_uri_image(mask_src) {
            seen.insert(mask_src.clone());
            results.push((mask_src.clone(), img));
            continue;
        }
        if mask_src.starts_with("data:") {
            continue;
        }
        if let Ok(resolved) = resolve_url(base_url, mask_src) {
            if seen.insert(resolved.clone()) {
                urls.push((mask_src.clone(), resolved));
            }
        }
    }
}

/// Decode a `data:` URI carrying an image (base64 or percent-encoded) into
/// cached image data. Returns `None` for non-image payloads or malformed
/// URIs so callers can treat them like ordinary unfetchable URLs.
fn decode_data_uri_image(src: &str) -> Option<ImageData> {
    let rest = src.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let bytes = if meta.to_ascii_lowercase().contains(";base64") {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()?
    } else {
        // Percent-encoded UTF-8 payload (the common `data:image/svg+xml,...`
        // form). `%23` decodes to `#`, which SVG colors use heavily.
        urlencoding::decode(payload).ok()?.into_owned().into_bytes()
    };
    decode_background_image(&bytes)
}

/// Collect and fetch background images referenced by computed styles.
///
/// CSS `background-image: url(...)` is commonly used for logos, wordmarks,
/// and icons. The paint crate knows how to draw background images, but the
/// referenced URLs must first be resolved, fetched, and decoded into the
/// image cache. Both raster sources and standalone SVG documents are handled
/// by the shared decoder, which caps oversized images so memory and paint
/// costs stay bounded.
pub fn fetch_background_images(styles: &StyleMap, base_url: &str) -> Vec<(String, ImageData)> {
    const MAX_IMAGES: usize = 30;
    let mut urls: Vec<(String, String)> = Vec::new();
    let mut results: Vec<(String, ImageData)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for style in styles.values() {
        for img in &style.background_image {
            if let BackgroundImage::Url(src) = img {
                // Data URLs are embedded in the stylesheet: decode them
                // directly into the image cache (SVG-data-URI icons are
                // the standard way menus draw magnifiers, hamburgers, etc.).
                if let Some(img) = decode_data_uri_image(src) {
                    if seen.insert(src.clone()) {
                        results.push((src.clone(), img));
                    }
                    continue;
                }
                if src.starts_with("data:") {
                    continue;
                }
                if let Ok(resolved) = resolve_url(base_url, src) {
                    if seen.insert(resolved.clone()) {
                        urls.push((src.to_string(), resolved));
                    }
                }
            }
        }
        if urls.len() >= MAX_IMAGES {
            break;
        }
    }

    // Icons carried by `mask-image` (e.g. background-color masked to an icon
    // glyph) also need to be in the cache.
    collect_mask_backgrounds(styles, base_url, &mut urls, &mut results, &mut seen);

    if urls.is_empty() {
        return results;
    }

    // Limit concurrent subresource fetches per host to avoid tripping CDN
    // rate-limiters. A small gap between chunks keeps the request rate polite
    // without materially slowing most pages.
    for (i, chunk) in urls.chunks(4).enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let handles: Vec<_> = chunk
            .iter()
            .map(|(src, resolved)| {
                let src = src.clone();
                let resolved = resolved.clone();
                let referer = base_url.to_string();
                std::thread::spawn(move || {
                    match fetch_bytes_with_referer(&resolved, Some(&referer)) {
                        Ok(bytes) => {
                            if let Some(img) = decode_background_image(&bytes) {
                                Some((src, img))
                            } else {
                                None
                            }
                        }
                        Err(_) => None,
                    }
                })
            })
            .collect();

        for handle in handles {
            if let Ok(Some(result)) = handle.join() {
                results.push(result);
            }
        }
    }

    results
}

/// Collect and fetch inline images referenced by `<img src=\"...\">`.
///
/// Replaced elements need their intrinsic dimensions in `ImageSizes` so the
/// layout engine can size them correctly, and they must be in the image cache
/// so the paint engine can draw them. Skips `data:` URLs and synthetic
/// `inline-svg:` placeholders (those are produced locally by `rasterize_inline_svgs`).
pub fn fetch_document_images(doc: &Document, base_url: &str) -> Vec<(String, ImageData)> {
    const MAX_IMAGES: usize = 50;
    let mut urls: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for node in &doc.nodes {
        if urls.len() >= MAX_IMAGES {
            break;
        }
        if let NodeData::Element(ref el) = node.data {
            if el.tag_name != "img" {
                continue;
            }
            let raw_src: String = el
                .get_attr("src")
                .or_else(|| el.get_attr("data-src"))
                .map(|s| s.to_string())
                .or_else(|| {
                    // Pick the first candidate from a srcset if src is missing.
                    el.get_attr("srcset").and_then(first_srcset_url)
                })
                .or_else(|| {
                    // An `<img>` inside a `<picture>` may rely on a preceding
                    // `<source srcset>`. Fetch the first source candidate so the
                    // responsive image has intrinsic dimensions.
                    doc.nodes[node.id]
                        .parent
                        .and_then(|parent_id| {
                            let parent = &doc.nodes[parent_id];
                            if let NodeData::Element(parent_el) = &parent.data {
                                (parent_el.tag_name == "picture").then_some(parent)
                            } else {
                                None
                            }
                        })
                        .and_then(|parent| {
                            for &sibling_id in &parent.children {
                                if sibling_id == node.id {
                                    break;
                                }
                                let sibling = &doc.nodes[sibling_id];
                                if let NodeData::Element(sib_el) = &sibling.data {
                                    if sib_el.tag_name == "source" {
                                        if let Some(url) =
                                            sib_el.get_attr("srcset").and_then(first_srcset_url)
                                        {
                                            return Some(url);
                                        }
                                    }
                                }
                            }
                            None
                        })
                })
                .unwrap_or_default();
            if raw_src.is_empty() || raw_src.starts_with("data:") || is_inline_svg_url(&raw_src) {
                continue;
            }
            if let Ok(resolved) = resolve_url(base_url, &raw_src) {
                if seen.insert(resolved.clone()) {
                    urls.push((raw_src, resolved));
                }
            }
        }
    }

    if urls.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    for (i, chunk) in urls.chunks(4).enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let handles: Vec<_> = chunk
            .iter()
            .map(|(src, resolved)| {
                let src = src.clone();
                let resolved = resolved.clone();
                let referer = base_url.to_string();
                std::thread::spawn(move || {
                    match fetch_bytes_with_referer(&resolved, Some(&referer)) {
                        Ok(bytes) => {
                            if let Some(img) = decode_and_downscale_image(&bytes) {
                                Some((src, img))
                            } else {
                                None
                            }
                        }
                        Err(_) => None,
                    }
                })
            })
            .collect();

        for handle in handles {
            if let Ok(Some(result)) = handle.join() {
                results.push(result);
            }
        }
    }

    results
}

/// Heuristic: true if the byte stream looks like an SVG XML document.
fn looks_like_svg_bytes(bytes: &[u8]) -> bool {
    let prefix = std::str::from_utf8(&bytes[..bytes.len().min(256)])
        .unwrap_or("")
        .trim_start()
        .to_ascii_lowercase();
    prefix.starts_with("<?xml")
        || prefix.starts_with("<svg")
        || prefix.starts_with("<!doctype svg")
        || prefix.contains("<svg")
}

/// True when a flat box is positioned entirely outside the viewport
/// horizontally. Off-canvas hidden menus (e.g. `translateX(-500%)`) and
/// accessibility-only skip links (`left: -10000px`) should not count toward
/// extracted text.
pub fn is_box_offscreen(fbox: &FlatBox, viewport_width: f32) -> bool {
    let off_right = fbox.x >= viewport_width;
    let off_left = fbox.x + fbox.width <= -100.0;
    off_right || off_left
}

/// Fallback DOM text extraction used when the layout engine produces very few
/// text boxes. Walks the visible DOM tree in document order, collects text node
/// content, and uses meaningful accessibility attributes (aria-label, title,
/// alt, placeholder) when an element has no rendered child text. Block-level
/// elements are separated by newlines so the result remains readable.
pub fn extract_dom_text(
    doc: &Document,
    styles: &StyleMap,
    flat_boxes: &[FlatBox],
    viewport_width: f32,
) -> String {
    use incognidium_dom::NodeData;
    use incognidium_style::{Display, Visibility};

    let mut node_has_text_box: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();
    let mut node_offscreen_all: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();
    let mut node_text_seen: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();
    for fb in flat_boxes.iter() {
        if fb.text.is_none() || fb.box_type == incognidium_layout::BoxType::Image {
            continue;
        }
        let onscreen = !is_box_offscreen(fb, viewport_width);
        let id = fb.node_id;
        node_has_text_box.insert(id);
        if onscreen {
            node_offscreen_all.remove(&id);
        } else if !node_text_seen.contains(&id) {
            node_offscreen_all.insert(id);
        }
        node_text_seen.insert(id);
    }

    fn is_hidden(styles: &StyleMap, node_id: incognidium_dom::NodeId) -> bool {
        styles.get(&node_id).map_or(false, |s| {
            s.display == Display::None || s.visibility == Visibility::Hidden
        })
    }

    fn should_skip_tag(tag: &str) -> bool {
        matches!(
            tag,
            "script"
                | "style"
                | "noscript"
                | "template"
                | "head"
                | "meta"
                | "link"
                | "iframe"
                | "object"
                | "embed"
                | "audio"
                | "video"
                | "track"
                | "source"
                | "canvas"
                | "svg"
        )
    }

    fn is_block(tag: &str) -> bool {
        matches!(
            tag,
            "address"
                | "article"
                | "aside"
                | "blockquote"
                | "body"
                | "dd"
                | "div"
                | "dl"
                | "dt"
                | "fieldset"
                | "figcaption"
                | "figure"
                | "footer"
                | "form"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "header"
                | "hr"
                | "li"
                | "main"
                | "nav"
                | "ol"
                | "p"
                | "pre"
                | "section"
                | "table"
                | "tbody"
                | "td"
                | "tfoot"
                | "th"
                | "thead"
                | "tr"
                | "ul"
        )
    }

    fn attr_text(el: &incognidium_dom::ElementData, names: &[&str]) -> Option<String> {
        for name in names {
            if let Some(v) = el.attributes.get(*name) {
                let t = v.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
        None
    }

    fn attribute_label(el: &incognidium_dom::ElementData) -> Option<String> {
        let tag = el.tag_name.as_str();
        match tag {
            "img" => attr_text(el, &["alt", "aria-label", "title"]),
            "area" => attr_text(el, &["alt", "aria-label", "title"]),
            "input" | "textarea" | "button" | "select" => {
                attr_text(el, &["placeholder", "aria-label", "title", "value"])
            }
            _ => attr_text(el, &["aria-label", "title"]),
        }
    }

    fn collect_node(
        doc: &Document,
        styles: &StyleMap,
        node_id: incognidium_dom::NodeId,
        in_hidden: bool,
        offscreen_all: &std::collections::HashSet<incognidium_dom::NodeId>,
    ) -> Vec<String> {
        if in_hidden {
            return Vec::new();
        }
        if offscreen_all.contains(&node_id) {
            return Vec::new();
        }
        let node = &doc.nodes[node_id];
        match &node.data {
            NodeData::Element(el) => {
                let tag = el.tag_name.as_str();
                if should_skip_tag(tag) {
                    return Vec::new();
                }
                let hidden = in_hidden || is_hidden(styles, node_id);
                let mut parts: Vec<String> = Vec::new();
                for &child in &node.children {
                    parts.extend(collect_node(doc, styles, child, hidden, offscreen_all));
                }
                if parts.is_empty() {
                    if let Some(t) = attribute_label(el) {
                        parts.push(t);
                    }
                }
                if is_block(tag) && !parts.is_empty() {
                    let joined = parts.join(" ");
                    if !joined.is_empty() {
                        return vec![joined];
                    }
                }
                parts
            }
            NodeData::Text(t) => {
                let s = t.content.trim();
                if !s.is_empty() {
                    vec![s.to_string()]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    let mut parts = Vec::new();
    if let Some(html_id) = doc.document_element() {
        parts = collect_node(doc, styles, html_id, false, &node_offscreen_all);
    }
    parts.join("\n")
}

/// Extract page metadata (`<title>` and `meta description`) for use as a
/// fallback when visible rendered text is sparse.
pub fn extract_page_metadata(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for node in &doc.nodes {
        if let NodeData::Element(ref el) = node.data {
            match el.tag_name.as_str() {
                "title" => {
                    for &child in &node.children {
                        if let NodeData::Text(ref t) = doc.nodes[child].data {
                            let s = t.content.trim();
                            if !s.is_empty() {
                                out.push(s.to_string());
                            }
                        }
                    }
                }
                "meta" => {
                    let name = el.get_attr("name").unwrap_or_default().to_lowercase();
                    if name == "description" || name == "og:description" {
                        if let Some(v) = el.get_attr("content") {
                            let s = v.trim();
                            if !s.is_empty() {
                                out.push(s.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Compute the canvas background color per CSS background propagation.
///
/// If the root element (`<html>`) has no opaque background, the `<body>`
/// element's background is propagated to the root canvas. This matches
/// Firefox and other browsers when a page only sets `body { background: ... }`.
pub fn propagate_canvas_background(doc: &Document, styles: &StyleMap) -> Option<CssColor> {
    let html_id = doc.document_element()?;
    let body_id = doc.body()?;
    let html_bg = styles.get(&html_id)?.background_color;
    if html_bg.a > 0 {
        return Some(html_bg);
    }
    let body_bg = styles.get(&body_id)?.background_color;
    if body_bg.a > 0 {
        return Some(body_bg);
    }
    None
}

/// Recursively print the layout tree for debugging layout collapse.
pub fn dump_layout_tree(layout_box: &LayoutBox, doc: &Document, styles: &StyleMap, depth: usize) {
    let indent = "  ".repeat(depth);
    let (tag, _cls) = if layout_box.node_id >= doc.nodes.len() {
        (String::from("::pseudo"), String::new())
    } else {
        match &doc.nodes[layout_box.node_id].data {
            NodeData::Element(ref e) => {
                let mut tag = e.tag_name.clone();
                if let Some(id) = e.get_attr("id") {
                    tag.push('#');
                    tag.push_str(id);
                }
                (tag, e.get_attr("class").unwrap_or("").to_string())
            }
            _ => (String::from("#text"), String::new()),
        }
    };
    let text_preview = layout_box
        .text
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(40)
        .collect::<String>();
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    let pos = format!("{:?}", style.position).to_lowercase();
    let transform_info = if style.transform.is_empty() {
        String::new()
    } else {
        format!(" transform={:?}", style.transform)
    };
    let bg_info = if style.background_color.a == 0 {
        String::new()
    } else {
        format!(
            " bg=rgba({},{},{},{})",
            style.background_color.r,
            style.background_color.g,
            style.background_color.b,
            style.background_color.a
        )
    };
    eprintln!(
        "{}{} node={} [{:.0},{:.0} {}x{}] {:?} pos={} top={:?} bottom={:?} margin=({:.0},{:.0},{:.0},{:.0}){}{} text=\"{}\"",
        indent,
        tag,
        layout_box.node_id,
        layout_box.x,
        layout_box.y,
        layout_box.width,
        layout_box.height,
        layout_box.box_type,
        pos,
        style.top,
        style.bottom,
        style.margin_top,
        style.margin_right,
        style.margin_bottom,
        style.margin_left,
        transform_info,
        bg_info,
        text_preview.replace('\n', " ")
    );
    for child in &layout_box.children {
        dump_layout_tree(child, doc, styles, depth + 1);
    }
}

/// Dump the flat box list to a file for debugging layout issues.
pub fn dump_flat_boxes(path: &str, flat_boxes: &[FlatBox], doc: &Document, styles: &StyleMap) {
    use std::io::Write;
    let mut f = std::fs::File::create(path).expect("create dump file");
    for b in flat_boxes {
        let node = doc.nodes.get(b.node_id);
        let (tag, class) = match node.map(|n| &n.data) {
            Some(NodeData::Element(el)) => (
                el.tag_name.clone(),
                el.get_attr("class").unwrap_or_default().to_string(),
            ),
            Some(NodeData::Text(_)) => ("#text".to_string(), String::new()),
            _ => ("#other".to_string(), String::new()),
        };
        let (bg, fg, style_info) = styles
            .get(&b.node_id)
            .map(|s| {
                let pos = format!("{:?}", s.position).to_lowercase();
                (
                    format!(
                        "#{:02x}{:02x}{:02x}",
                        s.background_color.r, s.background_color.g, s.background_color.b
                    ),
                    format!("#{:02x}{:02x}{:02x}", s.color.r, s.color.g, s.color.b),
                    format!(
                        "pos={} w={:?} h={:?} left={:?} ml={:?}",
                        pos, s.width, s.height, s.left, s.margin_left
                    ),
                )
            })
            .unwrap_or_else(|| ("-".to_string(), "-".to_string(), "-".to_string()));
        let preview = b
            .text
            .as_ref()
            .map(|t| t.chars().take(60).collect::<String>().replace('\n', " "))
            .unwrap_or_default();
        writeln!(
            f,
            "node={} tag={} class={} [{:.1},{:.1} {:.1}x{:.1}] bg={} fg={} {} img={} text={}",
            b.node_id,
            tag,
            class,
            b.x,
            b.y,
            b.width,
            b.height,
            bg,
            fg,
            style_info,
            b.image_src.as_deref().unwrap_or(""),
            preview
        )
        .expect("write dump");
    }
    eprintln!("Dumped {} flat boxes to {path}", flat_boxes.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use incognidium_style::ComputedStyle;

    #[test]
    fn test_remove_empty_placeholders_keeps_hidden_responsive() {
        // Responsive utility pattern: `hidden lg:flex` should not be treated as a
        // placeholder, because the stylesheet makes it visible at the viewport width
        // used for comparisons.
        let html = r#"<!doctype html>
<html><body>
<div class="mx-auto flex flex-col flex-nowrap sm:max-w-2xl lg:max-w-6xl lg:flex-row-reverse">
  <div class="lg:w-1/2"><article>Right</article></div>
  <div class="hidden flex-col lg:flex lg:w-1/2 lg:pr-12">
    <article>Left</article>
  </div>
</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let outer = doc
            .body()
            .and_then(|body| {
                doc.node(body)
                    .children
                    .iter()
                    .find(|id| {
                        matches!(&doc.node(**id).data,
                            NodeData::Element(ref e) if e.tag_name == "div"
                        )
                    })
                    .copied()
            })
            .unwrap();
        let element_children: Vec<_> = doc
            .node(outer)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .copied()
            .collect();
        assert_eq!(
            element_children.len(),
            2,
            "responsive `hidden lg:flex` container must not be removed as a placeholder"
        );
    }

    #[test]
    fn test_remove_empty_placeholders_keeps_hidden_with_content() {
        // A generic `hidden` container may be revealed by JS or responsive CSS
        // (e.g. `.js-enabled .hidden { display: block; }`). Keep it if it has
        // meaningful content so style resolution can toggle it.
        let html = r#"<!doctype html>
<html><body>
<div class="hidden">Toggleable content</div>
<div class="visible">Keep me</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .collect();
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn test_remove_empty_placeholders_removes_empty_hidden() {
        // A truly empty `hidden` wrapper is a placeholder and should be removed.
        let html = r#"<!doctype html>
<html><body>
<div class="hidden"></div>
<div class="visible">Keep me</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .collect();
        assert_eq!(remaining.len(), 1);
        let kept_id = *remaining[0];
        if let NodeData::Element(ref e) = doc.node(kept_id).data {
            assert_eq!(e.get_attr("class"), Some("visible"));
        } else {
            panic!("expected the visible div to remain");
        }
    }

    #[test]
    fn test_remove_empty_placeholders_removes_styled_components_ad_slot() {
        // Some ad slots use a styled-components class like
        // `AdSlot-styles__AdSlotContainerStyled-sc-...` with no visible content.
        // The placeholder detector should catch the `AdSlot` token even though it
        // is embedded in a longer hashed class name.
        let html = r#"<!doctype html>
<html><body>
<div class="AdSlot-styles__AdSlotContainerStyled-sc-4b576bed-0 jQxGIx"></div>
<div class="real-article">Keep me</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .collect();
        assert_eq!(remaining.len(), 1);
        let kept_id = *remaining[0];
        if let NodeData::Element(ref e) = doc.node(kept_id).data {
            assert_eq!(e.get_attr("class"), Some("real-article"));
        } else {
            panic!("expected the real article to remain");
        }
    }

    #[test]
    fn test_remove_empty_placeholders_keeps_aria_hidden_image_wrapper() {
        // Decorative cover images often live inside an `aria-hidden="true"`
        // wrapper so screen readers ignore them. The wrapper is real visual
        // content, not a placeholder, and must survive cleanup.
        let html = r#"<!doctype html>
<html><body>
<div class="cover-image__wrap" aria-hidden="true" tabindex="-1">
  <div class="responsive-image">
    <img src="/hero.jpg" alt="A scenic mountain range" />
  </div>
</div>
<div class="real-article">Keep me</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .collect();
        assert_eq!(remaining.len(), 2, "aria-hidden image wrapper must be kept");
        let wrapper_id = *remaining[0];
        if let NodeData::Element(ref e) = doc.node(wrapper_id).data {
            assert_eq!(e.get_attr("class"), Some("cover-image__wrap"));
        } else {
            panic!("expected the image wrapper to remain");
        }
    }

    #[test]
    fn test_dedupe_noscript_image_alts() {
        let html = r##"<!doctype html>
<html><body>
  <figure>
    <picture>
      <img alt="A scientist in a lab" loading="lazy" />
      <noscript>
        <img src="/lab.jpg" alt="A scientist in a lab" />
      </noscript>
    </picture>
    <figcaption>Scientist at work</figcaption>
  </figure>
</body></html>
"##;
        let mut doc = parse_html(html);
        dedupe_noscript_image_alts(&mut doc);

        let noscript = doc
            .node(doc.body().unwrap())
            .children
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &doc.node(*id).data,
                    NodeData::Element(ref e) if e.tag_name == "figure"
                )
            })
            .and_then(|figure| {
                let mut stack = vec![figure];
                while let Some(id) = stack.pop() {
                    if let NodeData::Element(ref e) = doc.node(id).data {
                        if e.tag_name == "noscript" {
                            return Some(id);
                        }
                    }
                    stack.extend(doc.node(id).children.iter().copied());
                }
                None
            })
            .expect("noscript should remain");

        let noscript_img = doc
            .node(noscript)
            .children
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &doc.node(*id).data,
                    NodeData::Element(ref e) if e.tag_name == "img"
                )
            })
            .expect("noscript img should remain");
        if let NodeData::Element(ref e) = doc.node(noscript_img).data {
            assert_eq!(
                e.get_attr("alt"),
                Some(""),
                "noscript fallback alt should be cleared"
            );
        } else {
            panic!("expected noscript img element");
        }

        let main_img = doc
            .node(doc.body().unwrap())
            .children
            .iter()
            .copied()
            .find(|id| {
                matches!(
                    &doc.node(*id).data,
                    NodeData::Element(ref e) if e.tag_name == "figure"
                )
            })
            .and_then(|figure| {
                let mut stack = vec![figure];
                while let Some(id) = stack.pop() {
                    if id == noscript {
                        continue;
                    }
                    if let NodeData::Element(ref e) = doc.node(id).data {
                        if e.tag_name == "img" {
                            return Some(id);
                        }
                    }
                    stack.extend(doc.node(id).children.iter().copied());
                }
                None
            })
            .expect("main img should exist");
        if let NodeData::Element(ref e) = doc.node(main_img).data {
            assert_eq!(
                e.get_attr("alt"),
                Some("A scientist in a lab"),
                "main img alt should be preserved"
            );
        } else {
            panic!("expected main img element");
        }
    }

    #[test]
    fn test_promote_lazy_image_sources_swaps_data_src_and_class() {
        // Many themes hide `.lazyload[data-src]` images with `display:none !important`
        // and rely on JS to swap `data-src` to `src`.
        // Without this promotion the hero/article images never render.
        let html = r#"<!doctype html>
<html><body>
<img class="hero lazyload" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==" data-src="/hero.jpg" width="1024" height="683" loading="lazy" />
<img class="thumb lazyloading" data-src="/thumb.jpg" data-srcset="/thumb-400.jpg 400w, /thumb-800.jpg 800w" />
<img class="normal" src="/existing.jpg" />
</body></html>
"#;
        let mut doc = parse_html(html);
        promote_lazy_image_sources(&mut doc);
        let body = doc.body().unwrap();
        let images: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| {
                matches!(&doc.node(**id).data, NodeData::Element(ref e) if e.tag_name == "img")
            })
            .copied()
            .collect();
        assert_eq!(images.len(), 3);

        let hero = &doc.node(images[0]).data;
        if let NodeData::Element(ref e) = hero {
            assert_eq!(e.get_attr("src"), Some("/hero.jpg"));
            assert_eq!(e.get_attr("class"), Some("hero lazyloaded"));
            assert_eq!(e.get_attr("loading"), Some("eager"));
        } else {
            panic!("expected img element");
        }

        let thumb = &doc.node(images[1]).data;
        if let NodeData::Element(ref e) = thumb {
            assert_eq!(e.get_attr("src"), Some("/thumb.jpg"));
            assert_eq!(
                e.get_attr("srcset"),
                Some("/thumb-400.jpg 400w, /thumb-800.jpg 800w")
            );
            assert_eq!(e.get_attr("class"), Some("thumb lazyloaded"));
        } else {
            panic!("expected img element");
        }

        let normal = &doc.node(images[2]).data;
        if let NodeData::Element(ref e) = normal {
            assert_eq!(e.get_attr("src"), Some("/existing.jpg"));
            assert_eq!(e.get_attr("class"), Some("normal"));
        } else {
            panic!("expected img element");
        }
    }

    #[test]
    fn test_promote_lazy_image_sources_without_lazy_class() {
        // Some sites use data-src without the standard lazyload class.
        let html = r#"<!doctype html>
<html><body>
<img class="responsive-image__image" data-src="/hero.jpg" width="800" height="600" />
<img class="responsive-image__image" data-src="/card.jpg" src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7" />
<img class="responsive-image__image" src="/existing.jpg" data-src="/other.jpg" />
</body></html>
"#;
        let mut doc = parse_html(html);
        promote_lazy_image_sources(&mut doc);
        let body = doc.body().unwrap();
        let images: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| {
                matches!(&doc.node(**id).data,
                    NodeData::Element(ref e) if e.tag_name == "img"
                )
            })
            .copied()
            .collect();
        assert_eq!(images.len(), 3);

        // First image: no src, has data-src -> promoted
        let first = &doc.node(images[0]).data;
        if let NodeData::Element(ref e) = first {
            assert_eq!(e.get_attr("src"), Some("/hero.jpg"));
            assert_eq!(e.get_attr("class"), Some("responsive-image__image"));
        } else {
            panic!("expected img element");
        }

        // Second image: placeholder src, has data-src -> promoted
        let second = &doc.node(images[1]).data;
        if let NodeData::Element(ref e) = second {
            assert_eq!(e.get_attr("src"), Some("/card.jpg"));
        } else {
            panic!("expected img element");
        }

        // Third image: real src exists -> NOT promoted
        let third = &doc.node(images[2]).data;
        if let NodeData::Element(ref e) = third {
            assert_eq!(e.get_attr("src"), Some("/existing.jpg"));
        } else {
            panic!("expected img element");
        }
    }

    #[test]
    fn test_remove_skip_links() {
        let html = r##"<!doctype html>
<html><body>
  <a href="#content" class="skip-link">Skip to main content</a>
  <a href="#nav">Skip to navigation</a>
  <main id="content">
    <p>Real article content</p>
  </main>
</body></html>
"##;
        let mut doc = parse_html(html);
        remove_skip_links(&mut doc);

        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| {
                if let NodeData::Element(ref e) = doc.node(**id).data {
                    e.tag_name != "script" && e.tag_name != "style"
                } else {
                    false
                }
            })
            .copied()
            .collect();
        assert_eq!(remaining.len(), 1, "skip links should be removed");
        if let NodeData::Element(ref e) = doc.node(remaining[0]).data {
            assert_eq!(e.tag_name, "main");
        } else {
            panic!("expected main element");
        }
    }

    #[test]
    fn test_remove_skip_links_keeps_real_anchors() {
        let html = r##"<!doctype html>
<html><body>
  <a href="#content" class="skip-link">Skip to main content</a>
  <a href="/article">Read the article</a>
  <a href="#section">Jump to section</a>
</body></html>
"##;
        let mut doc = parse_html(html);
        remove_skip_links(&mut doc);

        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| {
                if let NodeData::Element(ref e) = doc.node(**id).data {
                    e.tag_name == "a"
                } else {
                    false
                }
            })
            .copied()
            .collect();
        assert_eq!(remaining.len(), 2, "only skip links should be removed");
        let texts: Vec<String> = remaining
            .iter()
            .map(|id| {
                let mut s = String::new();
                fn collect(doc: &Document, id: incognidium_dom::NodeId, out: &mut String) {
                    match &doc.nodes[id].data {
                        NodeData::Text(t) => out.push_str(&t.content),
                        _ => {
                            for &cid in &doc.nodes[id].children {
                                collect(doc, cid, out);
                            }
                        }
                    }
                }
                collect(&doc, *id, &mut s);
                s.trim().to_string()
            })
            .collect();
        assert!(texts.contains(&"Read the article".to_string()));
        assert!(texts.contains(&"Jump to section".to_string()));
    }

    #[test]
    fn test_strip_svg_metadata_text_removes_desc_and_title_inside_svg() {
        let html = r#"<!doctype html>
<html><head><title>Page title</title></head><body>
<svg width="120" height="58"><title>Logo</title><desc>Site logo</desc><path d="M0 0h120v58H0z"/></svg>
</body></html>"#;
        let mut doc = parse_html(html);
        strip_svg_metadata_text(&mut doc);

        let head = doc
            .nodes
            .iter()
            .find(|n| {
                matches!(&n.data, NodeData::Element(ref e) if e.tag_name == "head"
                )
            })
            .expect("head element");
        let head_has_title = head.children.iter().any(|id| {
            matches!(&doc.node(*id).data,
                NodeData::Element(ref e) if e.tag_name == "title"
            )
        });
        assert!(head_has_title, "HTML <title> in <head> must be preserved");

        let svg = doc
            .nodes
            .iter()
            .find(|n| {
                matches!(&n.data, NodeData::Element(ref e) if e.tag_name == "svg"
                )
            })
            .expect("svg element");
        let svg_has_metadata = svg.children.iter().any(|id| {
            matches!(
                &doc.node(*id).data,
                NodeData::Element(ref e) if e.tag_name == "title" || e.tag_name == "desc"
            )
        });
        assert!(
            !svg_has_metadata,
            "SVG <title>/<desc> metadata children should be removed"
        );
    }

    #[test]
    fn test_decode_and_downscale_image_falls_back_to_svg() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32">
            <rect width="32" height="32" fill="red" />
        </svg>"#;
        let img = decode_and_downscale_image(svg).expect("SVG should rasterize");
        assert_eq!(img.width, 32);
        assert_eq!(img.height, 32);
        // Top-left pixel should be opaque red.
        assert!(img.pixels[3] > 0);
        assert!(img.pixels[0] > 200);
    }

    #[test]
    fn test_fetch_background_images_skips_data_urls() {
        // Embedded data URLs are skipped; they are already present in the
        // stylesheet and do not need to be fetched separately.
        let mut styles = StyleMap::new();
        let mut style = ComputedStyle::default();
        style.background_image = vec![BackgroundImage::Url(
            "data:image/png;base64,AAA".to_string(),
        )];
        styles.insert(0, style);

        let fetched = fetch_background_images(&styles, "https://example.com/");
        assert!(fetched.is_empty());
        for (src, _) in &fetched {
            assert!(!src.starts_with("data:"));
        }
    }

    #[test]
    fn test_decode_background_image_preserves_svg_intrinsic_size() {
        // CSS sprite sheets depend on background-position being relative to the
        // source's intrinsic size. The content-image decoder caps SVGs at 512 px,
        // which would break sprites; the background decoder must keep the full
        // intrinsic dimensions up to its larger cap.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="900" height="900">
            <rect width="900" height="900" fill="red" />
        </svg>"#;
        let bg = decode_background_image(svg).expect("background SVG should rasterize");
        assert_eq!(bg.width, 900);
        assert_eq!(bg.height, 900);

        let content = decode_and_downscale_image(svg).expect("content SVG should rasterize");
        assert!(content.width <= 512, "content SVG should be capped");
        assert!(content.height <= 512, "content SVG should be capped");
    }
}
